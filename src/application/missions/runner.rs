//! The Mission workflow engine.
//!
//! The existing supervised workflow worker evaluates Mission readiness
//! alongside Goal readiness: Mission does not add a second permanent
//! scheduler loop. `evaluate_missions` advances at most one eligible
//! Mission per tick through one short, fenced transition or one one-shot
//! agent phase; long agent calls run inside exclusive durable operations
//! with stable ownership `mission:<id>:round:<n>:<stage>`.
//!
//! Human gates stop the engine: Plan approval, Review approval (and the
//! decision requests reconciliation raises) wait for a person. Everything
//! else advances deterministically from durable state.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::missions::phases;
use crate::application::missions::service::FileMissionService;
use crate::application::work_items::FileWorkItemService;
use crate::error::RefineResult;
use crate::infrastructure::agents::invocation::{AgentProviderService, HostAgentProviderService};
use crate::infrastructure::storage::project_layout::prepare_refine_dir;
use crate::model::mission::{Mission, MissionStatus};

/// The outcome of one evaluation tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MissionEvaluation {
    pub advanced: Option<String>,
    pub detail: Option<String>,
}

pub struct MissionWorkflowEngine {
    pub runtime_root: PathBuf,
    pub target_root: PathBuf,
    /// Test injection: when set, agent phases run against this provider
    /// instead of the installed host agent.
    provider_override: Option<Arc<dyn AgentProviderService>>,
    provider_name_override: Option<String>,
}

impl MissionWorkflowEngine {
    pub fn new(runtime_root: impl Into<PathBuf>, target_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            target_root: target_root.into(),
            provider_override: None,
            provider_name_override: None,
        }
    }

    /// Run agent phases against an explicit provider. Production callers use
    /// the installed host agent; tests inject a stub.
    pub fn with_provider(
        mut self,
        provider: Arc<dyn AgentProviderService>,
        provider_name: &str,
    ) -> Self {
        self.provider_override = Some(provider);
        self.provider_name_override = Some(provider_name.to_string());
        self
    }

    /// Evaluate every Mission of the active app and advance at most one.
    /// Expected waits (human gates, unsettled waves) are neutral; errors are
    /// surfaced for the worker to log, never looped on silently.
    pub fn evaluate_missions(&self) -> RefineResult<MissionEvaluation> {
        if workflow_paused(&self.runtime_root)? {
            return Ok(MissionEvaluation::default());
        }
        let refine_dir = prepare_refine_dir(&self.target_root)?;
        let service = FileMissionService::new(&refine_dir);
        let missions = service.list_missions()?;
        for mission in missions {
            if mission.status.is_terminal() {
                continue;
            }
            match self.evaluate_one(&service, &mission.id) {
                Ok(Some(detail)) => {
                    return Ok(MissionEvaluation {
                        advanced: Some(mission.id.clone()),
                        detail: Some(detail),
                    });
                }
                Ok(None) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(MissionEvaluation::default())
    }

    /// Advance one Mission by exactly one step. `None` means the Mission is
    /// legitimately waiting (human gate, unsettled wave, no eligible work).
    pub fn evaluate_one(
        &self,
        service: &FileMissionService,
        mission_id: &str,
    ) -> RefineResult<Option<String>> {
        let mission = service.show_mission(mission_id)?;
        match mission.status {
            MissionStatus::Draft => Ok(None),
            MissionStatus::Investigate => self
                .with_agents(|provider, provider_name| {
                    phases::investigation::run_investigation(
                        service,
                        provider.as_ref(),
                        provider_name,
                        &self.runtime_root,
                        &self.target_root,
                        mission_id,
                        &Default::default(),
                    )
                })
                .map(|_| Some("investigation published the initial snapshot".to_string())),
            MissionStatus::Plan => {
                // The engine may enter Execute only after the plan approval
                // evidence exists; approval itself is a human gate.
                let round = phases::current_round(&mission)?;
                let approved = round
                    .phase_evidence
                    .get("plan_approval")
                    .map(|approval| !approval.is_null())
                    .unwrap_or(false);
                if !approved {
                    return Ok(None);
                }
                if round.snapshots.is_empty() {
                    return Ok(None);
                }
                service.transition_mission(
                    mission_id,
                    MissionStatus::Execute,
                    Some(mission.revision),
                )?;
                Ok(Some("plan approved; Execute begins".to_string()))
            }
            MissionStatus::Execute => self.advance_execute(service, &mission),
            MissionStatus::Synthesize => self
                .with_agents(|provider, provider_name| {
                    phases::synthesis::run_synthesis(
                        service,
                        provider.as_ref(),
                        provider_name,
                        &self.runtime_root,
                        &self.target_root,
                        mission_id,
                    )
                })
                .map(|_| Some("synthesis settled the candidate Outcome".to_string())),
            MissionStatus::Quality => {
                let refine_dir = prepare_refine_dir(&self.target_root)?;
                let work_items = FileWorkItemService::new(&refine_dir);
                self.with_agents(|provider, provider_name| {
                    phases::quality::run_mission_quality(
                        service,
                        &work_items,
                        provider.as_ref(),
                        provider_name,
                        &self.runtime_root,
                        &self.target_root,
                        mission_id,
                    )
                })
                .map(|_| Some("quality passed".to_string()))
            }
            MissionStatus::Governance => self
                .with_agents(|provider, provider_name| {
                    phases::governance::run_mission_governance(
                        service,
                        provider.as_ref(),
                        provider_name,
                        &self.runtime_root,
                        &self.target_root,
                        mission_id,
                    )
                })
                .map(|_| Some("governance passed; Review awaits".to_string())),
            MissionStatus::Review => Ok(None),
            MissionStatus::Consolidate => phases::consolidation::consolidate(
                service,
                &self.target_root,
                &self.runtime_root,
                mission_id,
            )
            .map(|_| Some("outcome consolidated and published".to_string())),
            MissionStatus::Done | MissionStatus::Failed | MissionStatus::Cancelled => Ok(None),
        }
    }

    /// The Execute loop: materialize and admit the current wave, reconcile
    /// settled waves, sweep before Synthesis.
    fn advance_execute(
        &self,
        service: &FileMissionService,
        mission: &Mission,
    ) -> RefineResult<Option<String>> {
        let refine_dir = prepare_refine_dir(&self.target_root)?;
        let work_items = FileWorkItemService::new(&refine_dir);
        let round = phases::current_round(mission)?;
        let Some(plan) = round.plan.clone() else {
            return Ok(None);
        };

        // Find the wave to work on: the first wave that is not settled.
        let mut settled_waves = Vec::new();
        let mut active_wave = None;
        for wave in &plan.waves {
            let settlement =
                phases::reconcile::wave_settlement(&work_items, &mission.id, wave.number, &[])?;
            if settlement.settled {
                settled_waves.push(wave.number);
            } else if active_wave.is_none() {
                active_wave = Some(wave.number);
            }
        }

        if let Some(wave) = active_wave {
            // Materialize idempotently, then admit anything still in
            // Backlog; a wave in flight reports its pending state.
            let materialized =
                phases::execution::materialize_wave_goals(service, &work_items, &mission.id, wave)?;
            let created = materialized
                .goals
                .iter()
                .filter(|goal| goal.created)
                .count();
            let admission = phases::execution::admit_wave(service, &work_items, &mission.id, wave)?;
            let admitted = admission.goals.iter().filter(|goal| goal.admitted).count();
            if created > 0 || admitted > 0 {
                return Ok(Some(format!(
                    "wave {wave}: materialized {created}, admitted {admitted}"
                )));
            }
            return Ok(None);
        }

        // All waves settled: reconcile each settled wave once, then sweep.
        for wave in settled_waves {
            let claims = phases::reconcile::collect_claims(&work_items, &mission.id, Some(wave))?;
            if claims.is_empty() {
                continue;
            }
            self.with_agents(|provider, provider_name| {
                phases::reconcile::run_reconciliation(
                    service,
                    &work_items,
                    provider.as_ref(),
                    provider_name,
                    &self.runtime_root,
                    &self.target_root,
                    &mission.id,
                    Some(wave),
                )
            })?;
            return Ok(Some(format!("wave {wave} reconciled")));
        }
        if phases::reconcile::sweep_needed(&work_items, &mission.id)? {
            self.with_agents(|provider, provider_name| {
                phases::reconcile::run_reconciliation(
                    service,
                    &work_items,
                    provider.as_ref(),
                    provider_name,
                    &self.runtime_root,
                    &self.target_root,
                    &mission.id,
                    None,
                )
            })?;
            return Ok(Some("pre-Synthesis sweep reconciled".to_string()));
        }
        service.transition_mission(
            &mission.id,
            MissionStatus::Synthesize,
            Some(mission.revision),
        )?;
        Ok(Some("all waves reconciled; Synthesize begins".to_string()))
    }

    /// Run one agent closure against the injected provider (tests) or the
    /// installed host agent (production).
    fn with_agents<T>(
        &self,
        run: impl FnOnce(&Arc<dyn AgentProviderService>, &str) -> RefineResult<T>,
    ) -> RefineResult<T> {
        if let Some(provider) = &self.provider_override {
            let provider_name = self.provider_name_override.clone().unwrap_or_else(|| {
                provider
                    .detect()
                    .ok()
                    .and_then(|capabilities| capabilities.first().map(|c| c.name.clone()))
                    .unwrap_or_else(|| "stub".to_string())
            });
            return run(provider, &provider_name);
        }
        let provider_name = crate::application::missions::agent_phase::resolve_mission_provider(
            &self.runtime_root,
            None,
        )?;
        let provider: Arc<dyn AgentProviderService> = Arc::new(
            HostAgentProviderService::with_runtime_root(self.runtime_root.join("agents")),
        );
        run(&provider, &provider_name)
    }
}

/// Whether workflow automation is paused for this runtime. Mission
/// automation shares the Goal workflow pause: a paused system admits no new
/// phase work.
fn workflow_paused(runtime_root: &Path) -> RefineResult<bool> {
    Ok(
        crate::infrastructure::process::subprocess::FileProcessSupervisor::new(runtime_root)
            .pause_state()?
            .workflow_paused,
    )
}

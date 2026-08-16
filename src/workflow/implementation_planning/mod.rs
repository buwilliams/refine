use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::goal::{
    IMPLEMENTATION_PLAN_SCHEMA_VERSION, ImplementationAgentEvidence, ImplementationChecklistItem,
    ImplementationCriticism, ImplementationCriticismArtifact, ImplementationCriticismFinding,
    ImplementationExecutionEvidence, ImplementationPlan, ImplementationPlanArtifact,
    ImplementationPlanBinding, ImplementationPlanPhase, ImplementationPlanState,
    ImplementationPlanningFailure, ImplementationPlanningOutputAttempt, PlanGovernancePrecheck,
    ProposedImplementationPlan,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::implementation_planning::{
    criticism_result_contract_json, plan_result_contract_json, revision_result_contract_json,
};
use crate::prompts::structured_output::repair_prompt;
use crate::structured_output::{RepairPolicy, run_with_repair};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation, ProviderSessionContinuity,
};
use crate::workflow::{
    agent_idle_timeout, goal_agent_prompt, json_object, now_timestamp,
    parse_governance_provider_output, plan_governance_precheck_prompt,
};

use super::context::WorkflowContext;

mod codec;
mod prompts;
mod runtime;

use codec::*;
use prompts::*;
pub(crate) use runtime::recover_interrupted_plan;
use runtime::*;

/// How a planning phase participates in provider-native session continuity.
///
/// The plan phase pins a fresh session; its repairs and the revise phase
/// resume it. Criticize always runs fresh — independent eyes on the proposal
/// are the point of that phase.
#[derive(Clone)]
enum PlanningPhaseSession {
    Fresh,
    Pin(String),
    Resume(String),
}

impl PlanningPhaseSession {
    fn for_invocation(&self, is_repair: bool) -> Option<ProviderSessionContinuity> {
        match self {
            Self::Fresh => None,
            Self::Pin(session_id) if is_repair => {
                Some(ProviderSessionContinuity::Resume(session_id.clone()))
            }
            Self::Pin(session_id) => Some(ProviderSessionContinuity::Pin(session_id.clone())),
            Self::Resume(session_id) => Some(ProviderSessionContinuity::Resume(session_id.clone())),
        }
    }
}

pub(super) fn run_governed_implementation_planning(
    ctx: &WorkflowContext<'_>,
    goal: &Value,
    agent_context: &Value,
    agent_cwd: &Path,
    implementation_branch: &str,
) -> RefineResult<ProposedImplementationPlan> {
    let spec = goal_agent_prompt(&ctx.goal_id, agent_context)?;
    let mut plan = load_or_initialize_plan(ctx, goal, agent_context, implementation_branch)?;

    if let Some(final_plan) = ensure_scoped_recovery_plan(ctx, goal, &mut plan, agent_cwd)? {
        return Ok(final_plan);
    }

    if plan.proposal.is_none() {
        begin_phase(ctx, &mut plan, ImplementationPlanPhase::Plan)?;
        let pinned_session =
            HostAgentProviderService::provider_supports_interactive_session_continuity(
                &ctx.provider,
            )
            .then(|| uuid::Uuid::new_v4().to_string());
        let (run, result) = run_observational_phase_with_repair(
            ctx,
            &mut plan,
            agent_cwd,
            implementation_branch,
            ImplementationPlanPhase::Plan,
            "plan",
            planning_prompt(&spec),
            "implementation plan",
            &plan_result_contract_json(),
            pinned_session
                .clone()
                .map(PlanningPhaseSession::Pin)
                .unwrap_or(PlanningPhaseSession::Fresh),
            decode_plan,
        )?;
        let previous = plan.clone();
        plan.proposal = Some(ImplementationPlanArtifact {
            started_at: run.started_at,
            completed_at: run.completed_at,
            git_before: run.git_before,
            git_after: run.git_after,
            result,
        });
        plan.provider_session_id = pinned_session;
        plan.updated_at = now_timestamp();
        persist_plan(ctx, Some(&previous), &plan)?;
    }

    if plan.criticism.is_none() {
        begin_phase(ctx, &mut plan, ImplementationPlanPhase::Criticize)?;
        let proposal = plan
            .proposal
            .as_ref()
            .expect("proposal phase persisted")
            .result
            .clone();
        let prompt = criticism_prompt(&spec, &proposal)?;
        let (run, result) = run_observational_phase_with_repair(
            ctx,
            &mut plan,
            agent_cwd,
            implementation_branch,
            ImplementationPlanPhase::Criticize,
            "criticize",
            prompt,
            "implementation criticism",
            &criticism_result_contract_json(),
            PlanningPhaseSession::Fresh,
            decode_criticism,
        )?;
        let previous = plan.clone();
        plan.criticism = Some(ImplementationCriticismArtifact {
            started_at: run.started_at,
            completed_at: run.completed_at,
            git_before: run.git_before,
            git_after: run.git_after,
            result,
        });
        plan.updated_at = now_timestamp();
        persist_plan(ctx, Some(&previous), &plan)?;
    }

    if plan.final_plan.is_none() {
        begin_phase(ctx, &mut plan, ImplementationPlanPhase::Revise)?;
        let proposal = plan
            .proposal
            .as_ref()
            .expect("proposal phase persisted")
            .result
            .clone();
        let criticism = plan
            .criticism
            .as_ref()
            .expect("criticism phase persisted")
            .result
            .clone();
        let prompt = revision_prompt(&spec, &proposal, &criticism)?;
        let revise_session = plan
            .provider_session_id
            .clone()
            .map(PlanningPhaseSession::Resume)
            .unwrap_or(PlanningPhaseSession::Fresh);
        let (run, result) = run_observational_phase_with_repair(
            ctx,
            &mut plan,
            agent_cwd,
            implementation_branch,
            ImplementationPlanPhase::Revise,
            "revise",
            prompt,
            "revised implementation plan",
            &revision_result_contract_json(),
            revise_session,
            |output| {
                let result = decode_plan(output)?;
                validate_revised_plan(&result, &criticism)?;
                Ok(result)
            },
        )?;
        let previous = plan.clone();
        plan.final_plan = Some(ImplementationPlanArtifact {
            started_at: run.started_at,
            completed_at: run.completed_at,
            git_before: run.git_before,
            git_after: run.git_after,
            result,
        });
        plan.updated_at = now_timestamp();
        persist_plan(ctx, Some(&previous), &plan)?;
    }

    ensure_plan_governance_precheck(
        ctx,
        &mut plan,
        agent_context,
        agent_cwd,
        implementation_branch,
        &spec,
    )?;

    Ok(plan.final_plan.expect("revision phase persisted").result)
}

/// A Quality/Governance recovery Round skips the planning trio: the finding's
/// drafted recovery request is already a reviewed delta against a candidate
/// both gates have seen, so its plan is synthesized deterministically and the
/// Round spends its agent budget on the fix. The full Quality and Governance
/// gates still judge the result, and every planning invariant that downstream
/// steps rely on — a persisted final plan, checklist-id evidence matching —
/// holds unchanged.
fn ensure_scoped_recovery_plan(
    ctx: &WorkflowContext<'_>,
    goal: &Value,
    plan: &mut ImplementationPlan,
    agent_cwd: &Path,
) -> RefineResult<Option<ProposedImplementationPlan>> {
    let Some(round) = goal
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
    else {
        return Ok(None);
    };
    let Some(retry) = round.get("automatic_retry") else {
        return Ok(None);
    };
    let Some(kind) = retry
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| matches!(*kind, "quality" | "governance"))
    else {
        return Ok(None);
    };
    let source_round = retry
        .get("source_round")
        .and_then(Value::as_u64)
        .unwrap_or(ctx.round_idx as u64);
    if let Some(final_plan) = plan.final_plan.as_ref() {
        return Ok(Some(final_plan.result.clone()));
    }
    let git = crate::tools::host::git_worktrees::FileGitWorktreeService::with_runtime_root(
        agent_cwd,
        ctx.runtime_root,
    );
    let observation = git.implementation_planning_observation()?;
    let now = now_timestamp();
    let result = ProposedImplementationPlan {
        summary: format!(
            "Scoped {kind} recovery for Round {source_round}: implement the current Round request directly on the retained candidate. Planning is skipped for this recovery; the full Quality and Governance gates still judge the result."
        ),
        checklist: vec![ImplementationChecklistItem {
            id: "R1".to_string(),
            description: format!(
                "Correct the Round {source_round} {kind} findings by implementing the current Round request on the retained candidate."
            ),
            affected_behavior: Vec::new(),
            governance_rationale: None,
            verification: Vec::new(),
        }],
        criticism_resolutions: Vec::new(),
    };
    let artifact = ImplementationPlanArtifact {
        started_at: now.clone(),
        completed_at: now.clone(),
        git_before: observation.clone(),
        git_after: observation,
        result: result.clone(),
    };
    let previous = plan.clone();
    plan.proposal = Some(artifact.clone());
    plan.final_plan = Some(artifact);
    plan.updated_at = now;
    persist_plan(ctx, Some(&previous), plan)?;
    ctx.log(
        "plan",
        "Scoped recovery Round synthesized its plan from the drafted recovery request",
        Some(json_object(serde_json::json!({
            "kind": kind,
            "source_round": source_round
        }))),
    )?;
    Ok(Some(result))
}

/// Advisory governance verdict on the finalized plan, before implementation.
///
/// A failing verdict is folded back into one extra revise pass as material
/// criticism; a plan that still fails settles the Round here, saving the whole
/// implement-and-quality spend the post-implementation gate would otherwise
/// reject. The post-implementation gate stays authoritative — an unreadable
/// pre-check verdict is recorded as inconclusive and the Round proceeds.
fn ensure_plan_governance_precheck(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    agent_context: &Value,
    agent_cwd: &Path,
    implementation_branch: &str,
    spec: &str,
) -> RefineResult<()> {
    let Some(governance) = agent_context.get("governance").cloned() else {
        return Ok(());
    };
    let rules = governance
        .get("rules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let guidance = agent_context
        .get("guidance_candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let product_or_constitution_configured = ["product", "constitution"].iter().any(|key| {
        governance
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    });
    if rules.is_empty() && guidance.is_empty() && !product_or_constitution_configured {
        return Ok(());
    }
    if cfg!(test)
        && ctx.provider == "smoke-ai"
        && std::env::var("REFINE_SMOKE_AI_PLAN_GOVERNANCE").as_deref() != Ok("1")
    {
        return Ok(());
    }
    loop {
        match plan.governance_precheck.clone() {
            Some(precheck) if precheck.status == "passed" || precheck.status == "inconclusive" => {
                return Ok(());
            }
            Some(precheck) if precheck.status == "failed" && precheck.attempt >= 2 => {
                let message = precheck
                    .message
                    .unwrap_or_else(|| "governance violations remained".to_string());
                return Err(persist_phase_failure(
                    ctx,
                    plan,
                    ImplementationPlanPhase::Revise,
                    "plan_governance",
                    RefineError::Conflict(format!(
                        "finalized plan still violates governance after one revision: {message}"
                    )),
                ));
            }
            Some(precheck) if precheck.status == "failed" => {
                revise_plan_for_governance(
                    ctx,
                    plan,
                    agent_cwd,
                    implementation_branch,
                    spec,
                    &precheck,
                )?;
            }
            _ => {
                let attempt = plan
                    .governance_precheck
                    .as_ref()
                    .map(|precheck| precheck.attempt)
                    .unwrap_or(1);
                run_plan_governance_verdict(
                    ctx,
                    plan,
                    &governance,
                    &rules,
                    &guidance,
                    agent_cwd,
                    attempt,
                )?;
            }
        }
    }
}

fn run_plan_governance_verdict(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    governance: &Value,
    rules: &[Value],
    guidance: &[Value],
    agent_cwd: &Path,
    attempt: u32,
) -> RefineResult<()> {
    let final_plan = plan
        .final_plan
        .as_ref()
        .map(|artifact| artifact.result.clone())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {} round {} has no finalized plan for the governance pre-check",
                ctx.goal_id,
                ctx.round_idx + 1
            ))
        })?;
    let plan_json = serde_json::to_string_pretty(&final_plan).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode finalized plan for the governance pre-check: {error}"
        ))
    })?;
    let prompt = plan_governance_precheck_prompt(
        governance,
        rules,
        guidance,
        agent_cwd,
        &ctx.goal_id,
        ctx.round_idx,
        &plan_json,
    );
    ctx.revalidate_authority(crate::model::workflow::GoalStatus::Plan)?;
    let git = crate::tools::host::git_worktrees::FileGitWorktreeService::with_runtime_root(
        agent_cwd,
        ctx.runtime_root,
    );
    let git_before = git.implementation_planning_observation()?;
    let provider = HostAgentProviderService::with_runtime_root(ctx.runtime_root.join("agents"));
    let mut metadata = ctx.workflow_process_metadata("plan", "WorkflowPlanGovernancePrecheck");
    metadata.insert(
        "implementation_phase".to_string(),
        serde_json::json!("plan_governance"),
    );
    let invocation = provider.invoke(ProviderInvocation {
        provider: ctx.provider.clone(),
        prompt,
        session_id: None,
        cwd: Some(agent_cwd.display().to_string()),
        stall_timeout_seconds: agent_idle_timeout(&ctx.settings).map(|timeout| timeout.as_secs()),
        process_metadata: metadata,
    });
    let git_after = git.implementation_planning_observation()?;
    ctx.revalidate_authority(crate::model::workflow::GoalStatus::Plan)?;
    if git_after != git_before {
        let error = RefineError::Conflict(
            "plan governance pre-check changed the worktree; changes were retained".to_string(),
        );
        let failure_result = record_failure(
            ctx,
            plan,
            ImplementationPlanningFailure {
                phase: ImplementationPlanPhase::Revise,
                category: "worktree_mutation".to_string(),
                message: error.to_string(),
                failed_at: now_timestamp(),
                git_before: Some(git_before),
                git_after: Some(git_after),
            },
        );
        return Err(failure_or_persistence_error(error, failure_result));
    }
    let (status, evaluation) = match invocation {
        Ok(output) => {
            let mut evaluation = parse_governance_provider_output(&output, rules.len());
            evaluation
                .details
                .insert("phase".to_string(), Value::String("plan".to_string()));
            let parse_error = evaluation
                .details
                .get("verdict_parse_error")
                .and_then(Value::as_bool)
                == Some(true);
            let status = if parse_error {
                "inconclusive"
            } else if evaluation.failed {
                "failed"
            } else {
                "passed"
            };
            (status, Some(evaluation))
        }
        // Advisory: a provider fault here must not fail planning the
        // authoritative gate would still have caught for real violations.
        Err(error) => {
            ctx.log(
                "governance",
                "Plan governance pre-check provider failed; proceeding without a pre-check verdict",
                Some(json_object(serde_json::json!({
                    "phase": "plan",
                    "error": error.to_string()
                }))),
            )?;
            ("inconclusive", None)
        }
    };
    let previous = plan.clone();
    plan.governance_precheck = Some(PlanGovernancePrecheck {
        status: status.to_string(),
        attempt,
        violations: evaluation
            .as_ref()
            .and_then(|evaluation| evaluation.details.get("failed_actions"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        message: evaluation
            .as_ref()
            .and_then(|evaluation| evaluation.message.clone()),
        checked_at: now_timestamp(),
    });
    plan.updated_at = now_timestamp();
    persist_plan(ctx, Some(&previous), plan)?;
    ctx.log(
        "governance",
        match status {
            "passed" => "Plan governance pre-check passed",
            "failed" => "Plan governance pre-check found violations",
            _ => "Plan governance pre-check was inconclusive",
        },
        Some(json_object(serde_json::json!({
            "phase": "plan",
            "attempt": attempt,
            "status": status
        }))),
    )?;
    Ok(())
}

/// Fold pre-check violations back into the existing criticize → revise
/// contract: each violation becomes a material finding the revised plan must
/// resolve, produced by one additional revise pass.
fn revise_plan_for_governance(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    agent_cwd: &Path,
    implementation_branch: &str,
    spec: &str,
    precheck: &PlanGovernancePrecheck,
) -> RefineResult<()> {
    let proposal = plan
        .final_plan
        .as_ref()
        .map(|artifact| artifact.result.clone())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {} round {} has no finalized plan to revise for governance",
                ctx.goal_id,
                ctx.round_idx + 1
            ))
        })?;
    let criticism = governance_precheck_criticism(precheck);
    let prompt = revision_prompt(spec, &proposal, &criticism)?;
    let session = plan
        .provider_session_id
        .clone()
        .map(PlanningPhaseSession::Resume)
        .unwrap_or(PlanningPhaseSession::Fresh);
    let (run, result) = run_observational_phase_with_repair(
        ctx,
        plan,
        agent_cwd,
        implementation_branch,
        ImplementationPlanPhase::Revise,
        "revise",
        prompt,
        "revised implementation plan",
        &revision_result_contract_json(),
        session,
        |output| {
            let result = decode_plan(output)?;
            validate_revised_plan(&result, &criticism)?;
            Ok(result)
        },
    )?;
    let previous = plan.clone();
    plan.final_plan = Some(ImplementationPlanArtifact {
        started_at: run.started_at,
        completed_at: run.completed_at,
        git_before: run.git_before,
        git_after: run.git_after,
        result,
    });
    plan.governance_precheck = Some(PlanGovernancePrecheck {
        status: "revised".to_string(),
        attempt: precheck.attempt + 1,
        violations: precheck.violations.clone(),
        message: precheck.message.clone(),
        checked_at: now_timestamp(),
    });
    plan.updated_at = now_timestamp();
    persist_plan(ctx, Some(&previous), plan)
}

fn governance_precheck_criticism(precheck: &PlanGovernancePrecheck) -> ImplementationCriticism {
    let mut findings = precheck
        .violations
        .iter()
        .enumerate()
        .map(|(index, violation)| {
            let description = violation
                .get("message")
                .or_else(|| violation.get("rule"))
                .or_else(|| violation.get("reason"))
                .and_then(Value::as_str)
                .unwrap_or("Governance rule violation")
                .replace(['\n', '\r'], " ");
            ImplementationCriticismFinding {
                id: format!("G{}", index + 1),
                material: true,
                checklist_item_ids: Vec::new(),
                description,
                recommendation:
                    "Revise the plan so every checklist item complies with this governance rule."
                        .to_string(),
            }
        })
        .collect::<Vec<_>>();
    if findings.is_empty() {
        findings.push(ImplementationCriticismFinding {
            id: "G1".to_string(),
            material: true,
            checklist_item_ids: Vec::new(),
            description: precheck
                .message
                .clone()
                .unwrap_or_else(|| "Governance rule violation".to_string())
                .replace(['\n', '\r'], " "),
            recommendation: "Revise the plan so every checklist item complies with governance."
                .to_string(),
        });
    }
    ImplementationCriticism {
        summary: precheck
            .message
            .clone()
            .unwrap_or_else(|| "Plan-stage governance pre-check reported violations.".to_string()),
        findings,
    }
}

pub(super) fn begin_implementation_phase(
    ctx: &WorkflowContext<'_>,
) -> RefineResult<ProposedImplementationPlan> {
    let mut plan = current_plan(ctx)?;
    let final_plan = plan
        .final_plan
        .as_ref()
        .map(|artifact| artifact.result.clone())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {} round {} has no finalized plan",
                ctx.goal_id,
                ctx.round_idx + 1
            ))
        })?;
    begin_phase(ctx, &mut plan, ImplementationPlanPhase::Implement)?;
    Ok(final_plan)
}

/// The plan-phase provider session the implementation launch should resume,
/// when the configured provider supports interactive continuity.
pub(super) fn implementation_resume_session(
    ctx: &WorkflowContext<'_>,
) -> Option<ProviderSessionContinuity> {
    if !HostAgentProviderService::provider_supports_interactive_session_continuity(&ctx.provider) {
        return None;
    }
    current_plan(ctx)
        .ok()
        .and_then(|plan| plan.provider_session_id)
        .map(ProviderSessionContinuity::Resume)
}

pub(super) fn governed_implementation_prompt(
    goal_id: &str,
    agent_context: &Value,
    final_plan: &ProposedImplementationPlan,
) -> RefineResult<String> {
    implementation_prompt(&goal_agent_prompt(goal_id, agent_context)?, final_plan)
}

pub(super) fn complete_implementation_planning(
    ctx: &WorkflowContext<'_>,
    started_at: String,
    report: String,
    evidence: Option<ImplementationExecutionEvidence>,
) -> RefineResult<()> {
    let mut plan = current_plan(ctx)?;
    if plan.phase != ImplementationPlanPhase::Implement || plan.final_plan.is_none() {
        return Err(RefineError::Conflict(format!(
            "Goal {} round {} is not ready to record implementation evidence",
            ctx.goal_id,
            ctx.round_idx + 1
        )));
    }
    let evidence = evidence.or_else(|| {
        (cfg!(test) && ctx.provider == "smoke-ai").then(|| ImplementationExecutionEvidence {
            checklist: plan
                .final_plan
                .as_ref()
                .into_iter()
                .flat_map(|artifact| &artifact.result.checklist)
                .map(|item| crate::model::goal::ImplementationChecklistResult {
                    id: item.id.clone(),
                    outcome: crate::model::goal::ImplementationChecklistOutcome::Completed,
                    evidence: "Completed by the smoke-ai provider fixture".to_string(),
                })
                .collect(),
            verification: Vec::new(),
        })
    });
    let evidence = match evidence.ok_or_else(|| {
        RefineError::Serialization(
            "Goal Agent completion omitted required implementation_evidence".to_string(),
        )
    }) {
        Ok(evidence) => evidence,
        Err(error) => {
            return Err(persist_phase_failure(
                ctx,
                &mut plan,
                ImplementationPlanPhase::Implement,
                "invalid_output",
                error,
            ));
        }
    };
    if let Err(error) = validate_implementation_evidence(&plan, &evidence) {
        return Err(persist_phase_failure(
            ctx,
            &mut plan,
            ImplementationPlanPhase::Implement,
            "invalid_output",
            error,
        ));
    }
    let previous = plan.clone();
    let completed_at = now_timestamp();
    plan.state = ImplementationPlanState::Completed;
    plan.phase_started_at = started_at.clone();
    plan.updated_at = completed_at.clone();
    plan.completed_at = Some(completed_at.clone());
    plan.implementation = Some(ImplementationAgentEvidence {
        started_at,
        completed_at,
        report,
        execution: evidence,
    });
    persist_plan(ctx, Some(&previous), &plan)
}

pub(super) fn fail_implementation_phase(
    ctx: &WorkflowContext<'_>,
    category: &str,
    error: &RefineError,
) -> RefineError {
    if let Ok(mut plan) = current_plan(ctx) {
        return persist_phase_failure(
            ctx,
            &mut plan,
            ImplementationPlanPhase::Implement,
            category,
            RefineError::Conflict(error.to_string()),
        );
    }
    RefineError::Conflict(error.to_string())
}

fn validate_implementation_evidence(
    plan: &ImplementationPlan,
    evidence: &ImplementationExecutionEvidence,
) -> RefineResult<()> {
    let expected = plan
        .final_plan
        .as_ref()
        .expect("validated above")
        .result
        .checklist
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let observed = evidence
        .checklist
        .iter()
        .map(|item| item.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if expected != observed || observed.len() != evidence.checklist.len() {
        return Err(RefineError::Serialization(format!(
            "implementation evidence checklist IDs did not match accepted plan (expected {}, observed {})",
            expected.into_iter().collect::<Vec<_>>().join(", "),
            observed.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(())
}

fn load_or_initialize_plan(
    ctx: &WorkflowContext<'_>,
    goal: &Value,
    agent_context: &Value,
    implementation_branch: &str,
) -> RefineResult<ImplementationPlan> {
    let binding = plan_binding(ctx, goal, agent_context, implementation_branch)?;
    let round = goal
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .ok_or_else(|| {
            RefineError::NotFound(format!("Goal {} has no current round", ctx.goal_id))
        })?;
    if let Some(raw) = round
        .get("implementation_plan")
        .filter(|value| !value.is_null())
    {
        let mut plan: ImplementationPlan = crate::structured_output::decode_persisted(
            raw.clone(),
            "implementation planning evidence",
        )?;
        // A persisted failure never blocks re-entry: the workflow determines
        // what happens, so a Goal re-queued after a planning failure enters
        // this step again and does the work over. The failed plan is replaced
        // wholesale rather than revived piecemeal — a retained governance
        // pre-check verdict would instantly re-settle the Round it already
        // failed, and a retained repair ledger would describe attempts this
        // fresh pass never made.
        if plan.state == ImplementationPlanState::Failed {
            let failure = plan.failure.clone();
            let fresh = initial_plan(binding);
            persist_plan(ctx, Some(&plan), &fresh)?;
            ctx.log(
                "plan",
                "Planning re-entry replaced the failed prior plan; the step is done over",
                Some(json_object(serde_json::json!({
                    "failed_phase": failure.as_ref().map(|failure| failure.phase.clone()),
                    "failed_category": failure.as_ref().map(|failure| failure.category.clone())
                }))),
            )?;
            return Ok(fresh);
        }
        if plan.binding != binding {
            return Err(RefineError::Conflict(format!(
                "Goal {} round {} implementation planning binding changed",
                ctx.goal_id,
                ctx.round_idx + 1
            )));
        }
        if plan.schema_version != IMPLEMENTATION_PLAN_SCHEMA_VERSION {
            let previous = plan.clone();
            plan.schema_version = IMPLEMENTATION_PLAN_SCHEMA_VERSION;
            if plan
                .failure
                .as_ref()
                .is_some_and(|failure| failure.category == "interrupted")
            {
                plan.state = ImplementationPlanState::InProgress;
                plan.failure = None;
            }
            plan.updated_at = now_timestamp();
            persist_plan(ctx, Some(&previous), &plan)?;
        }
        return Ok(plan);
    }
    let plan = initial_plan(binding);
    persist_plan(ctx, None, &plan)?;
    Ok(plan)
}

fn initial_plan(binding: ImplementationPlanBinding) -> ImplementationPlan {
    let now = now_timestamp();
    ImplementationPlan {
        schema_version: IMPLEMENTATION_PLAN_SCHEMA_VERSION,
        state: ImplementationPlanState::InProgress,
        phase: ImplementationPlanPhase::Plan,
        binding,
        started_at: now.clone(),
        phase_started_at: now.clone(),
        updated_at: now,
        completed_at: None,
        proposal: None,
        criticism: None,
        final_plan: None,
        implementation: None,
        failure: None,
        invalid_output_attempts: Vec::new(),
        provider_session_id: None,
        governance_precheck: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_observational_phase_with_repair<T>(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    agent_cwd: &Path,
    branch: &str,
    phase_state: ImplementationPlanPhase,
    phase: &str,
    initial_prompt: String,
    output_label: &str,
    contract_json: &str,
    session: PlanningPhaseSession,
    decode: impl Fn(&str) -> RefineResult<T>,
) -> RefineResult<(PlanningPhaseRun, T)> {
    let plan = std::cell::RefCell::new(plan);
    run_with_repair(
        &RepairPolicy::default(),
        |directive| {
            let prompt = match directive {
                None => initial_prompt.clone(),
                Some(directive) => {
                    repair_prompt(&initial_prompt, output_label, contract_json, directive)
                }
            };
            let provider_session = session.for_invocation(directive.is_some());
            let mut plan = plan.borrow_mut();
            run_observational_phase(
                ctx,
                &mut plan,
                agent_cwd,
                branch,
                phase_state.clone(),
                phase,
                prompt,
                provider_session,
            )
        },
        |run| run.output.as_str(),
        &decode,
        |run, outcome| {
            let Some(diagnostics) = outcome.diagnostics else {
                return Ok(());
            };
            ctx.revalidate_authority(crate::model::workflow::GoalStatus::Plan)?;
            let mut plan = plan.borrow_mut();
            let plan = &mut **plan;
            let previous = plan.clone();
            plan.invalid_output_attempts
                .push(ImplementationPlanningOutputAttempt {
                    phase: phase_state.clone(),
                    attempt: outcome.attempt,
                    raw_output: outcome.raw_output.to_string(),
                    diagnostics: diagnostics.to_string(),
                    recorded_at: now_timestamp(),
                });
            plan.updated_at = now_timestamp();
            persist_plan(ctx, Some(&previous), plan)?;
            if outcome.exhausted
                && let Err(persistence) = record_failure(
                    ctx,
                    plan,
                    ImplementationPlanningFailure {
                        phase: phase_state.clone(),
                        category: "invalid_output_repair_exhausted".to_string(),
                        message: diagnostics.to_string(),
                        failed_at: now_timestamp(),
                        git_before: Some(run.git_before.clone()),
                        git_after: Some(run.git_after.clone()),
                    },
                )
            {
                return Err(RefineError::Conflict(format!(
                    "{diagnostics}; additionally failed to persist implementation planning failure evidence: {persistence}"
                )));
            }
            Ok(())
        },
    )
}

fn plan_binding(
    ctx: &WorkflowContext<'_>,
    goal: &Value,
    agent_context: &Value,
    implementation_branch: &str,
) -> RefineResult<ImplementationPlanBinding> {
    let required = |key: &str| {
        goal.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| {
                RefineError::Conflict(format!("Goal {} has no pinned {key}", ctx.goal_id))
            })
    };
    let encoded = serde_json::to_vec(agent_context).map_err(|error| {
        RefineError::Serialization(format!("failed to encode pinned agent context: {error}"))
    })?;
    Ok(ImplementationPlanBinding {
        goal_id: ctx.goal_id.clone(),
        round_idx: ctx.round_idx,
        context_version: agent_context
            .get("version")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        context_digest: format!("{:x}", Sha256::digest(encoded)),
        implementation_branch: implementation_branch.to_string(),
        target_branch: required("target_branch")?,
        base_commit: required("base_commit")?,
    })
}

fn begin_phase(
    ctx: &WorkflowContext<'_>,
    plan: &mut ImplementationPlan,
    phase: ImplementationPlanPhase,
) -> RefineResult<()> {
    if plan.phase == phase {
        return Ok(());
    }
    let previous = plan.clone();
    let now = now_timestamp();
    plan.phase = phase;
    plan.state = ImplementationPlanState::InProgress;
    plan.phase_started_at = now.clone();
    plan.updated_at = now;
    plan.failure = None;
    persist_plan(ctx, Some(&previous), plan)
}

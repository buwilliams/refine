use super::*;
use crate::application::events::{FileEventService, InvocationContext};
use crate::model::automation::{BindingMode, SkillResult};
use std::collections::BTreeMap;

impl FileQualityService {
    pub(super) fn run_skill_checks(
        &self,
        request: QualityCheckRequest,
    ) -> RefineResult<QualityCheckResult> {
        self.ensure_operation_active(&request, "Skill Quality evaluation")?;
        let runtime = self
            .runtime_root
            .as_ref()
            .ok_or_else(|| RefineError::Degraded("Quality requires a runtime".into()))?;
        let root = PathBuf::from(&request.cwd);
        let _workspace = crate::infrastructure::storage::workspace::WorkspaceLease::acquire(&root)?;
        verify_candidate(&root, &request.candidate_commit, "before Skill checks")?;
        let detail =
            FileWorkItemService::new(&self.refine_dir).show_goal_detail(&request.owner_id)?;
        let round = detail
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|r| r.get(request.round_idx))
            .ok_or_else(|| RefineError::Conflict("Quality Round changed".into()))?;
        let events = FileEventService::with_runtime_root(&self.refine_dir, runtime);
        let config = events.gate_configuration(
            &request.owner_id,
            request.round_idx,
            &request.node_id,
            "workflow.quality.enter",
            || self.ensure_operation_active(&request, "Quality requirements"),
        )?;
        let target_root = request
            .process_metadata
            .get("target_app_id")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| root.clone());
        let context = InvocationContext {
            node_id: request.node_id.clone(),
            target_root: target_root.clone(),
            cwd: root.clone(),
            workspace: Some(
                serde_json::from_value(
                    request
                        .process_metadata
                        .get("managed_worktree")
                        .cloned()
                        .ok_or_else(|| {
                            RefineError::Degraded("Quality has no admitted workspace".into())
                        })?,
                )
                .map_err(|e| {
                    RefineError::Serialization(format!("invalid Quality workspace: {e}"))
                })?,
            ),
            lifecycle: None,
            provider: request.provider.clone(),
            goal_id: Some(request.owner_id.clone()),
            round_idx: Some(request.round_idx),
            workflow_revision: request
                .process_metadata
                .get("workflow_revision")
                .and_then(Value::as_u64),
            candidate_commit: Some(request.candidate_commit.clone()),
            data: json!({"goal":crate::application::events::execution::goal_context(&detail),
                "system":{"node_id":request.node_id,"candidate_commit":request.candidate_commit,"workspace":root,"project_root":target_root,"workflow_step":"quality"},"verification_only":true}),
            metadata: request.process_metadata.clone(),
        };
        let (required, snapshot) = super::skill_evidence::requirements(&events, &config, &context)?;
        crate::infrastructure::process::supervisor::coordination::with_record_lock(
            &self.refine_dir,
            &request.owner_id,
            || {
                self.ensure_operation_active(&request, "pin Quality requirements")?;
                FileWorkItemService::for_node(&self.refine_dir, &request.node_id)
                    .update_goal_round_evaluation_summary(
                        &request.owner_id,
                        request.round_idx,
                        &json!({"quality_requirements":snapshot}),
                    )?;
                Ok(())
            },
        )?;
        if required.is_empty() {
            verify_candidate(
                &root,
                &request.candidate_commit,
                "after optional Quality gate",
            )?;
            self.ensure_operation_active(&request, "optional Quality settlement")?;
            return Ok(QualityCheckResult {
                owner_id: request.owner_id,
                ok: true,
                summary: "No required Quality Skills apply; passed without agent checks.".into(),
                results: Vec::new(),
                diagnostics: vec![format!(
                    "No required bindings in Skill requirements {}.",
                    snapshot["id"]
                )],
                candidate_commit: request.candidate_commit,
                checked_at: None,
                provider_attempts: Vec::new(),
                skill_evidence: Some(super::skill_evidence::coverage(&snapshot, &[])),
            });
        }
        let tree =
            crate::infrastructure::git::worktrees::FileGitWorktreeService::with_runtime_root(
                &root, runtime,
            )
            .commit_tree(&request.candidate_commit)?;
        let mut accepted =
            super::skill_evidence::retained_reviews(&events, &required, &context, round, &tree)?;
        let mut invocation_evidence = Vec::new();
        for result in accepted.values() {
            invocation_evidence.push(format!("Reused Skill review {} for final candidate content: automation/invocations/{}.json", result.binding_id, result.invocation_id));
        }
        // Only missing reviews are observationally requested against the finalized candidate.
        for event in config.events.values().filter(|e| {
            e.enabled
                && e.source.as_deref() == Some("workflow.quality.enter")
                && e.scope.applies(&request.node_id)
        }) {
            let missing = required
                .iter()
                .filter(|r| r.event_id == event.id && !accepted.contains_key(&r.key()))
                .map(|r| r.binding.binding.id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            if missing.is_empty() {
                continue;
            }
            let mut proof_event = event.clone();
            let selected = config
                .bindings(event, &request.node_id)
                .into_iter()
                .filter(|(b, _)| b.mode == BindingMode::Context || missing.contains(&b.id))
                .map(|(b, _)| b.id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            proof_event.bindings.retain(|b| selected.contains(&b.id));
            let invocation = events.prepare_pinned(
                &config,
                &proof_event,
                context.clone(),
                BTreeMap::new(),
                &format!(
                    "{}:{}:{}:{}:quality-proof:{}:{}",
                    request.owner_id,
                    request.round_idx,
                    detail["event_generation"].as_u64().unwrap_or(0),
                    request.node_id,
                    request.candidate_commit,
                    snapshot["id"].as_str().unwrap_or_default()
                ),
            )?;
            invocation_evidence.push(format!(
                "Event invocation {}: automation/invocations/{}.json",
                invocation.id, invocation.id
            ));
            let invocation = events.execute_with_metadata(
                &invocation.id,
                Some(&request.process_metadata),
                || self.ensure_operation_active(&request, "Quality Skill result"),
            )?;
            invocation.blocking_results()?;
            invocation.context.validate_workspace(&self.refine_dir)?;
            for id in missing {
                let mut result = invocation.results.get(&id).cloned().ok_or_else(|| {
                    RefineError::Conflict("Required Quality review is missing".into())
                })?;
                result.binding_id = format!("{}:{id}", event.id);
                accepted.insert(result.binding_id.clone(), result);
            }
        }
        let skills = accepted.into_values().collect::<Vec<SkillResult>>();
        let coverage = super::skill_evidence::coverage(&snapshot, &skills);
        if !coverage.covers(&snapshot) {
            return Err(RefineError::Conflict(
                "Quality requirement coverage is incomplete".into(),
            ));
        }
        // The selected agents decide. Optional artifacts are retained by the
        // invocation recorder, never executed or graded by this coordinator.
        let results = skills
            .iter()
            .map(|skill| QualityTestResult {
                test: skill.binding_id.clone(),
                status: if skill.outcome == "success" {
                    "passed"
                } else {
                    "failed"
                }
                .into(),
                evidence: skill.summary.clone(),
                command: String::new(),
                process_id: None,
                exit_code: None,
            })
            .collect::<Vec<_>>();
        let diagnostics = invocation_evidence;
        verify_candidate(&root, &request.candidate_commit, "after Skill checks")?;
        self.ensure_operation_active(&request, "Skill Quality settlement")?;
        let ok = skills.iter().all(|skill| skill.outcome == "success");
        let mut result = QualityCheckResult {
            owner_id: request.owner_id,
            ok,
            summary: if ok {
                "Quality Skills reported success.".into()
            } else {
                String::new()
            },
            results,
            diagnostics,
            candidate_commit: request.candidate_commit,
            checked_at: None,
            provider_attempts: Vec::new(),
            skill_evidence: Some(coverage),
        };
        if !ok {
            result.summary = quality_failure_summary(&result);
        }
        Ok(result)
    }
}

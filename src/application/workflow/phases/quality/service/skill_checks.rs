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
        let mut skills = accepted.into_values().collect::<Vec<SkillResult>>();
        let coverage = super::skill_evidence::coverage(&snapshot, &skills);
        if !coverage.covers(&snapshot) {
            return Err(RefineError::Conflict(
                "Quality requirement coverage is incomplete".into(),
            ));
        }
        // Imported enforced commands remain supervised until the migrated Skill is
        // deliberately edited or replaced. Merely omitting them from a response cannot
        // silently weaken the pre-upgrade gate.
        let archive = self.refine_dir.join("automation/migration-v1.json");
        if archive.exists() {
            let migration: Value = crate::infrastructure::storage::automation::read_json(&archive)?;
            if let Some(skill) = config.skills.get("default-quality").filter(|s| s.enabled)
                && migration["quality_prompt_hash"].as_str()
                    == Some(&crate::application::events::execution::stable_id(
                        &skill.prompt,
                    ))
                && config
                    .events
                    .values()
                    .filter(|e| e.source.as_deref() == Some("workflow.quality.enter"))
                    .any(|e| {
                        config
                            .bindings(e, &request.node_id)
                            .iter()
                            .any(|(b, _)| b.skill_id == skill.id && b.mode == BindingMode::Blocking)
                    })
            {
                let commands = migration["quality_commands"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !commands.is_empty() {
                    skills.push(SkillResult { invocation_id: "migration-v1".into(), binding_id: "imported-quality-commands".into(), role: "quality".into(), outcome: "success".into(), summary: "Preserved enforced checks".into(), evidence: vec![], artifacts: json!({"tests": commands.iter().filter_map(Value::as_str).map(|command| json!({"test": command, "command": command, "status":"pending", "evidence":""})).collect::<Vec<_>>()}) });
                }
            }
        }
        let mut results = Vec::new();
        let mut diagnostics = invocation_evidence;
        for skill in skills {
            if skill.outcome == "error" {
                return Err(RefineError::Degraded(skill.summary));
            }
            if skill.outcome != "success" {
                results.push(QualityTestResult {
                    test: skill.binding_id,
                    status: "failed".into(),
                    evidence: skill.summary,
                    command: String::new(),
                    process_id: None,
                    exit_code: None,
                });
                continue;
            }
            let tests = skill
                .artifacts
                .get("tests")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    RefineError::InvalidInput(
                        "Quality Skill must return supervised test commands".into(),
                    )
                })?;
            if tests.is_empty() {
                return Err(RefineError::InvalidInput(
                    "Quality success requires at least one observed check".into(),
                ));
            }
            for test in tests {
                let mut result: QualityTestResult = serde_json::from_value(test.clone())
                    .map_err(|e| RefineError::InvalidInput(format!("Quality Skill test: {e}")))?;
                if result.command.trim().is_empty() || result.test.trim().is_empty() {
                    return Err(RefineError::InvalidInput(
                        "Quality tests require a name and noninteractive command".into(),
                    ));
                }
                result.test = format!("{}: {}", skill.binding_id, result.test);
                self.ensure_operation_active(&request, "next Skill check")?;
                let mut metadata = request.process_metadata.clone();
                metadata.insert("quality_test".into(), json!(result.test));
                metadata.insert("quality_command".into(), json!(result.command));
                let observed = self.run_observed_command(&result.command, &root, metadata)?;
                if observed.shell_parser_aborted() {
                    return Err(quality_command_harness_fault(&result.command, &observed));
                }
                result.status = if observed.exit_code == Some(0) {
                    "passed"
                } else {
                    "failed"
                }
                .into();
                result.evidence = observed.evidence();
                result.process_id = Some(observed.process_id);
                result.exit_code = observed.exit_code;
                diagnostics.push(result.evidence.clone());
                results.push(result);
            }
        }
        verify_candidate(&root, &request.candidate_commit, "after Skill checks")?;
        self.ensure_operation_active(&request, "Skill Quality settlement")?;
        let ok = !results.is_empty() && results.iter().all(|r| r.status == "passed");
        let mut result = QualityCheckResult {
            owner_id: request.owner_id,
            ok,
            summary: "All Quality Skills passed with supervised evidence.".into(),
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

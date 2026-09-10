//! Select already accepted reviews by pinned requirements and observed final content.
use super::*;
use crate::application::events::execution::{PinnedBinding, stable_id};
use crate::application::events::{EventInvocation, FileEventService, InvocationContext};
use crate::model::automation::{AutomationConfig, BindingMode, SkillResult};
use crate::model::goal::QualitySkillEvidence;
use std::collections::BTreeMap;

pub(super) struct RequiredReview {
    pub event_id: String,
    pub binding: PinnedBinding,
    pub attachments: Vec<PinnedBinding>,
}
impl RequiredReview {
    pub fn key(&self) -> String {
        format!("{}:{}", self.event_id, self.binding.binding.id)
    }
    fn matches(&self, invocation: &EventInvocation) -> bool {
        invocation.event.id == self.event_id
            && invocation.bindings.contains(&self.binding)
            && invocation
                .bindings
                .iter()
                .filter(|b| b.binding.mode == BindingMode::Context)
                .eq(self.attachments.iter())
    }
}

pub(super) fn requirements(
    service: &FileEventService,
    config: &AutomationConfig,
    context: &InvocationContext,
) -> RefineResult<(Vec<RequiredReview>, Value)> {
    let mut reviews = Vec::new();
    let mut selected = Vec::new();
    for event in config.events.values().filter(|e| {
        e.enabled
            && e.source.as_deref() == Some("workflow.quality.enter")
            && e.scope.applies(&context.node_id)
    }) {
        let bindings =
            service.resolve_bindings(config, event, &mut context.clone(), &BTreeMap::new())?;
        let attachments = bindings
            .iter()
            .filter(|b| b.binding.mode == BindingMode::Context)
            .cloned()
            .collect::<Vec<_>>();
        for binding in bindings
            .into_iter()
            .filter(|b| b.binding.mode == BindingMode::Blocking)
        {
            let review = RequiredReview {
                event_id: event.id.clone(),
                binding,
                attachments: attachments.clone(),
            };
            selected.push(json!({"key":review.key(),"binding":review.binding,"attachments":review.attachments,"contract_version":1}));
            reviews.push(review);
        }
    }
    let id = stable_id(&json!(selected).to_string());
    let snapshot = json!({"id":id,"candidate_commit":context.candidate_commit,"node_id":context.node_id,
        "required_bindings":reviews.iter().map(RequiredReview::key).collect::<Vec<_>>(),"selected":selected});
    Ok((reviews, snapshot))
}

pub(super) fn retained_reviews(
    service: &FileEventService,
    required: &[RequiredReview],
    context: &InvocationContext,
    round: &Value,
    tree: &str,
) -> RefineResult<BTreeMap<String, SkillResult>> {
    let mut accepted = BTreeMap::new();
    let Some(records) = round["event_results"].as_object() else {
        return Ok(accepted);
    };
    for (id, record) in records {
        if record["source"] != "workflow.quality.enter" {
            continue;
        }
        let invocation = match service.invocation(id) {
            Ok(value) => value,
            Err(RefineError::NotFound(_)) => continue,
            Err(error) => return Err(error),
        };
        if invocation.context.goal_id != context.goal_id
            || invocation.context.round_idx != context.round_idx
            || invocation.context.node_id != context.node_id
            || invocation.context.data["goal"]["event_generation"]
                != context.data["goal"]["event_generation"]
        {
            continue;
        }
        for requirement in required.iter().filter(|r| r.matches(&invocation)) {
            let Some(result) = invocation.results.get(&requirement.binding.binding.id) else {
                continue;
            };
            // Legacy failed findings already attached to this finalized candidate remain findings.
            // Successful legacy reports with no content observation cannot establish coverage.
            let legacy_finding = result.outcome == "failure"
                && round["quality_candidate_commit"] == json!(context.candidate_commit);
            let observed = crate::application::events::completion::accepted_tree(
                &invocation,
                &requirement.binding.binding.id,
            );
            if observed.as_deref() != Some(tree) && !legacy_finding {
                continue;
            }
            if !matches!(result.outcome.as_str(), "success" | "failure") {
                continue;
            }
            invocation.blocking_results()?;
            invocation.context.validate_workspace(&service.refine_dir)?;
            let key = requirement.key();
            // Never erase a valid failed finding with a later success for the same candidate.
            if accepted
                .get(&key)
                .is_some_and(|r: &SkillResult| r.outcome == "failure")
            {
                continue;
            }
            let mut result = result.clone();
            result.binding_id = key.clone();
            accepted.insert(key, result);
        }
    }
    Ok(accepted)
}

pub(super) fn coverage(snapshot: &Value, skills: &[SkillResult]) -> QualitySkillEvidence {
    QualitySkillEvidence {
        requirements_id: snapshot["id"].as_str().unwrap_or_default().into(),
        required_bindings: serde_json::from_value(snapshot["required_bindings"].clone())
            .unwrap_or_default(),
        invocations: skills
            .iter()
            .map(|r| (r.binding_id.clone(), r.invocation_id.clone()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::events::InvocationState;
    #[test]
    fn only_matching_final_content_covers_requirements_and_failed_findings_are_retained() {
        let temp =
            std::env::temp_dir().join(format!("refine-review-evidence-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["commit", "--allow-empty", "-qm", "base"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .current_dir(&temp)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let git = FileGitWorktreeService::new(&temp);
        let commit = git.resolve_commit("HEAD").unwrap();
        let branch = "refine/REVIEW/round-1";
        let checkout = git.managed_worktree_path(branch).unwrap();
        git.ensure_worktree_from_base(branch, &checkout, &commit)
            .unwrap();
        let service = FileEventService::new(temp.join("state"));
        let work = FileWorkItemService::new(&service.refine_dir);
        work.create_goal_summary("Review", Some("REVIEW")).unwrap();
        work.append_goal_round_summary("REVIEW", "test", "Check candidate")
            .unwrap();
        work.update_goal_git_refs("REVIEW", branch, "main", &commit, Some(&commit))
            .unwrap();
        let config = service.config().unwrap();
        let context = InvocationContext {
            node_id: "default".into(),
            target_root: temp.clone(),
            cwd: checkout.clone(),
            workspace: None,
            lifecycle: None,
            provider: "smoke-ai".into(),
            goal_id: Some("REVIEW".into()),
            round_idx: Some(0),
            workflow_revision: Some(7),
            candidate_commit: Some("FINAL".into()),
            data: json!({"goal":{"event_generation":3}}),
            metadata: Default::default(),
        };
        let (required, snapshot) = requirements(&service, &config, &context).unwrap();
        let mut unrelated = (*config).clone();
        unrelated.revision += 1;
        unrelated
            .skills
            .get_mut("default-governance")
            .unwrap()
            .prompt = "Unrelated edit".into();
        assert_eq!(
            requirements(&service, &unrelated, &context).unwrap().1["id"],
            snapshot["id"]
        );
        let mut round = json!({"event_results":{}});
        for (key, tree, outcome) in [
            ("earlier-review", "BEFORE", "success"),
            ("last-review", "FINAL-TREE", "success"),
            ("finding", "FINAL-TREE", "failure"),
        ] {
            let mut invocation = service
                .prepare_pinned(
                    &config,
                    &config.events["workflow.quality.enter"],
                    context.clone(),
                    BTreeMap::new(),
                    key,
                )
                .unwrap();
            let binding = &required[0].binding;
            invocation.results.insert(
                binding.binding.id.clone(),
                SkillResult {
                    invocation_id: invocation.id.clone(),
                    binding_id: binding.binding.id.clone(),
                    role: "quality".into(),
                    outcome: outcome.into(),
                    summary: outcome.into(),
                    evidence: vec!["Observed check".into()],
                    artifacts: json!({"tests":[{"test":"observed", "command":"true", "status":"passed", "evidence":"observed"}]}),
                },
            );
            invocation.attempts.push(json!({"binding_id":binding.binding.id,"attempt":0,"process_id":"managed-review","raw_output":"retained",
                "checkout":{"observation":{"head_commit":"SOURCE","branch":"candidate","status_porcelain":[]},"tree":tree}}));
            invocation.state = if outcome == "failure" {
                InvocationState::Failed
            } else {
                InvocationState::Succeeded
            };
            service.save_invocation(&invocation).unwrap();
            round["event_results"][&invocation.id] = json!({"source":"workflow.quality.enter"});
            let accepted =
                retained_reviews(&service, &required, &context, &round, "FINAL-TREE").unwrap();
            if key == "earlier-review" {
                assert!(accepted.is_empty());
            } else {
                assert_eq!(accepted[&required[0].key()].outcome, outcome);
            }
        }
        let accepted =
            retained_reviews(&service, &required, &context, &round, "FINAL-TREE").unwrap();
        let evidence = coverage(&snapshot, &accepted.into_values().collect::<Vec<_>>());
        assert!(evidence.covers(&snapshot));
        let mut changed = (*config).clone();
        changed.skills.get_mut("default-quality").unwrap().prompt = "Different requirement".into();
        let (changed_required, changed_snapshot) =
            requirements(&service, &changed, &context).unwrap();
        assert!(!evidence.covers(&changed_snapshot));
        assert!(
            retained_reviews(&service, &changed_required, &context, &round, "FINAL-TREE")
                .unwrap()
                .is_empty()
        );
        let mut next_round = context.clone();
        next_round.round_idx = Some(1);
        assert!(
            retained_reviews(&service, &required, &next_round, &round, "FINAL-TREE")
                .unwrap()
                .is_empty()
        );
        std::fs::rename(&checkout, temp.join("retained-checkout")).unwrap();
        let error =
            retained_reviews(&service, &required, &context, &round, "FINAL-TREE").unwrap_err();
        assert!(
            error.to_string().contains("managed workspace unavailable"),
            "{error}"
        );
        std::fs::remove_dir_all(temp).unwrap();
    }
}

//! Distinguish completed Skill decisions from unfinished work whose owner was lost.
//! Reuse receipts and keep unfinished occurrences schedulable in their current step.
use crate::application::events::FileEventService;
use crate::application::work_items::WorkflowStepAuthority;
use crate::error::RefineResult;
use serde_json::Value;

#[derive(Default)]
pub(super) struct RestartEvidence {
    pub completed: bool,
    pub started: bool,
}

pub(super) fn step_receipts(
    events: &FileEventService,
    goal: &Value,
    authority: WorkflowStepAuthority,
) -> RefineResult<RestartEvidence> {
    let id = goal["id"].as_str().unwrap_or_default();
    let source = format!(
        "workflow.{}.enter",
        goal["status"].as_str().unwrap_or_default()
    );
    let mut evidence = RestartEvidence::default();
    let mut incomplete = false;
    let mut offset = 0;
    loop {
        let history = events.goal_invocations(id, offset, 100)?;
        let Some(items) = history["items"].as_array() else {
            return Ok(evidence);
        };
        for item in items {
            let Some(id) = item["id"].as_str() else {
                continue;
            };
            let invocation = events.invocation(id)?;
            if invocation.context.round_idx != Some(authority.round_idx)
                || invocation.context.data["goal"]["event_generation"]
                    .as_u64()
                    .unwrap_or(0)
                    != authority.generation
                || invocation.context.node_id != goal["node_id"].as_str().unwrap_or("default")
            {
                continue;
            }
            use crate::application::events::InvocationState;
            use crate::model::automation::BindingMode;
            let required = invocation
                .bindings
                .iter()
                .filter(|b| b.binding.mode == BindingMode::Blocking)
                .collect::<Vec<_>>();
            if required.is_empty() {
                continue;
            }
            evidence.started |= invocation.state != InvocationState::Pending
                || !invocation.attempts.is_empty()
                || invocation.context.metadata.contains_key("started_bindings")
                || invocation
                    .context
                    .metadata
                    .contains_key("event_operation_id");
            let accepted = invocation.gate_assessment()
                == crate::application::workflow::gates::GateAssessment::Satisfied;
            let completed_receipts = matches!(
                invocation.state,
                InvocationState::Pending | InvocationState::Running
            ) && required.iter().all(|binding| {
                invocation.attempts.iter().any(|receipt| {
                    receipt["binding_id"] == binding.binding.id && receipt["raw_output"].is_string()
                })
            });
            if accepted || completed_receipts {
                if invocation.event.source.as_deref() == Some(&source) {
                    evidence.completed = true;
                }
            } else {
                incomplete = true;
            }
        }
        offset += items.len();
        if items.is_empty() || offset >= history["total"].as_u64().unwrap_or(0) as usize {
            break;
        }
    }
    evidence.completed &= !incomplete;
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::events::{InvocationContext, InvocationState};
    use crate::application::work_items::FileWorkItemService;
    use crate::application::workflow::WorkflowEngine;
    use crate::model::workflow::GoalStatus;
    use serde_json::json;

    #[test]
    fn restart_preserves_completed_occurrence_and_keeps_unfinished_work_schedulable() {
        for completed in [false, true] {
            let root =
                std::env::temp_dir().join(format!("refine-step-restart-{}", uuid::Uuid::new_v4()));
            let target = root.join("target");
            std::fs::create_dir_all(&target).unwrap();
            for args in [
                vec!["init", "-b", "main"],
                vec!["config", "user.name", "Test"],
                vec!["config", "user.email", "test@example.invalid"],
                vec!["commit", "--allow-empty", "-m", "base"],
            ] {
                assert!(
                    std::process::Command::new("git")
                        .args(args)
                        .current_dir(&target)
                        .output()
                        .unwrap()
                        .status
                        .success()
                );
            }
            let items = FileWorkItemService::new(
                crate::infrastructure::storage::project_layout::prepare_refine_dir(&target)
                    .unwrap(),
            );
            items.create_goal_summary("Restart", Some("GOAL1")).unwrap();
            items
                .append_goal_round_summary("GOAL1", "Reporter", "Retain completed work")
                .unwrap();
            items
                .set_goal_status_unchecked("GOAL1", &GoalStatus::Plan)
                .unwrap();
            use crate::infrastructure::git::worktrees::FileGitWorktreeService;
            let git = FileGitWorktreeService::new(&target);
            let branch = "refine/GOAL1/round-1";
            let base = git.resolve_commit("main").unwrap();
            let worktree = git
                .ensure_worktree_from_base(
                    branch,
                    &git.managed_worktree_path(branch).unwrap(),
                    &base,
                )
                .unwrap();
            items
                .update_goal_git_refs("GOAL1", branch, "main", &base, None)
                .unwrap();
            let goal = items.show_goal_detail("GOAL1").unwrap();
            let events =
                FileEventService::with_runtime_root(&items.refine_dir, root.join("runtime"));
            let mut invocation = events
                .prepare(
                    "workflow.plan.enter",
                    InvocationContext {
                        node_id: "default".into(),
                        target_root: target.clone(),
                        cwd: worktree.into(),
                        workspace: None,
                        lifecycle: None,
                        provider: "smoke-ai".into(),
                        goal_id: Some("GOAL1".into()),
                        round_idx: Some(0),
                        workflow_revision: goal["workflow_revision"].as_u64(),
                        candidate_commit: None,
                        data: json!({"goal":goal}),
                        metadata: Default::default(),
                    },
                    Default::default(),
                    "restart-plan",
                )
                .unwrap();
            invocation.state = InvocationState::Running;
            invocation.context.metadata.insert(
                "started_bindings".into(),
                json!({invocation.bindings[0].binding.id.clone():"started"}),
            );
            if completed {
                for binding in &invocation.bindings {
                    let mut result: crate::model::automation::SkillResult = serde_json::from_value(
                        crate::application::agent_io::contracts::skill_result::result_contract(
                            &invocation.id,
                            &binding.binding.id,
                            &binding.skill.role,
                        ),
                    )
                    .unwrap();
                    result.outcome = "success".into();
                    result.summary = "Completed plan".into();
                    invocation
                        .results
                        .insert(binding.binding.id.clone(), result);
                }
                invocation.state = InvocationState::Succeeded;
            }
            events.save_invocation(&invocation).unwrap();
            let engine = WorkflowEngine::with_target_root(root.join("runtime"), &target);
            assert_eq!(
                engine.recover_interrupted_goals("worker replaced").unwrap(),
                usize::from(!completed)
            );
            assert_eq!(events.invocation(&invocation.id).unwrap(), invocation);
            if completed {
                assert_eq!(items.show_goal_detail("GOAL1").unwrap(), goal);
            } else {
                assert_eq!(
                    items.show_goal_summary("GOAL1").unwrap().goal.status,
                    GoalStatus::Plan
                );
            }
            assert!(!root.join("runtime/agents/processes").exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn claiming_todo_without_executing_does_not_turn_process_replacement_into_failure() {
        let root =
            std::env::temp_dir().join(format!("refine-claim-restart-{}", uuid::Uuid::new_v4()));
        let target = root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        let items = FileWorkItemService::new(target.join(".refine"));
        items.create_goal_summary("Queued", Some("GOAL1")).unwrap();
        items
            .append_goal_round_summary("GOAL1", "Reporter", "Authorized work")
            .unwrap();
        items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        let (round, revision, prompt) = items.authored_goal_commitment("GOAL1").unwrap();
        items
            .claim_workflow_attempt("GOAL1", GoalStatus::Todo, round, revision, &prompt)
            .unwrap();
        let before = items.show_goal_detail("GOAL1").unwrap();
        let engine = WorkflowEngine::with_target_root(root.join("runtime"), &target);
        assert_eq!(
            engine
                .recover_interrupted_goals("worker replaced before launch")
                .unwrap(),
            0
        );
        assert_eq!(items.show_goal_detail("GOAL1").unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }
}

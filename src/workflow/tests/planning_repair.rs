use super::*;
use crate::workflow::context::WorkflowContext;

#[cfg(unix)]
#[test]
fn governed_planning_repairs_invalid_proposal_and_retains_the_attempt() {
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir("governed-planning-repair");
    let target_root = temp_root.join("repo");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&target_root).unwrap();
    git(&target_root, &["init", "-b", "main"]).unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    fs::write(target_root.join("app.txt"), "base\n").unwrap();
    git(&target_root, &["add", "app.txt"]).unwrap();
    git(&target_root, &["commit", "-m", "base"]).unwrap();
    let base = git_output(&target_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    let branch = "refine/GOAL1/round-1";
    git(&target_root, &["checkout", "-b", branch]).unwrap();

    fs::write(
        &smoke_ai,
        r#"#!/bin/sh
case "$*" in
  *"Structured Output Repair 1/2"*)
    payload='{"state":"completed","message":"repaired","guidance_applied":[],"planning_result":{"summary":"Repair the workflow output contract.","checklist":[{"id":"P1","description":"Implement the bounded repair behavior."}]}}'
    ;;
  *"Current Workflow Phase: Criticize"*)
    payload='{"state":"completed","message":"criticized","guidance_applied":[],"planning_result":{"summary":"No material omissions.","findings":[]}}'
    ;;
  *"Current Workflow Phase: Revise"*)
    payload='{"state":"completed","message":"revised","guidance_applied":[],"planning_result":{"summary":"Repair the workflow output contract.","checklist":[{"id":"P1","description":"Implement the bounded repair behavior."}],"criticism_resolutions":[]}}'
    ;;
  *)
    payload='{"state":"completed","message":"invalid proposal","guidance_applied":[],"planning_result":{"summary":"Missing the checklist."}}'
    ;;
esac
printf '%s\n' "$payload" > "$REFINE_AGENT_SIGNAL_PATH"
sleep 10
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&smoke_ai, permissions).unwrap();

    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_provider = std::env::var_os("REFINE_SMOKE_AI_PATH");
    let previous_planning = std::env::var_os("REFINE_SMOKE_AI_GOVERNED_PLANNING");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &smoke_ai);
        std::env::set_var("REFINE_SMOKE_AI_GOVERNED_PLANNING", "1");
    }

    let refine_dir = test_refine_dir(&target_root);
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Planning repair", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Implement bounded repair")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    work_items
        .update_goal_git_refs("GOAL1", branch, "main", &base, None)
        .unwrap();
    let agent_context = json!({
        "version": 1,
        "goal": {"id": "GOAL1", "name": "Planning repair", "node_id": "default"},
        "previous_rounds": [],
        "current_round": {"round": 1, "prompt": "Implement bounded repair"}
    });
    work_items
        .update_goal_round_evaluation_summary("GOAL1", 0, &json!({"agent_context": agent_context}))
        .unwrap();
    let (round_idx, revision, request) = work_items.authored_goal_commitment("GOAL1").unwrap();
    let authority = work_items
        .claim_workflow_attempt("GOAL1", GoalStatus::Plan, round_idx, revision, &request)
        .unwrap();
    let context = WorkflowContext::new(
        &runtime_root,
        &target_root,
        "GOAL1".to_string(),
        "default".to_string(),
        "smoke-ai".to_string(),
        round_idx,
        authority,
        Default::default(),
        work_items.clone(),
    );
    let goal = work_items.show_goal_detail("GOAL1").unwrap();
    let plan =
        run_governed_implementation_planning(&context, &goal, &agent_context, &target_root, branch)
            .unwrap();
    assert_eq!(plan.checklist[0].id, "P1");

    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    let attempts = detail["rounds"][0]["implementation_plan"]["invalid_output_attempts"]
        .as_array()
        .unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["phase"], "plan");
    assert_eq!(attempts[0]["attempt"], 1);
    assert!(
        attempts[0]["diagnostics"]
            .as_str()
            .unwrap()
            .contains("structured implementation plan JSON")
    );
    assert_eq!(
        attempts[0]["raw_output"],
        r#"{"summary":"Missing the checklist."}"#
    );

    unsafe {
        if let Some(previous) = previous_provider {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
        if let Some(previous) = previous_planning {
            std::env::set_var("REFINE_SMOKE_AI_GOVERNED_PLANNING", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_GOVERNED_PLANNING");
        }
    }
    fs::remove_dir_all(temp_root).unwrap();
}

use super::*;
use crate::model::goal::ProposedImplementationPlan;
use crate::workflow::context::WorkflowContext;

#[cfg(unix)]
fn run_governed_planning_with_script(
    prefix: &str,
    script_body: &str,
) -> (RefineResult<ProposedImplementationPlan>, serde_json::Value) {
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir(prefix);
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
        format!(
            "#!/bin/sh\n{script_body}\nprintf '%s\\n' \"$payload\" > \"$REFINE_AGENT_SIGNAL_PATH\"\nsleep 10\n"
        ),
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
    let result =
        run_governed_implementation_planning(&context, &goal, &agent_context, &target_root, branch);
    let detail = work_items.show_goal_detail("GOAL1").unwrap();

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
    (result, detail)
}

#[cfg(unix)]
#[test]
fn governed_planning_repairs_invalid_proposal_and_retains_the_attempt() {
    let (result, detail) = run_governed_planning_with_script(
        "governed-planning-repair",
        r#"case "$*" in
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
esac"#,
    );
    let plan = result.unwrap();
    assert_eq!(plan.checklist[0].id, "P1");

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
}

#[cfg(unix)]
#[test]
fn governed_revision_accepts_aliased_resolution_ids_without_a_repair_round() {
    let (result, detail) = run_governed_planning_with_script(
        "governed-revision-alias",
        r#"case "$*" in
  *"Current Workflow Phase: Criticize"*)
    payload='{"state":"completed","message":"criticized","guidance_applied":[],"planning_result":{"summary":"One material omission.","findings":[{"id":"C1","material":true,"checklist_item_ids":["P1"],"description":"Missing failure-path coverage.","recommendation":"Cover the failure path."}]}}'
    ;;
  *"Current Workflow Phase: Revise"*)
    payload='{"state":"completed","message":"revised","guidance_applied":[],"planning_result":{"summary":"Cover the failure path.","checklist":[{"id":"P1","description":"Implement the behavior and its failure path."}],"criticism_resolutions":[{"id":"C1","resolution":"Extended P1 to cover the failure path."}]}}'
    ;;
  *)
    payload='{"state":"completed","message":"planned","guidance_applied":[],"planning_result":{"summary":"Implement the behavior.","checklist":[{"id":"P1","description":"Implement the behavior."}]}}'
    ;;
esac"#,
    );
    // The production transcript shape from goal DRE2A015047036C9717F9959E7: the
    // revise agent resolves finding C1 under the key `id`, not `criticism_id`.
    let plan = result.unwrap();
    assert_eq!(plan.criticism_resolutions.len(), 1);
    assert_eq!(plan.criticism_resolutions[0].criticism_id, "C1");
    assert_eq!(
        plan.criticism_resolutions[0].resolution,
        "Extended P1 to cover the failure path."
    );

    let attempts = &detail["rounds"][0]["implementation_plan"]["invalid_output_attempts"];
    assert!(
        attempts.as_array().is_none_or(Vec::is_empty),
        "attempts: {attempts:?}"
    );
}

#[cfg(unix)]
#[test]
fn governed_planning_repair_diagnostics_name_ambiguity_and_schema_paths() {
    let (result, detail) = run_governed_planning_with_script(
        "governed-planning-diagnostics",
        r#"case "$*" in
  *"Current Workflow Phase: Plan"*"Structured Output Repair 1/2"*)
    payload='{"state":"completed","message":"repaired plan","guidance_applied":[],"planning_result":{"summary":"Implement the behavior.","checklist":[{"id":"P1","description":"Implement the behavior."}]}}'
    ;;
  *"Current Workflow Phase: Revise"*"Structured Output Repair 1/2"*)
    payload='{"state":"completed","message":"repaired revision","guidance_applied":[],"planning_result":{"summary":"Cover the failure path.","checklist":[{"id":"P1","description":"Implement the behavior and its failure path."}],"criticism_resolutions":[{"criticism_id":"C1","resolution":"Extended P1 to cover the failure path."}]}}'
    ;;
  *"Current Workflow Phase: Criticize"*)
    payload='{"state":"completed","message":"criticized","guidance_applied":[],"planning_result":{"summary":"One material omission.","findings":[{"id":"C1","material":true,"checklist_item_ids":["P1"],"description":"Missing failure-path coverage.","recommendation":"Cover the failure path."}]}}'
    ;;
  *"Current Workflow Phase: Revise"*)
    payload='{"state":"completed","message":"revised","guidance_applied":[],"planning_result":{"summary":"Cover the failure path.","checklist":[{"id":"P1","description":"Implement the behavior and its failure path."}],"criticism_resolutions":[{"criticism_id":"C1"}]}}'
    ;;
  *)
    payload='{"state":"completed","message":"planned","guidance_applied":[],"planning_result":"planned as {\"summary\":\"one\"} or {\"summary\":\"two\"}"}'
    ;;
esac"#,
    );
    let plan = result.unwrap();
    assert_eq!(plan.criticism_resolutions[0].criticism_id, "C1");

    let attempts = detail["rounds"][0]["implementation_plan"]["invalid_output_attempts"]
        .as_array()
        .unwrap();
    assert_eq!(attempts.len(), 2, "attempts: {attempts:?}");

    // Plan attempt 1 stringified prose instead of the required object; the
    // transport fault must be named explicitly.
    assert_eq!(attempts[0]["phase"], "plan");
    let stringified = attempts[0]["diagnostics"].as_str().unwrap();
    assert!(
        stringified.contains("invalid recursively stringified JSON"),
        "diagnostics: {stringified}"
    );

    // Revise attempt 1 dropped the resolution field; the repair prompt must
    // carry the exact schema path instead of a generic sentence.
    assert_eq!(attempts[1]["phase"], "revise");
    let schema_path = attempts[1]["diagnostics"].as_str().unwrap();
    assert!(
        schema_path.contains("criticism_resolutions[0]"),
        "diagnostics: {schema_path}"
    );
    assert!(
        schema_path.contains("missing field `resolution`"),
        "diagnostics: {schema_path}"
    );
}

use super::*;
use crate::application::workflow::engine::context::WorkflowContext;
use crate::model::goal::ProposedImplementationPlan;
use serde_json::Value;

fn native_script(body: &str) -> String {
    format!(
        r#"#!/usr/bin/env python3
import json,sys,pathlib
prompt=' '.join(sys.argv[1:])
result=json.JSONDecoder().raw_decode(prompt.split('Refine completion contract (supplied by the system):\n',1)[1])[0]
{body}
print(json.dumps(result))
"#
    )
}

#[cfg(unix)]
fn run_planning(
    prefix: &str,
    script_body: &str,
    multiple: bool,
    retry: bool,
) -> (RefineResult<ProposedImplementationPlan>, Value) {
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
    let workspace = target_root.join(".git/refine-worktrees/refine-GOAL1-round-1");
    crate::infrastructure::git::worktrees::FileGitWorktreeService::new(&target_root)
        .ensure_worktree_from_base(branch, &workspace, &base)
        .unwrap();
    fs::write(target_root.join("manual.txt"), "untracked human work").unwrap();
    let primary_index = fs::read(target_root.join(".git/index")).unwrap();
    let primary_head = git_output(&target_root, &["rev-parse", "HEAD"]);

    fs::write(&smoke_ai, native_script(script_body)).unwrap();
    let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&smoke_ai, permissions).unwrap();

    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_provider = std::env::var_os("REFINE_SMOKE_AI_PATH");

    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &smoke_ai);
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
    let mut context = WorkflowContext::new(
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
    context.branch = Some(branch.into());
    context.worktree_path = Some(workspace.display().to_string());
    let events =
        crate::application::events::FileEventService::with_runtime_root(&refine_dir, &runtime_root);
    if multiple {
        let config = events.config().unwrap();
        let mut skill = config.skills["default-plan"].clone();
        skill.id = "independent-plan".into();
        skill.name = "Independent plan".into();
        events
            .save(
                "skills",
                &skill.id,
                json!({"revision":config.revision,"item":skill,
            "trigger":{"source":"workflow.plan.enter","order":1}}),
            )
            .unwrap();
    }
    let goal = work_items.show_goal_detail("GOAL1").unwrap();
    let mut result =
        run_governed_implementation_planning(&context, &goal, &agent_context, &workspace, branch);

    if retry {
        let error = result.unwrap_err();
        assert_eq!(
            work_items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["implementation_plan"]["state"],
            "failed"
        );
        let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        assert_eq!(
            workflow.settle_goal_failure("GOAL1", authority, "plan", &error),
            crate::application::work_items::FailureSettlement::AuthoritativeFailure
        );
        let settled = work_items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(settled["status"], "failed");
        assert_eq!(settled["rounds"][0]["failure_category"], "plan");
        assert_eq!(settled["rounds"][0]["failure_message"], error.to_string());
        work_items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
            .unwrap();
        fs::write(&smoke_ai, native_script("")).unwrap();
        let (round_idx, revision, request) = work_items.authored_goal_commitment("GOAL1").unwrap();
        let authority = work_items
            .claim_workflow_attempt("GOAL1", GoalStatus::Plan, round_idx, revision, &request)
            .unwrap();
        let mut context = WorkflowContext::new(
            &runtime_root,
            &target_root,
            "GOAL1".into(),
            "default".into(),
            "smoke-ai".into(),
            round_idx,
            authority,
            Default::default(),
            work_items.clone(),
        );
        context.branch = Some(branch.into());
        context.worktree_path = Some(workspace.display().to_string());
        let goal = work_items.show_goal_detail("GOAL1").unwrap();
        result = run_governed_implementation_planning(
            &context,
            &goal,
            &agent_context,
            &workspace,
            branch,
        );
    }
    let mut detail = work_items.show_goal_detail("GOAL1").unwrap();
    let history = events.invocations(0, 100).unwrap();
    detail["invocations"] = json!(
        history["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| events.invocation(i["id"].as_str().unwrap()).unwrap())
            .collect::<Vec<_>>()
    );
    detail["file"] = json!(fs::read_to_string(workspace.join("app.txt")).unwrap());
    unsafe {
        if let Some(previous) = previous_provider {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    assert_eq!(
        primary_index,
        fs::read(target_root.join(".git/index")).unwrap()
    );
    assert_eq!(
        primary_head,
        git_output(&target_root, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        fs::read_to_string(target_root.join("manual.txt")).unwrap(),
        "untracked human work"
    );
    assert_eq!(
        fs::read_to_string(target_root.join("app.txt")).unwrap(),
        "base\n"
    );
    fs::remove_dir_all(temp_root).unwrap();
    (result, detail)
}

#[cfg(unix)]
#[test]
fn plan_skill_repairs_invalid_artifact_and_retains_diagnostics() {
    let (result, detail) = run_planning(
        "event-plan-repair",
        "if not prompt.startswith('Repair only'): result['artifacts']['plan']={'summary':'Missing checklist'}",
        false,
        false,
    );
    assert_eq!(
        result.unwrap().checklist[0].id,
        "workflow.plan.enter:default-plan:P1"
    );
    let attempts = detail["invocations"][0]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert!(
        attempts[0]["diagnostic"]
            .as_str()
            .unwrap()
            .contains("checklist")
    );
    assert!(
        attempts[0]["raw_output"]
            .as_str()
            .unwrap()
            .contains("Missing checklist")
    );
    assert!(attempts[1]["diagnostic"].is_null());
}

#[cfg(unix)]
#[test]
fn plan_skill_exhausts_bounded_repairs_without_accepting_an_invalid_plan() {
    let (result, detail) = run_planning(
        "event-plan-exhausted",
        "result['artifacts']['plan']={'summary':'Missing checklist'}",
        false,
        false,
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("missing field `checklist`")
    );
    assert_eq!(
        detail["invocations"][0]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        detail["rounds"][0]["implementation_plan"]["failure"]["category"],
        "invalid_output"
    );
}

#[cfg(unix)]
#[test]
fn multiple_plan_skills_contribute_separate_namespaced_checklists() {
    let (result, detail) = run_planning("event-multiple-plans", "", true, false);
    let plan = result.unwrap();
    assert_eq!(plan.checklist.len(), 2);
    assert_ne!(plan.checklist[0].id, plan.checklist[1].id);
    assert_eq!(
        detail["invocations"][0]["results"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    assert!(detail["rounds"][0]["implementation_plan"]["criticism"].is_null());
}

#[cfg(unix)]
#[test]
fn a_requeued_goal_re_enters_planning_and_retains_prior_event_failure() {
    let (result, detail) = run_planning(
        "event-requeue-plan",
        "result['artifacts']['plan']={}",
        false,
        true,
    );
    assert_eq!(result.unwrap().checklist.len(), 1);
    let history = detail["invocations"].as_array().unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().any(|i| i["state"] == "error"));
    assert!(history.iter().any(|i| i["state"] == "succeeded"));
    assert!(detail["rounds"][0]["implementation_plan"]["failure"].is_null());
}

#[cfg(unix)]
#[test]
fn plan_skill_cannot_mutate_the_checkout_and_retains_its_response() {
    let (result, detail) = run_planning(
        "event-plan-mutation",
        "pathlib.Path('app.txt').write_text('unauthorized change')",
        false,
        false,
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("changed the checkout")
    );
    assert_eq!(detail["file"], "unauthorized change");
    assert_eq!(
        detail["invocations"][0]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

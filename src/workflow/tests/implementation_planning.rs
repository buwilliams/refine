use super::*;

use sha2::{Digest, Sha256};

use crate::model::goal::{
    IMPLEMENTATION_PLAN_SCHEMA_VERSION, ImplementationPlan, ImplementationPlanBinding,
    ImplementationPlanPhase, ImplementationPlanState, PlanningProcessEvidence,
};
use crate::tools::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::workflow::context::WorkflowContext;

#[test]
fn provider_fixture_runs_governed_phases_in_order_and_persists_shared_evidence() {
    let temp_root = unique_temp_dir("workflow-implementation-planning");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    let phase_log = temp_root.join("phases.log");
    fs::write(
        target_root.join("app.py"),
        "def health():\n    return 'ok'\n",
    )
    .unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&target_root, &["add", "app.py"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    fs::write(
        &smoke_ai,
        r#"#!/bin/sh
phase_log='__PHASE_LOG__'
prompt="$1"
prompt_path=$(printf '%s\n' "$prompt" | sed -n 's/^`\(\/.*\)`$/\1/p' | head -n 1)
if [ -n "$prompt_path" ]; then
  prompt=$(sed -n '1,$p' "$prompt_path")
fi
case "$prompt" in
  *"Current Workflow Phase: Plan"*)
    printf 'plan\n' >> "$phase_log"
    printf '%s\n' '{"state":"completed","message":"planned","guidance_applied":[],"planning_result":{"summary":"Plan shared behavior","checklist":[{"id":"P1","description":"Change shared behavior","affected_behavior":["workflow","Goal detail"],"governance_rationale":"Preserve shared authority","verification":["cargo test focused"]}],"criticism_resolutions":[]}}' > "$REFINE_AGENT_SIGNAL_PATH"
    ;;
  *"Current Workflow Phase: Criticize"*)
    case "$prompt" in *"Plan shared behavior"*) ;; *) exit 41 ;; esac
    printf 'criticize\n' >> "$phase_log"
    printf '%s\n' '{"state":"completed","message":"criticized","guidance_applied":[],"planning_result":{"summary":"Add failure evidence","findings":[{"id":"C1","material":true,"checklist_item_ids":["P1"],"description":"Failure evidence must be durable","recommendation":"Record it on the Round"}]}}' > "$REFINE_AGENT_SIGNAL_PATH"
    ;;
  *"Current Workflow Phase: Revise"*)
    case "$prompt" in *"Failure evidence must be durable"*) ;; *) exit 42 ;; esac
    printf 'revise\n' >> "$phase_log"
    printf '%s\n' '{"state":"completed","message":"revised","guidance_applied":[],"planning_result":{"summary":"Plan shared behavior with durable failures","checklist":[{"id":"P1","description":"Change shared behavior and retain failures","affected_behavior":["workflow","Goal detail"],"governance_rationale":"Preserve shared authority","verification":["cargo test focused"]}],"criticism_resolutions":[{"criticism_id":"C1","resolution":"Persist failure evidence before failing the Goal"}]}}' > "$REFINE_AGENT_SIGNAL_PATH"
    ;;
  *)
    printf '%s' "$prompt" > "$phase_log.implementation-prompt"
    printf 'implement\n' >> "$phase_log"
    printf '\n# governed implementation\n' >> app.py
    printf '%s\n' '{"state":"completed","message":"Implemented governed plan; verification fixture passed.","guidance_applied":[],"implementation_evidence":{"checklist":[{"id":"P1","outcome":"completed","evidence":"app.py contains governed implementation marker"}],"verification":["provider fixture: passed"]}}' > "$REFINE_AGENT_SIGNAL_PATH"
    ;;
esac
"#
        .replace("__PHASE_LOG__", &phase_log.display().to_string()),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }

    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    let previous_planning = std::env::var_os("REFINE_SMOKE_AI_GOVERNED_PLANNING");
    let previous_phase_log = std::env::var_os("REFINE_PHASE_LOG");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        std::env::set_var("REFINE_SMOKE_AI_GOVERNED_PLANNING", "1");
        std::env::set_var("REFINE_PHASE_LOG", phase_log.to_str().unwrap());
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Governed implementation planning", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Implement the shared behavior")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai", "quality_enabled": "0"}))
        .unwrap();

    let result = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .evaluate_workflow()
        .unwrap();
    assert_eq!(result.steps.len(), 1);
    assert_eq!(
        fs::read_to_string(&phase_log).unwrap(),
        "plan\ncriticize\nrevise\nimplement\n"
    );
    let implementation_prompt =
        fs::read_to_string(format!("{}.implementation-prompt", phase_log.display())).unwrap();
    assert!(implementation_prompt.contains("Plan shared behavior with durable failures"));
    assert!(implementation_prompt.contains("Implement the shared behavior"));
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    let plan = &detail["rounds"][0]["implementation_plan"];
    assert_eq!(plan["schema_version"], 1);
    assert_eq!(plan["state"], "completed");
    assert_eq!(plan["phase"], "implement");
    assert_eq!(plan["binding"]["goal_id"], "GOAL1");
    assert_eq!(plan["proposal"]["result"]["checklist"][0]["id"], "P1");
    assert_eq!(plan["criticism"]["result"]["findings"][0]["id"], "C1");
    assert_eq!(
        plan["final_plan"]["result"]["criticism_resolutions"][0]["criticism_id"],
        "C1"
    );
    assert_eq!(
        plan["implementation"]["execution"]["checklist"][0]["outcome"],
        "completed"
    );
    assert!(plan["proposal"]["process"]["process_id"].as_str().is_some());
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Review
    );

    unsafe {
        restore_env("REFINE_SMOKE_AI_PATH", previous_smoke_ai);
        restore_env("REFINE_SMOKE_AI_GOVERNED_PLANNING", previous_planning);
        restore_env("REFINE_PHASE_LOG", previous_phase_log);
    }
    fs::remove_dir_all(temp_root).unwrap();
}

unsafe fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
    if let Some(value) = value {
        unsafe { std::env::set_var(key, value) };
    } else {
        unsafe { std::env::remove_var(key) };
    }
}

#[test]
fn interrupted_planning_attempt_fails_and_follow_up_round_starts_with_fresh_identities() {
    let temp_root = unique_temp_dir("workflow-planning-fresh-round");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    let phase_log = temp_root.join("phases.log");
    fs::write(target_root.join("app.py"), "base\n").unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&target_root, &["add", "app.py"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "Initialize"]).unwrap();
    fs::write(
        &smoke_ai,
        r#"#!/bin/sh
phase_log='__PHASE_LOG__'
prompt="$1"
prompt_path=$(printf '%s\n' "$prompt" | sed -n 's/^`\(\/.*\)`$/\1/p' | head -n 1)
if [ -n "$prompt_path" ]; then prompt=$(sed -n '1,$p' "$prompt_path"); fi
case "$prompt" in
  *"Current Workflow Phase: Plan"*)
    printf 'plan\n' >> "$phase_log"
    printf '%s\n' '{"state":"completed","message":"planned","guidance_applied":[],"planning_result":{"summary":"Fresh plan","checklist":[{"id":"P1","description":"Implement fresh round"}],"criticism_resolutions":[]}}' > "$REFINE_AGENT_SIGNAL_PATH" ;;
  *"Current Workflow Phase: Criticize"*)
    printf 'criticize\n' >> "$phase_log"
    printf '%s\n' '{"state":"completed","message":"criticized","guidance_applied":[],"planning_result":{"summary":"No findings","findings":[]}}' > "$REFINE_AGENT_SIGNAL_PATH" ;;
  *"Current Workflow Phase: Revise"*)
    printf 'revise\n' >> "$phase_log"
    printf '%s\n' '{"state":"completed","message":"revised","guidance_applied":[],"planning_result":{"summary":"Fresh final plan","checklist":[{"id":"P1","description":"Implement fresh round"}],"criticism_resolutions":[]}}' > "$REFINE_AGENT_SIGNAL_PATH" ;;
  *)
    printf 'implement\n' >> "$phase_log"
    printf '\n# fresh round implementation\n' >> app.py
    printf '%s\n' '{"state":"completed","message":"fresh round completed","guidance_applied":[],"implementation_evidence":{"checklist":[{"id":"P1","outcome":"completed","evidence":"fixture completed"}],"verification":["fixture passed"]}}' > "$REFINE_AGENT_SIGNAL_PATH" ;;
esac
"#
        .replace("__PHASE_LOG__", &phase_log.display().to_string()),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let base = git_stdout(&target_root, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Planning restart", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Implement")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let claim_id = automation.claim("GOAL1").unwrap();
    let execution_id = automation.start_claim(&claim_id).unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();
    let branch = "refine/GOAL1/round-1";
    let worktree_path = temp_root.join("planning-worktree");
    FileGitWorktreeService::with_runtime_root(&target_root, &runtime_root)
        .ensure_worktree(branch, &worktree_path)
        .unwrap();
    work_items
        .update_goal_git_refs("GOAL1", branch, "main", &base, None)
        .unwrap();
    let context = json!({
        "version": 1,
        "goal": {"id": "GOAL1"},
        "previous_rounds": [],
        "current_round": {"round": 1, "prompt": "Implement"}
    });
    work_items
        .update_goal_round_evaluation_summary("GOAL1", 0, &json!({"agent_context": context}))
        .unwrap();
    let now = now_timestamp();
    let plan = ImplementationPlan {
        schema_version: IMPLEMENTATION_PLAN_SCHEMA_VERSION,
        state: ImplementationPlanState::InProgress,
        phase: ImplementationPlanPhase::Plan,
        binding: ImplementationPlanBinding {
            goal_id: "GOAL1".to_string(),
            round_idx: 0,
            context_version: 1,
            context_digest: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&context).unwrap())
            ),
            claim_id: claim_id.clone(),
            execution_id: execution_id.clone(),
            implementation_branch: branch.to_string(),
            target_branch: "main".to_string(),
            base_commit: base,
        },
        started_at: now.clone(),
        phase_started_at: now.clone(),
        updated_at: now,
        active_process: Some(PlanningProcessEvidence {
            operation_id: "planning-operation".to_string(),
            process_id: None,
            provider: "smoke-ai".to_string(),
            state: None,
            exit_code: None,
            output: None,
            structured_output: None,
        }),
        completed_at: None,
        proposal: None,
        criticism: None,
        final_plan: None,
        implementation: None,
        failure: None,
    };
    work_items
        .replace_goal_round_implementation_plan("GOAL1", 0, None, &plan)
        .unwrap();

    assert_eq!(
        automation
            .recover_interrupted_goals("daemon restarted")
            .unwrap(),
        1
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Failed
    );
    let recovered = automation.load_state().unwrap();
    let claim = recovered
        .claims
        .iter()
        .find(|claim| claim.claim_id == claim_id)
        .unwrap();
    assert_eq!(claim.state, WorkflowClaimState::Interrupted);
    assert_eq!(claim.execution_id.as_deref(), Some(execution_id.as_str()));
    let stale_ctx = WorkflowContext::new(
        &runtime_root,
        &target_root,
        claim.clone(),
        &execution_id,
        0,
        serde_json::Map::new(),
        FileWorkItemService::new(&refine_dir),
    );
    let stale_write_error = persist_plan(&stale_ctx, Some(&plan), &plan).unwrap_err();
    assert!(
        stale_write_error
            .to_string()
            .contains("no longer owns implementation planning claim"),
        "{stale_write_error}"
    );
    let interrupted_detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(
        interrupted_detail["rounds"][0]["implementation_plan"]["state"],
        "failed"
    );
    assert_eq!(
        interrupted_detail["rounds"][0]["implementation_plan"]["failure"]["category"],
        "interrupted"
    );
    assert!(worktree_path.is_dir());
    assert!(
        git_stdout(
            &target_root,
            &["show-ref", "--verify", &format!("refs/heads/{branch}")]
        )
        .is_ok()
    );

    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Recover with a fresh plan")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai", "quality_enabled": "0"}))
        .unwrap();
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    let previous_planning = std::env::var_os("REFINE_SMOKE_AI_GOVERNED_PLANNING");
    let previous_phase_log = std::env::var_os("REFINE_PHASE_LOG");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        std::env::set_var("REFINE_SMOKE_AI_GOVERNED_PLANNING", "1");
        std::env::set_var("REFINE_PHASE_LOG", phase_log.to_str().unwrap());
    }

    automation.evaluate_workflow().unwrap();
    let recovered_detail = work_items.show_goal_detail("GOAL1").unwrap();
    let recovered_plan = &recovered_detail["rounds"][1]["implementation_plan"];
    assert_eq!(
        fs::read_to_string(&phase_log).unwrap(),
        "plan\ncriticize\nrevise\nimplement\n"
    );
    assert_eq!(
        recovered_plan["proposal"]["result"]["summary"],
        "Fresh plan"
    );
    assert_ne!(recovered_plan["binding"]["claim_id"], claim_id);
    assert_ne!(recovered_plan["binding"]["execution_id"], execution_id);
    assert_eq!(recovered_plan["binding"]["round_idx"], 1);
    assert_eq!(recovered_plan["state"], "completed");

    unsafe {
        restore_env("REFINE_SMOKE_AI_PATH", previous_smoke_ai);
        restore_env("REFINE_SMOKE_AI_GOVERNED_PLANNING", previous_planning);
        restore_env("REFINE_PHASE_LOG", previous_phase_log);
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn planning_worktree_mutation_is_retained_as_failure_evidence_and_blocks_implementation() {
    let temp_root = unique_temp_dir("workflow-planning-mutation");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::write(target_root.join("app.py"), "base\n").unwrap();
    git(
        &target_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&target_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&target_root, &["add", "app.py"]).unwrap();
    git(&target_root, &["commit", "-q", "-m", "Initialize"]).unwrap();
    fs::write(
        &smoke_ai,
        r#"#!/bin/sh
printf 'planning must not write\n' > unexpected.txt
printf '%s\n' '{"state":"completed","message":"planned","guidance_applied":[],"planning_result":{"summary":"Plan","checklist":[{"id":"P1","description":"Implement"}]}}' > "$REFINE_AGENT_SIGNAL_PATH"
"#,
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let _guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    let previous_planning = std::env::var_os("REFINE_SMOKE_AI_GOVERNED_PLANNING");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        std::env::set_var("REFINE_SMOKE_AI_GOVERNED_PLANNING", "1");
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Mutation fence", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Implement")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai", "quality_enabled": "0"}))
        .unwrap();

    assert!(
        WorkflowEngine::with_target_root(&runtime_root, &target_root)
            .evaluate_workflow()
            .unwrap_err()
            .to_string()
            .contains("changed the worktree")
    );
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    let plan = &detail["rounds"][0]["implementation_plan"];
    assert_eq!(plan["state"], "failed");
    assert_eq!(plan["phase"], "plan");
    assert_eq!(plan["failure"]["category"], "worktree_mutation");
    assert_ne!(plan["failure"]["git_before"], plan["failure"]["git_after"]);
    let worktree = Path::new(
        detail["rounds"][0]["implementation_plan"]["binding"]["implementation_branch"]
            .as_str()
            .unwrap(),
    );
    let worktree_path = target_root
        .join(".git/refine-worktrees")
        .join(worktree.to_string_lossy().replace('/', "-"));
    assert!(worktree_path.join("unexpected.txt").is_file());
    assert!(plan.get("implementation").is_none() || plan["implementation"].is_null());

    unsafe {
        restore_env("REFINE_SMOKE_AI_PATH", previous_smoke_ai);
        restore_env("REFINE_SMOKE_AI_GOVERNED_PLANNING", previous_planning);
    }
    fs::remove_dir_all(temp_root).unwrap();
}

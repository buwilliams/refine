use super::*;

#[test]
fn file_automation_promotes_todo_goals_and_starts_executions() {
    let temp_root = unique_temp_dir("automation");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Queued", Some("GOAL1"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .create_goal_summary("Backlog", Some("GOAL2"))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 1);
    assert_eq!(automation.promote().unwrap(), 0);
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].goal_id, "GOAL1");

    let execution_id = automation.start_claim(&state.claims[0].claim_id).unwrap();
    assert!(execution_id.starts_with("exec-"));
    let state = automation.load_state().unwrap();
    assert_eq!(
        state.claims[0].execution_id.as_deref(),
        Some(execution_id.as_str())
    );
    assert_eq!(state.claims[0].state, WorkflowClaimState::Running);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn promoting_a_large_todo_queue_does_not_materialize_worktrees() {
    let temp_root = unique_temp_dir("large-todo-queue");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    // Pinned: this asserts worktrees are not materialized, not what the host
    // would choose, and an unset cap now varies with the machine.
    FileSettingsService::new(&refine_dir)
        .update(&json!({"parallel_run_cap": 2}))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    for index in 0..128 {
        let id = format!("GOAL{index:04}");
        work_items.create_goal_summary(&id, Some(&id)).unwrap();
        work_items
            .transition_goal_status(&id, GoalStatus::Todo)
            .unwrap();
    }

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 2);
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 2);
    assert!(
        state
            .claims
            .iter()
            .all(|claim| claim.state == WorkflowClaimState::Claimed)
    );
    assert!(!target_root.join(".git/refine-worktrees").exists());
    for index in 0..128 {
        let id = format!("GOAL{index:04}");
        assert_eq!(
            work_items.show_goal_summary(&id).unwrap().goal.status,
            GoalStatus::Todo
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_auto_promotes_backlog_goals_when_configured() {
    let temp_root = unique_temp_dir("automation-backlog-promote");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Instant Backlog", Some("GOAL1"))
        .unwrap();
    work_items
        .create_goal_summary("Never Backlog", Some("GOAL2"))
        .unwrap();
    let settings = FileSettingsService::new(&refine_dir);
    settings
        .update(&json!({"backlog_promote_after_seconds": "-1"}))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 0);
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Backlog
    );

    settings
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();
    assert_eq!(automation.promote().unwrap(), 2);
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Todo
    );
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 2);
    let mut claimed_goal_ids = state
        .claims
        .iter()
        .map(|claim| claim.goal_id.as_str())
        .collect::<Vec<_>>();
    claimed_goal_ids.sort_unstable();
    assert_eq!(claimed_goal_ids, vec!["GOAL1", "GOAL2"]);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_promotes_all_ordered_feature_backlog_goals() {
    let temp_root = unique_temp_dir("automation-feature-backlog-promote");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_feature_summary("Imported Feature", Some("FEA1"), None, None, None)
        .unwrap();
    for id in ["GOAL1", "GOAL2", "GOAL3"] {
        work_items.create_goal_summary(id, Some(id)).unwrap();
        work_items.assign_goal_to_feature("FEA1", id).unwrap();
        work_items.order_goal_in_feature("FEA1", id).unwrap();
    }
    FileSettingsService::new(&refine_dir)
        .update(&json!({"backlog_promote_after_seconds": "0"}))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote_backlog_to_todo().unwrap(), 3);
    for id in ["GOAL1", "GOAL2", "GOAL3"] {
        assert_eq!(
            work_items.show_goal_summary(id).unwrap().goal.status,
            GoalStatus::Todo
        );
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_blocks_lower_priority_work_behind_higher_priority_goals() {
    let temp_root = unique_temp_dir("automation-priority-band");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    FileSettingsService::new(&refine_dir)
        .update(&json!({"parallel_run_cap": 3}))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    for (id, priority) in [("LOW", "low"), ("MEDIUM", "medium"), ("HIGH", "high")] {
        work_items.create_goal_summary(id, Some(id)).unwrap();
        work_items
            .update_goal_metadata_summary(id, None, Some(priority), None, None)
            .unwrap();
        work_items
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap();
    }

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert!(automation.claim("MEDIUM").is_err());
    assert!(automation.claim("LOW").is_err());
    assert_eq!(automation.promote().unwrap(), 1);
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].goal_id, "HIGH");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_respects_feature_order_on_promote_claim_and_start() {
    let temp_root = unique_temp_dir("automation-feature-order");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let claim_runtime_root = temp_root.join("run/8081");
    FileSettingsService::new(&refine_dir)
        .update(&json!({
            "parallel_run_cap": 2,
            "parallel_per_node_cap": 2
        }))
        .unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_feature_summary("Feature", Some("FEAT1"), None, None, None)
        .unwrap();
    for id in ["FIRST", "SECOND", "UNORDERED"] {
        work_items.create_goal_summary(id, Some(id)).unwrap();
        work_items
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap();
        work_items.assign_goal_to_feature("FEAT1", id).unwrap();
    }
    for id in ["FIRST", "SECOND"] {
        work_items.order_goal_in_feature("FEAT1", id).unwrap();
    }

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert!(automation.claim("SECOND").is_err());
    assert_eq!(automation.promote().unwrap(), 2);
    let state = automation.load_state().unwrap();
    let claimed_goal_ids = state
        .claims
        .iter()
        .map(|claim| claim.goal_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(claimed_goal_ids, vec!["FIRST", "UNORDERED"]);

    for status in [
        GoalStatus::InProgress,
        GoalStatus::ReadyMerge,
        GoalStatus::Build,
        GoalStatus::Review,
    ] {
        work_items
            .advance_automated_goal_status("FIRST", status)
            .unwrap();
    }
    let claim_automation = WorkflowEngine::with_target_root(&claim_runtime_root, &target_root);
    assert_eq!(claim_automation.promote().unwrap(), 2);
    let state = claim_automation.load_state().unwrap();
    let second_claim = state
        .claims
        .iter()
        .find(|claim| claim.goal_id == "SECOND")
        .map(|claim| claim.claim_id.clone())
        .unwrap();
    let rejected_bulk_reopen = work_items
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["FIRST".to_string()]),
                ..Default::default()
            },
            crate::tools::product::work_items::BulkGoalUpdate::Status("todo".to_string()),
        )
        .unwrap();
    assert_eq!(rejected_bulk_reopen.updated, 0);
    assert_eq!(rejected_bulk_reopen.skipped, 1);
    assert_eq!(
        work_items.show_goal_summary("FIRST").unwrap().goal.status,
        GoalStatus::Review
    );
    claim_automation.start_claim(&second_claim).unwrap();

    fs::remove_dir_all(temp_root).unwrap();
}

// Claim eligibility used to be answered by rescanning the whole snapshot per
// candidate, with the priority scan calling the Feature scan inside its own
// loop. That is quadratic at rest and cubic once Todo Goals share a Feature and
// a priority band, which is the shape a real backlog takes: at a thousand Goals
// a single promotion pass costs on the order of 1e9 predicate evaluations and
// cannot finish inside its own one-second replenish interval.
//
// Ten thousand Goals is chosen so the complexity classes are unmistakable —
// linear is microseconds, quadratic is 1e8 and takes seconds to minutes, cubic
// never finishes. The bound is deliberately loose so machine speed cannot make
// this flaky while still failing outright on any return to super-linear cost.
#[test]
fn claim_eligibility_stays_linear_on_a_large_single_feature_backlog() {
    use crate::model::goal::GoalIndexProjection;
    use crate::tools::product::project_state::{GoalSummaryProjection, ProjectionSnapshot};
    use crate::workflow::GoalPriority;
    use crate::workflow::policy::ClaimEligibility;
    use std::collections::{BTreeMap, BTreeSet};
    use std::time::Instant;

    const GOAL_COUNT: i64 = 10_000;

    let mut goals = BTreeMap::new();
    for index in 0..GOAL_COUNT {
        let id = format!("GOAL{index:06}");
        goals.insert(
            id.clone(),
            GoalSummaryProjection {
                goal: GoalIndexProjection {
                    id: id.clone(),
                    name: format!("Goal {index}"),
                    status: GoalStatus::Todo,
                    priority: GoalPriority::Medium,
                    reporter: None,
                    assignee: None,
                    round_count: 0,
                    created: "2026-01-01T00:00:00Z".to_string(),
                    updated: "2026-01-01T00:00:00Z".to_string(),
                    branch_name: None,
                    node_id: Some("default".to_string()),
                    feature_id: Some("FEATURE1".to_string()),
                    feature_order: Some(index),
                    json_path: format!("goals/GO/{index:06}/goal.json"),
                },
                node_display_name: None,
                latest_round_prompt: None,
                searchable_text: String::new(),
                activity_ids: Vec::new(),
            },
        );
    }
    let snapshot = ProjectionSnapshot {
        goals,
        ..ProjectionSnapshot::default()
    };

    let goals = snapshot
        .goals
        .values()
        .map(|projection| projection.goal.clone())
        .collect::<Vec<_>>();
    let started = Instant::now();
    let eligibility = ClaimEligibility::new(goals.iter(), &BTreeSet::new());
    let eligible = goals
        .iter()
        .filter(|goal| eligibility.feature_eligible(&goal.id))
        .filter(|goal| eligibility.priority_eligible(goal))
        .count();
    let elapsed = started.elapsed();

    // Only the lowest-ordered Goal clears the Feature queue; every later one is
    // held behind it. Same answer the per-candidate scans gave.
    assert_eq!(eligible, 1, "feature order must still serialize the queue");
    assert!(
        elapsed < Duration::from_secs(5),
        "eligibility for {GOAL_COUNT} Goals took {elapsed:?}, which indicates super-linear cost"
    );
}

// Scheduling used to load a projection of the whole project, so its cost and
// memory tracked everything the project had ever contained rather than what it
// was currently doing. At five million Goals that does not fit however few are
// active. The scheduler now reads an index bounded by work in flight, and this
// pins that: a promotion pass must not build a projection at all.
#[test]
fn scheduling_never_builds_a_projection_of_the_whole_project() {
    use crate::tools::product::project_state::{ActiveGoalIndex, FileProjectStateStore};

    let temp_root = unique_temp_dir("automation-bounded");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);

    // One claimable Goal behind a body of completed work, as a mature project
    // looks.
    work_items
        .create_goal_summary("Live", Some("GOALLIVE"))
        .unwrap();
    work_items
        .transition_goal_status("GOALLIVE", GoalStatus::Todo)
        .unwrap();
    for index in 0..40 {
        let id = format!("GOALDONE{index:03}");
        work_items
            .create_goal_summary("Finished", Some(&id))
            .unwrap();
        work_items.cancel_goal_summary(&id).unwrap();
    }

    // Write-through keeps the index current, so the completed Goals never enter
    // the scheduler's working set in the first place.
    let index = ActiveGoalIndex::load_or_rebuild(&refine_dir).unwrap();
    assert_eq!(
        index
            .goals()
            .map(|goal| goal.id.clone())
            .collect::<Vec<_>>(),
        vec!["GOALLIVE"],
        "completed Goals must not stay resident"
    );

    FileProjectStateStore::reset_rebuild_count(&refine_dir);
    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 1);

    assert_eq!(
        FileProjectStateStore::rebuild_count(&refine_dir),
        0,
        "a promotion pass must not read a projection of every Goal"
    );
    let state = automation.load_state().unwrap();
    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].goal_id, "GOALLIVE");

    fs::remove_dir_all(temp_root).unwrap();
}

// The governor was wired as a fallback but the fallback was unreachable: a
// seeded cap meant the key was never unset, so every node ran a fixed 2
// regardless of its hardware — the exact behavior the governor exists to
// remove. These pin the three configurations that have to behave differently.
#[test]
fn concurrency_follows_the_host_unless_an_operator_overrides_it() {
    use crate::tools::host::host_resources::{HostResources, observed_agent_memory_bytes};

    let temp_root = unique_temp_dir("automation-governed-cap");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let engine = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let settings = FileSettingsService::new(&refine_dir);

    let expected = HostResources::current(&runtime_root)
        .recommended_agent_concurrency(observed_agent_memory_bytes(&runtime_root));

    // Unset: the host decides.
    assert_eq!(engine.policy().unwrap().global_limit, expected);
    assert!(expected >= 1, "scarcity slows work rather than stopping it");

    // Explicitly chosen: the operator decides, even above what the host would
    // pick, because a deliberate cap is not the governor's to overrule.
    settings
        .update(&json!({"parallel_run_cap": expected + 3}))
        .unwrap();
    assert_eq!(engine.policy().unwrap().global_limit, expected + 3);

    // Cleared: the host decides again, without hand-editing the node registry.
    settings.update(&json!({"parallel_run_cap": ""})).unwrap();
    assert_eq!(engine.policy().unwrap().global_limit, expected);

    fs::remove_dir_all(temp_root).unwrap();
}

// An operator's cap is honoured whatever its value, including one equal to the
// number Refine used to seed. Reinterpreting that as "unset" would hand a
// capable host to the governor without anyone touching anything, but would also
// make the value impossible to choose deliberately, and the two cases cannot be
// told apart at read time. Clearing is the supported way to hand it back.
#[test]
fn a_stored_cap_is_honoured_even_when_it_equals_the_retired_default() {
    use crate::tools::host::host_resources::{HostResources, observed_agent_memory_bytes};

    let temp_root = unique_temp_dir("automation-retired-cap");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let engine = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let settings = FileSettingsService::new(&refine_dir);

    let expected = HostResources::current(&runtime_root)
        .recommended_agent_concurrency(observed_agent_memory_bytes(&runtime_root));

    settings.update(&json!({"parallel_run_cap": 2})).unwrap();
    assert_eq!(
        engine.policy().unwrap().global_limit,
        2,
        "a stored cap must be honoured even when it matches the retired default"
    );

    settings.update(&json!({"parallel_run_cap": ""})).unwrap();
    assert_eq!(
        engine.policy().unwrap().global_limit,
        expected,
        "clearing hands the decision to the host"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

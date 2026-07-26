use super::*;

#[test]
fn a_nested_projection_cache_still_resolves_the_real_active_node() {
    let temp_root = unique_temp_dir("work-item-nested-cache-active-node");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(
        runtime_root.join("active-node.json"),
        json!({
            "active_node_id": "bo2lnxnevo03-buddy",
            "refine_dir": refine_dir.display().to_string(),
            "updated_at": "2026-07-25T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();

    // A goal owned by that Node, as automation would have created it.
    let owned = FileWorkItemService::for_node(&refine_dir, "bo2lnxnevo03-buddy")
        .create_goal_summary("Owned by the active node", Some("GOAL1"))
        .unwrap();
    assert_eq!(owned.goal.node_id.as_deref(), Some("bo2lnxnevo03-buddy"));

    // The cache directory claim execution uses: two levels below the runtime root.
    let nested_cache = runtime_root.join("cache/workflow").join("CLAIM123");
    let service =
        FileWorkItemService::with_projection_cache(&refine_dir, &runtime_root, &nested_cache);

    // Ownership must resolve against runtime_root/active-node.json, not against a
    // file that would have to sit next to the nested cache directory.
    assert!(!nested_cache.join("active-node.json").exists());
    service
        .transition_goal_status(&owned.goal.id, GoalStatus::Todo)
        .unwrap();
    assert_eq!(
        service
            .show_goal_summary(&owned.goal.id)
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );

    fs::remove_dir_all(temp_root).unwrap_or(());
}

#[test]
fn file_work_item_service_creates_and_lists_goal_json() {
    let temp_root = unique_temp_dir("work-item-create");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);

    let goal = service
        .create_goal_summary("Created from Rust", Some("GOAL1"))
        .unwrap();
    assert_eq!(goal.goal.id, "GOAL1");
    assert_eq!(goal.goal.status, GoalStatus::Backlog);
    assert!(refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert_eq!(service.list_goal_summaries().unwrap().len(), 1);
    assert_eq!(
        service.show_goal_summary("GOAL1").unwrap().goal.name,
        "Created from Rust"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn retrying_a_failed_goal_clears_the_recorded_failure_reason() {
    let temp_root = unique_temp_dir("work-item-retry-clears-failure");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Stale candidate", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    service
        .update_latest_goal_round_evaluation_summary(
            "GOAL1",
            &json!({
                "failure_category": "merge",
                "failure_message": "Candidate abc123 is stale: recorded base def456 is not its ancestor",
                "failure_at": "2026-07-25T12:02:30Z"
            }),
        )
        .unwrap();

    let failed = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(failed["rounds"][0]["failure_category"], "merge");
    assert!(
        failed["rounds"][0]["failure_message"]
            .as_str()
            .unwrap_or_default()
            .contains("is stale")
    );

    // A retry reuses this same round, so the reason it carries is now spent.
    // Leaving it would show a live failure on work that has moved on.
    service.retry_goal_merge_summary("GOAL1").unwrap();

    let retried = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(retried["status"], "ready-merge");
    assert_eq!(retried["rounds"][0]["failure_category"], "");
    assert_eq!(retried["rounds"][0]["failure_message"], "");
    assert_eq!(retried["rounds"][0]["failure_at"], "");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_uses_active_node_and_rejects_foreign_mutations() {
    let temp_root = unique_temp_dir("work-item-node-ownership");
    let refine_dir = temp_root.join(".refine");
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("remote-node").unwrap();
    nodes.activate("remote-node").unwrap();

    let service = FileWorkItemService::new(&refine_dir);
    let local_goal = service
        .create_goal_summary("Remote-owned", Some("GOAL1"))
        .unwrap();
    assert_eq!(local_goal.goal.node_id.as_deref(), Some("remote-node"));
    let local_feature = service
        .create_feature_summary("Remote feature", Some("FEA1"), None, None, None)
        .unwrap();
    assert_eq!(
        local_feature.feature.node_id.as_deref(),
        Some("remote-node")
    );

    nodes.activate("default").unwrap();
    let err = service
        .update_goal_metadata_summary("GOAL1", Some("Blocked"), None, None, None)
        .unwrap_err();
    assert_eq!(
        err.category(),
        crate::process::supervisor::errors::ErrorCategory::Conflict
    );
    let err = service
        .update_feature_metadata_summary("FEA1", Some("Blocked"), None, None, None)
        .unwrap_err();
    assert_eq!(
        err.category(),
        crate::process::supervisor::errors::ErrorCategory::Conflict
    );

    service
        .bulk_transfer_goals_to_node(
            "default",
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    let updated = service
        .update_goal_metadata_summary("GOAL1", Some("Default-owned"), None, None, None)
        .unwrap();
    assert_eq!(updated.goal.name, "Default-owned");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_spreads_eligible_goals_evenly_across_nodes() {
    let temp_root = unique_temp_dir("distribute-spread");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("node-a").unwrap();
    nodes.create("node-b").unwrap();
    for index in 1..=6 {
        service
            .create_goal_summary(&format!("Goal {index}"), Some(&format!("GOAL{index}")))
            .unwrap();
    }

    let targets = vec![
        "default".to_string(),
        "node-a".to_string(),
        "node-b".to_string(),
    ];
    let result = service
        .distribute_goals_across_nodes(&targets, false, &std::collections::BTreeSet::new(), false)
        .unwrap();

    assert_eq!(result.strategy, "spread");
    assert_eq!(result.eligible, 6);
    let mut counts = std::collections::BTreeMap::new();
    for goal in service.list_goal_summaries().unwrap() {
        let owner = goal.goal.node_id.unwrap_or_else(|| "default".to_string());
        *counts.entry(owner).or_insert(0usize) += 1;
    }
    assert_eq!(counts.get("default"), Some(&2));
    assert_eq!(counts.get("node-a"), Some(&2));
    assert_eq!(counts.get("node-b"), Some(&2));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_converge_moves_only_reviewable_goals_to_review_node() {
    let temp_root = unique_temp_dir("distribute-converge");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let nodes = crate::tools::product::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("worker").unwrap();
    service
        .create_goal_summary("Reviewable", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Still backlog", Some("GOAL2"))
        .unwrap();
    service.transfer_goal_to_node("worker", "GOAL1").unwrap();
    service.transfer_goal_to_node("worker", "GOAL2").unwrap();
    // Review is a workflow-owned state; write it directly for the fixture.
    let goal_path = refine_dir.join("goals/GO/AL1/goal.json");
    let updated = fs::read_to_string(&goal_path)
        .unwrap()
        .replace("\"backlog\"", "\"review\"");
    fs::write(&goal_path, updated).unwrap();

    let targets = vec!["default".to_string()];
    let result = service
        .distribute_goals_across_nodes(&targets, true, &std::collections::BTreeSet::new(), false)
        .unwrap();

    assert_eq!(result.strategy, "converge");
    assert_eq!(result.moved, 1);
    assert_eq!(result.moves[0].goal_id, "GOAL1");
    assert_eq!(result.moves[0].to_node_id, "default");
    let backlog_goal = service.show_goal_summary("GOAL2").unwrap();
    assert_eq!(backlog_goal.goal.node_id.as_deref(), Some("worker"));
    fs::remove_dir_all(temp_root).unwrap();
}

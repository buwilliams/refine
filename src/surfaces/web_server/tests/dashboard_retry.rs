use super::*;

#[test]
fn dashboard_retry_delays_reuse_one_projection_across_failures_and_filter_nodes() {
    let temp_root = unique_temp_dir("dashboard-retry-delay-projection");
    let app_root = temp_root.join("app");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&app_root);
    let refine_dir = refine_dir_for_target_root(&app_root).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    let failures = [
        ("GOAL-LOCAL-1", "default", "local failure one"),
        ("GOAL-LOCAL-2", "default", "local failure two"),
        ("GOAL-REMOTE", "remote-node", "remote failure"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (goal_id, node_id, message))| {
        work_items
            .create_goal_summary(message, Some(goal_id))
            .unwrap();
        work_items
            .append_goal_round_summary(goal_id, "Reporter", "Retry this Round")
            .unwrap();
        work_items
            .transition_goal_status(goal_id, GoalStatus::Todo)
            .unwrap();
        let detail = work_items.show_goal_detail(goal_id).unwrap();
        let revision = crate::tools::product::work_items::workflow_revision(&detail);
        let failed_at =
            Utc::now() + chrono::TimeDelta::minutes(2) + chrono::TimeDelta::seconds(index as i64);
        let failed_at = failed_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let retry_not_before = (chrono::DateTime::parse_from_rfc3339(&failed_at).unwrap()
            + chrono::TimeDelta::seconds(5))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        (
            crate::workflow::WorkflowClaim {
                claim_id: format!("claim-{goal_id}"),
                goal_id: goal_id.to_string(),
                node_id: node_id.to_string(),
                provider: "smoke-ai".to_string(),
                target_app_id: "default".to_string(),
                execution_id: Some(format!("execution-{goal_id}")),
                round_idx: Some(0),
                goal_revision: Some(revision),
                failure_stage: Some("execution".to_string()),
                failure_message: Some(message.to_string()),
                decision_version: 2,
                occurrences: 1,
                state: WorkflowClaimState::Failed,
                created_at: failed_at.clone(),
                updated_at: failed_at,
            },
            retry_not_before,
        )
    })
    .collect::<Vec<_>>();

    let mut server = server_with_projection();
    server.target_root = Some(app_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    server.warm_current_projection_cache().unwrap();
    FileProjectStateStore::reset_rebuild_count(&refine_dir);

    let write_failures = |count: usize| {
        let state = crate::workflow::WorkflowAutomationState {
            claims: failures
                .iter()
                .take(count)
                .map(|(claim, _)| claim.clone())
                .collect(),
            ..Default::default()
        };
        fs::create_dir_all(&runtime_root).unwrap();
        fs::write(
            runtime_root.join(crate::workflow::WORKFLOW_AUTOMATION_STATE_FILE),
            serde_json::to_vec_pretty(&state).unwrap(),
        )
        .unwrap();
    };
    let retry_items = |response: &ApiResponse| {
        response.body["needs_attention"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item.get("retry_not_before").is_some())
            .cloned()
            .collect::<Vec<_>>()
    };

    write_failures(1);
    let single = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(single.status, 200, "{:#}", single.body);
    let single_items = retry_items(&single);
    assert_eq!(single_items.len(), 1);
    assert_eq!(single_items[0]["goal_id"], "GOAL-LOCAL-1");
    assert_eq!(
        single_items[0]["retry_not_before"].as_str(),
        Some(failures[0].1.as_str())
    );
    let rebuilds_with_one_failure = FileProjectStateStore::rebuild_count(&refine_dir);

    write_failures(failures.len());
    let current_node = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard".to_string(),
        body: None,
    });
    assert_eq!(current_node.status, 200, "{:#}", current_node.body);
    let current_items = retry_items(&current_node);
    assert_eq!(current_items.len(), 2);
    assert!(
        current_items
            .iter()
            .all(|item| item["goal_id"] != "GOAL-REMOTE")
    );
    for (claim, expected_retry) in failures.iter().take(2) {
        let item = current_items
            .iter()
            .find(|item| item["goal_id"].as_str() == Some(claim.goal_id.as_str()))
            .unwrap();
        assert_eq!(
            item["retry_not_before"].as_str(),
            Some(expected_retry.as_str())
        );
    }

    let all_nodes = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/dashboard?node=all".to_string(),
        body: None,
    });
    assert_eq!(all_nodes.status, 200, "{:#}", all_nodes.body);
    let all_items = retry_items(&all_nodes);
    assert_eq!(all_items.len(), 3);
    assert!(all_items.iter().any(|item| {
        item["goal_id"] == "GOAL-REMOTE"
            && item["retry_not_before"].as_str() == Some(failures[2].1.as_str())
    }));
    assert_eq!(
        FileProjectStateStore::rebuild_count(&refine_dir),
        rebuilds_with_one_failure,
        "Dashboard projection rebuild count must not grow with failed-claim count"
    );
    assert_eq!(rebuilds_with_one_failure, 0);

    remove_temp_dir(&temp_root);
}

use super::*;

#[test]
fn direct_and_daemon_http_imports_have_the_same_durable_semantics() {
    let direct_root = unique_temp_dir("import-parity-direct");
    let http_root = unique_temp_dir("import-parity-http");
    let direct_refine_dir = direct_root.join(".refine");
    let http_refine_dir = http_root.join(".refine");
    for refine_dir in [&direct_refine_dir, &http_refine_dir] {
        let service = FileWorkItemService::new(refine_dir);
        let original = service.create_goal_summary("Existing Goal", None).unwrap();
        service
            .append_goal_round_summary(
                &original.goal.id,
                "Original Reporter",
                "Existing duplicate prompt",
            )
            .unwrap();
    }
    let drafts = json!([
        {
            "name": "Created Goal",
            "prompt": "Created parity prompt",
            "reporter": "QA",
            "assignee": "Owner",
            "priority": "high"
        },
        {
            "name": "Update Existing",
            "prompt": "Existing duplicate prompt",
            "reporter": "QA",
            "priority": "medium",
            "duplicate_decision": "update_original_priority"
        }
    ]);
    let parsed_drafts =
        crate::tools::product::imports::import_drafts_from_value(&drafts, None).unwrap();
    let direct = crate::tools::product::imports::FileImportService::new(&direct_refine_dir)
        .persist_with_destination(
            parsed_drafts,
            crate::tools::product::imports::ImportFeatureDestination::New {
                name: "Parity Feature".to_string(),
                description: Some("Same destination".to_string()),
                reporter: Some("QA".to_string()),
                assignee: Some("Owner".to_string()),
            },
            &mut (),
        )
        .unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(http_root.clone());
    let persisted = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/persist".to_string(),
        body: Some(json!({
            "new_feature_name": "Parity Feature",
            "new_feature_description": "Same destination",
            "feature_reporter": "QA",
            "feature_assignee": "Owner",
            "drafts": drafts
        })),
    });
    assert_eq!(persisted.status, 201, "{}", persisted.body);

    assert_eq!(
        direct.created,
        persisted.body["count"].as_u64().unwrap() as usize
    );
    assert_eq!(
        serde_json::to_value(&direct.duplicate_actions).unwrap(),
        persisted.body["duplicate_actions"]
    );
    assert_eq!(
        direct.duplicate_outcomes[0].outcome,
        "updated_original_priority"
    );
    assert_eq!(direct.feature.as_ref().unwrap().name, "Parity Feature");
    assert!(direct.feature.as_ref().unwrap().created);
    assert_eq!(persisted.body["feature"]["name"], "Parity Feature");

    let direct_service = FileWorkItemService::new(&direct_refine_dir);
    let direct_created = direct_service
        .show_goal_summary(&direct.goal_ids[0])
        .unwrap();
    let http_created_id = persisted.body["goals"][0]["id"].as_str().unwrap();
    let http_created = FileWorkItemService::new(&http_refine_dir)
        .show_goal_summary(http_created_id)
        .unwrap();
    assert_eq!(direct_created.goal.name, http_created.goal.name);
    assert_eq!(direct_created.goal.priority, http_created.goal.priority);
    assert_eq!(direct_created.goal.reporter, http_created.goal.reporter);
    assert_eq!(direct_created.goal.assignee, http_created.goal.assignee);
    assert_eq!(
        direct_created.goal.round_count,
        http_created.goal.round_count
    );
    assert_eq!(
        direct_created.latest_round_prompt,
        http_created.latest_round_prompt
    );
    assert!(direct_created.goal.feature_id.is_some());
    assert!(http_created.goal.feature_id.is_some());

    let direct_existing = direct_service
        .list_goal_summaries()
        .unwrap()
        .into_iter()
        .find(|goal| goal.goal.name == "Existing Goal")
        .unwrap();
    let http_existing = FileWorkItemService::new(&http_refine_dir)
        .list_goal_summaries()
        .unwrap()
        .into_iter()
        .find(|goal| goal.goal.name == "Existing Goal")
        .unwrap();
    assert_eq!(direct_existing.goal.priority.as_str(), "medium");
    assert_eq!(direct_existing.goal.priority, http_existing.goal.priority);

    remove_temp_dir(&direct_root);
    remove_temp_dir(&http_root);
}

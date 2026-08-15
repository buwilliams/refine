use super::super::*;

pub(crate) fn goal_create_list_show_edit_note_round_delete(fixture: &IntegrationFixture) {
    let goal_id = fixture.create_goal("cli surface goal");
    assert_eq!(fixture.goal_field(&goal_id, "status"), "backlog");
    assert_eq!(fixture.goal_field(&goal_id, "priority"), "low");
    assert_eq!(fixture.goal_field(&goal_id, "name"), "cli surface goal");
    assert_eq!(fixture.goal_field(&goal_id, "node_id"), "default");

    let list = fixture.run_refine(&["goal", "list"]);
    fixture.assert_success("goal list", &list);
    let payload = fixture.json_stdout(&list);
    assert!(
        payload["goals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|goal| goal["id"].as_str() == Some(goal_id.as_str()))
    );

    let edit = fixture.run_refine(&[
        "goal",
        "edit",
        &goal_id,
        "--name",
        "cli surface renamed goal",
        "--priority",
        "high",
    ]);
    fixture.assert_success("goal edit", &edit);
    assert_eq!(
        fixture.goal_field(&goal_id, "name"),
        "cli surface renamed goal"
    );
    assert_eq!(fixture.goal_field(&goal_id, "priority"), "high");

    let note = fixture.run_refine(&[
        "goal",
        "note",
        &goal_id,
        "needs a closer look",
        "--author",
        "refine-smoke",
    ]);
    fixture.assert_success("goal note", &note);
    assert_eq!(fixture.json_stdout(&note)["goal"]["id"], goal_id);
    let shown_after_note = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after note", &shown_after_note);
    let note_id = fixture.json_stdout(&shown_after_note)["goal"]["notes"][0]["id"]
        .as_str()
        .expect("goal show should expose note id")
        .to_string();
    let note_edit = fixture.run_refine(&[
        "goal",
        "note-edit",
        &goal_id,
        &note_id,
        "needs a closer look after edit",
    ]);
    fixture.assert_success("goal note-edit", &note_edit);
    let shown_after_note_edit = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after note edit", &shown_after_note_edit);
    assert_eq!(
        fixture.json_stdout(&shown_after_note_edit)["goal"]["notes"][0]["body"],
        "needs a closer look after edit"
    );
    let note_delete = fixture.run_refine(&["goal", "note-delete", &goal_id, &note_id]);
    fixture.assert_success("goal note-delete", &note_delete);
    let shown_after_note_delete = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after note delete", &shown_after_note_delete);
    assert_eq!(
        fixture.json_stdout(&shown_after_note_delete)["goal"]["notes"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    assert_eq!(fixture.goal_field(&goal_id, "round_count"), 0);
    let round = fixture.run_refine(&[
        "goal",
        "round",
        &goal_id,
        "--reporter",
        "refine-smoke",
        "--prompt",
        "Implement the desired behavior",
    ]);
    fixture.assert_success("goal round", &round);
    assert_eq!(fixture.goal_field(&goal_id, "round_count"), 1);

    let jira_export_path = fixture.app_root.join("goal-evidence.csv");
    let jira_export = fixture.run_refine(&[
        "goal",
        "export",
        &goal_id,
        "--output",
        jira_export_path.to_str().unwrap(),
    ]);
    fixture.assert_success("goal Jira export", &jira_export);
    let jira_csv = fs::read_to_string(&jira_export_path).unwrap();
    assert!(jira_csv.starts_with("Summary,Description,Work Type,Priority"));
    assert!(jira_csv.contains("Implement the desired behavior"));
    fs::remove_file(jira_export_path).unwrap();

    let delete = fixture.run_refine(&["goal", "delete", &goal_id]);
    fixture.assert_success("goal delete", &delete);
    let payload = fixture.json_stdout(&delete);
    assert_eq!(payload["deleted"], true);
    assert_eq!(payload["id"], goal_id);

    let after = fixture.run_refine(&["goal", "show", &goal_id]);
    assert!(!after.status.success());
    assert!(
        String::from_utf8_lossy(&after.stderr)
            .to_lowercase()
            .contains("not found")
    );
}

pub(crate) fn goal_feature_assignment_and_round_edit_latest(fixture: &IntegrationFixture) {
    let goal_id = fixture.create_goal("cli feature assignment goal");
    let feature = fixture.run_refine(&[
        "feature",
        "create",
        "cli assignment feature",
        "--description",
        "Feature used by the CLI assignment regression.",
        "--reporter",
        "refine-smoke",
    ]);
    fixture.assert_success("feature create assignment", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let assign = fixture.run_refine(&["goal", "assign-feature", &goal_id, &feature_id]);
    fixture.assert_success("goal assign-feature", &assign);
    assert_eq!(fixture.goal_field(&goal_id, "feature_id"), feature_id);

    let remove = fixture.run_refine(&["goal", "remove-feature", &goal_id]);
    fixture.assert_success("goal remove-feature", &remove);
    assert!(fixture.goal_field(&goal_id, "feature_id").is_null());

    let round = fixture.run_refine(&[
        "goal",
        "round",
        &goal_id,
        "--reporter",
        "refine-smoke",
        "--prompt",
        "first prompt",
    ]);
    fixture.assert_success("goal round assignment", &round);
    let edit = fixture.run_refine(&[
        "goal",
        "round",
        &goal_id,
        "--edit-latest",
        "--reporter",
        "refine-smoke",
        "--prompt",
        "edited prompt",
    ]);
    fixture.assert_success("goal round edit latest", &edit);
    let shown = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after round edit", &shown);
    let goal = fixture.json_stdout(&shown);
    assert_eq!(goal["goal"]["round_count"], 1);
    assert!(goal.to_string().contains("edited prompt"), "{goal:#}");

    fixture.assert_success(
        "goal delete assignment",
        &fixture.run_refine(&["goal", "delete", &goal_id]),
    );
    fixture.assert_success(
        "feature delete assignment",
        &fixture.run_refine(&["feature", "delete", &feature_id]),
    );
}

pub(crate) fn goal_cancel_uses_active_node_and_rejects_foreign_owner(fixture: &IntegrationFixture) {
    fixture.assert_success(
        "create cancel active node",
        &fixture.run_refine(&["node", "create", "cancel-active-node"]),
    );
    fixture.assert_success(
        "create cancel foreign node",
        &fixture.run_refine(&["node", "create", "cancel-foreign-node"]),
    );
    fixture.assert_success(
        "activate cancel active node",
        &fixture.run_refine(&["node", "activate", "cancel-active-node"]),
    );

    let owned_goal = fixture.create_goal("CLI owned cancellation");
    let note = fixture.run_refine(&[
        "goal",
        "note",
        &owned_goal,
        "active Node ownership confirmed",
        "--author",
        "Refine",
    ]);
    fixture.assert_success("goal note on active Node", &note);
    assert_eq!(
        fixture.json_stdout(&note)["goal"]["node_id"],
        "cancel-active-node"
    );
    let cancel = fixture.run_refine(&["goal", "cancel", &owned_goal]);
    fixture.assert_success("goal cancel on active Node", &cancel);
    assert_eq!(fixture.json_stdout(&cancel)["goal"]["status"], "cancelled");

    fixture.assert_success(
        "activate cancel foreign node",
        &fixture.run_refine(&["node", "activate", "cancel-foreign-node"]),
    );
    let foreign_goal = fixture.create_goal("CLI foreign cancellation");
    fixture.assert_success(
        "restore cancel active node",
        &fixture.run_refine(&["node", "activate", "cancel-active-node"]),
    );
    let rejected = fixture.run_refine(&["goal", "cancel", &foreign_goal]);
    assert!(!rejected.status.success(), "foreign-owned cancel succeeded");
    assert!(
        rejected.stdout.is_empty(),
        "foreign-owned cancel wrote stdout:\n{}",
        String::from_utf8_lossy(&rejected.stdout)
    );
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("is owned by node cancel-foreign-node, not active node cancel-active-node"),
        "stderr:\n{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert_eq!(fixture.goal_field(&foreign_goal, "status"), "backlog");

    fixture.assert_success(
        "restore default after cancel ownership regression",
        &fixture.run_refine(&["node", "activate", "default"]),
    );
}

pub(crate) fn goal_workflow_actions_start_retry_and_undo(fixture: &IntegrationFixture) {
    let started_id = fixture.create_goal("goal action start");
    let started = fixture.run_refine(&["goal", "start", &started_id]);
    fixture.assert_success("goal start", &started);
    assert_eq!(fixture.json_stdout(&started)["goal"]["status"], "todo");
    assert_eq!(fixture.goal_field(&started_id, "status"), "todo");
    fixture.assert_success(
        "goal cancel started",
        &fixture.run_refine(&["goal", "cancel", &started_id]),
    );
    fixture.assert_success(
        "goal undo started cancel",
        &fixture.run_refine(&["goal", "undo", &started_id]),
    );
    fixture.assert_success(
        "goal delete started",
        &fixture.run_refine(&["goal", "delete", &started_id]),
    );

    let quality_id = fixture.create_goal("goal action quality retry");
    seed_goal_status(fixture, &quality_id, "failed");
    let retried_quality = fixture.run_refine(&["goal", "retry", &quality_id, "--stage", "quality"]);
    fixture.assert_success("goal retry quality", &retried_quality);
    assert_eq!(
        fixture.json_stdout(&retried_quality)["goal"]["status"],
        "quality"
    );
    fixture.assert_success(
        "goal cancel quality retry",
        &fixture.run_refine(&["goal", "cancel", &quality_id]),
    );
    let undone_quality_cancel = fixture.run_refine(&["goal", "undo", &quality_id]);
    fixture.assert_success("goal undo quality cancel", &undone_quality_cancel);
    assert_eq!(
        fixture.json_stdout(&undone_quality_cancel)["goal"]["status"],
        "todo"
    );
    fixture.assert_success(
        "goal delete quality retry",
        &fixture.run_refine(&["goal", "delete", &quality_id]),
    );

    let governance_id = fixture.create_goal("goal action governance retry");
    seed_goal_status(fixture, &governance_id, "failed");
    let retried_governance =
        fixture.run_refine(&["goal", "retry", &governance_id, "--stage", "governance"]);
    fixture.assert_success("goal retry governance", &retried_governance);
    assert_eq!(
        fixture.json_stdout(&retried_governance)["goal"]["status"],
        "governance"
    );
    let retired_merge = fixture.run_refine(&["goal", "merge", &governance_id]);
    assert!(
        !retired_merge.status.success(),
        "retired goal merge command unexpectedly remained available"
    );
    fixture.assert_success(
        "goal cancel governance retry",
        &fixture.run_refine(&["goal", "cancel", &governance_id]),
    );
    fixture.assert_success(
        "goal delete governance retry",
        &fixture.run_refine(&["goal", "delete", &governance_id]),
    );

    let cancelled_id = fixture.create_goal("goal action undo cancelled");
    let cancelled = fixture.run_refine(&["goal", "cancel", &cancelled_id]);
    fixture.assert_success("goal cancel for undo", &cancelled);
    let reopened = fixture.run_refine(&["goal", "undo", &cancelled_id]);
    fixture.assert_success("goal undo cancelled", &reopened);
    assert_eq!(fixture.json_stdout(&reopened)["goal"]["status"], "todo");
    fixture.assert_success(
        "goal delete cancel undo",
        &fixture.run_refine(&["goal", "delete", &cancelled_id]),
    );
}

pub(crate) fn seed_goal_status(fixture: &IntegrationFixture, goal_id: &str, status: &str) {
    let payload = fixture.api_json(
        "POST",
        "/api/goals/bulk",
        serde_json::json!({
            "selected_ids": [goal_id],
            "exclude_ids": [],
            "update": {
                "status": status
            }
        }),
    );
    assert_eq!(payload["updated"], 1, "{payload:#}");
    assert_eq!(fixture.goal_field(goal_id, "status"), status);
}

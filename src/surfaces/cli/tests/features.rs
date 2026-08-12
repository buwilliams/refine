use super::*;

#[test]
fn feature_create_list_show_and_membership_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-feature-membership");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Goal One",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "create",
            "Feature One",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "add-goal",
            "FEA1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "show",
            "FEA1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "list",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let assigned = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(assigned.contains("\"feature_id\": \"FEA1\""));
    assert!(assigned.contains("\"feature_order\": null"));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "unorder-goal",
            "FEA1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let unordered = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(unordered.contains("\"feature_order\": null"));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "order-goal",
            "FEA1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let ordered = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(ordered.contains("\"feature_order\": 1"));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "remove-goal",
            "FEA1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let removed = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(removed.contains("\"feature_id\": null"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_reorder_and_move_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-feature-reorder-move");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    for (id, name) in [("GOAL1", "Goal One"), ("GOAL2", "Goal Two")] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "goal",
                "create",
                name,
                "--target-root",
                target_root.to_str().unwrap(),
                "--id",
                id,
            ])
            .unwrap(),
        )
        .unwrap();
    }
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "create",
            "Feature One",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();
    for goal_id in ["GOAL1", "GOAL2"] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "add-goal",
                "FEA1",
                goal_id,
                "--target-root",
                target_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
    }
    for goal_id in ["GOAL1", "GOAL2"] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "order-goal",
                "FEA1",
                goal_id,
                "--target-root",
                target_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
    }
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "reorder-goal",
            "FEA1",
            "GOAL2",
            "1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL2/goal.json"))
            .unwrap()
            .contains("\"feature_order\": 1")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "move",
            "FEA1",
            "todo",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_cancel_and_delete_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-feature-cancel-delete");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    for (id, name) in [("GOAL1", "Goal One"), ("GOAL2", "Goal Two")] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "goal",
                "create",
                name,
                "--target-root",
                target_root.to_str().unwrap(),
                "--id",
                id,
            ])
            .unwrap(),
        )
        .unwrap();
    }
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "create",
            "Feature One",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();
    for goal_id in ["GOAL1", "GOAL2"] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "add-goal",
                "FEA1",
                goal_id,
                "--target-root",
                target_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
    }

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "cancel",
            "FEA1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"status\": \"cancelled\"")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "delete",
            "FEA1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(!refine_dir.join("features/FE/A1/feature.json").exists());
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(!refine_dir.join("goals/GO/AL2/goal.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_import_uses_shared_import_service() {
    let temp_root = unique_temp_dir("cli-feature-import");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "create",
            "Imported Feature",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();
    let csv = temp_root.join("import.csv");
    fs::write(
        &csv,
        "prompt,reporter,priority\nFix the broken flow,QA,high\n",
    )
    .unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "import",
            "--target-root",
            target_root.to_str().unwrap(),
            "--file",
            csv.to_str().unwrap(),
            "--csv",
            "--feature-id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();

    let snapshot = FileProjectProjectionStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let goal = snapshot.goals.values().next().unwrap();
    assert_eq!(goal.goal.feature_id.as_deref(), Some("FEA1"));
    assert_eq!(goal.goal.priority.as_str(), "high");
    assert_eq!(goal.goal.reporter.as_deref(), Some("QA"));

    fs::remove_dir_all(temp_root).unwrap();
}

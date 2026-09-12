use super::*;

#[test]
fn goal_round_append_and_edit_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-goal-rounds");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Round Goal",
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
            "goal",
            "round",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
            "--reporter",
            "Reporter",
            "--prompt",
            "Initial prompt",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "round",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
            "--edit-latest",
            "--reporter",
            "Reviewer",
            "--prompt",
            "Revised prompt",
        ])
        .unwrap(),
    )
    .unwrap();

    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));
    assert!(written.contains("\"prompt\": \"Revised prompt\""));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn goal_round_delete_uses_one_based_number_and_revision() {
    let root = unique_temp_dir("cli-round-delete");
    let service = crate::application::work_items::FileWorkItemService::new(root.join(".refine"));
    service
        .create_goal_summary("Delete retry", Some("GOAL1"))
        .unwrap();
    for prompt in ["original", "failed retry"] {
        service
            .append_goal_round_summary("GOAL1", "User", prompt)
            .unwrap();
    }
    assert!(Cli::try_parse_from(["refine", "goal", "round-delete", "GOAL1", "0"]).is_err());
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "round-delete",
            "GOAL1",
            "2",
            "--target-root",
            root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let goal = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(goal["rounds"][0]["prompt"], "original");
    assert_eq!(goal["status"], "backlog");
    fs::remove_dir_all(root).unwrap();
}

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

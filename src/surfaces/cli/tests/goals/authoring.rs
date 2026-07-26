use super::*;

#[test]
fn goal_draft_cli_builds_the_shared_plan_goal_extraction_request() {
    let parsed = Cli::try_parse_from([
        "refine",
        "goal",
        "draft",
        "--text",
        "Plan one independently actionable slice.",
        "--reporter",
        "Buddy",
        "--provider",
        "smoke-ai",
    ])
    .unwrap();
    let Commands::Goal {
        action:
            GoalAction::Draft {
                text,
                file,
                reporter,
                provider,
                ..
            },
    } = parsed.command
    else {
        panic!("expected goal draft command");
    };
    let body = plan_goal_draft_body(text, file, reporter, provider).unwrap();
    assert_eq!(body["purpose"], "plan_goal");
    assert_eq!(body["text"], "Plan one independently actionable slice.");
    assert_eq!(body["reporter"], "Buddy");
    assert_eq!(body["provider"], "smoke-ai");
}

#[test]
fn goal_draft_cli_requires_exactly_one_nonempty_plan_source() {
    let missing = plan_goal_draft_body(None, None, None, None).unwrap_err();
    assert_eq!(missing.to_string(), "goal draft requires --text or --file");

    let empty = plan_goal_draft_body(Some("  ".to_string()), None, None, None).unwrap_err();
    assert_eq!(
        empty.to_string(),
        "goal draft Plan transcript cannot be empty"
    );

    let both = plan_goal_draft_body(
        Some("Plan".to_string()),
        Some(PathBuf::from("plan.md")),
        None,
        None,
    )
    .unwrap_err();
    assert_eq!(
        both.to_string(),
        "goal draft accepts either --text or --file, not both"
    );
}

#[test]
fn goal_create_list_show_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-goal-create");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");

    let create = Cli::try_parse_from([
        "refine",
        "goal",
        "create",
        "CLI Goal",
        "--target-root",
        target_root.to_str().unwrap(),
        "--id",
        "GOAL1",
    ])
    .unwrap();
    dispatch(create).unwrap();

    let list = Cli::try_parse_from([
        "refine",
        "goal",
        "list",
        "--target-root",
        target_root.to_str().unwrap(),
    ])
    .unwrap();
    dispatch(list).unwrap();

    let show = Cli::try_parse_from([
        "refine",
        "goal",
        "show",
        "GOAL1",
        "--target-root",
        target_root.to_str().unwrap(),
    ])
    .unwrap();
    dispatch(show).unwrap();

    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"name\": \"CLI Goal\""));
    fs::remove_dir_all(temp_root).unwrap();
}

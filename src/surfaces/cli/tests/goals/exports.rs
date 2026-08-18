use super::*;

#[test]
fn jira_export_worker_cli_process_helper() {
    let (Ok(runtime_root), Ok(operation_id)) = (
        std::env::var("REFINE_TEST_JIRA_RUNTIME_ROOT"),
        std::env::var("REFINE_TEST_JIRA_OPERATION_ID"),
    ) else {
        return;
    };
    let delay = std::env::var("REFINE_TEST_JIRA_WORKER_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    thread::sleep(std::time::Duration::from_millis(delay));
    let cli = Cli::try_parse_from([
        "refine",
        "system",
        "runner-worker",
        "--kind",
        "jira-export",
        "--port-runtime-root",
        &runtime_root,
        "--operation-id",
        &operation_id,
    ])
    .unwrap();
    dispatch(cli).unwrap();
}

#[test]
fn goal_export_cli_accepts_stdout_or_file_delivery() {
    let stdout = Cli::try_parse_from(["refine", "goal", "export", "GOAL1"]).unwrap();
    let Commands::Goal {
        action: GoalAction::Export { id, output, .. },
    } = stdout.command
    else {
        panic!("expected Goal export command");
    };
    assert_eq!(id, "GOAL1");
    assert_eq!(output, None);

    let file = Cli::try_parse_from([
        "refine",
        "goal",
        "export",
        "GOAL1",
        "--output",
        "evidence.csv",
    ])
    .unwrap();
    let Commands::Goal {
        action: GoalAction::Export { output, .. },
    } = file.command
    else {
        panic!("expected Goal export command");
    };
    assert_eq!(output, Some(PathBuf::from("evidence.csv")));
}

#[test]
fn goal_export_cli_writes_shared_jira_csv() {
    let temp_root = unique_temp_dir("cli-goal-jira-export");
    let refine_dir = temp_root.join(".refine");
    let output = temp_root.join("jira.csv");
    let service = crate::application::work_items::FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("CLI evidence export", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Auditor", "Export the evidence")
        .unwrap();
    service
        .update_latest_goal_round_implementation_report("GOAL1", "The export is complete.")
        .unwrap();

    dispatch(Cli {
        command: Commands::Goal {
            action: GoalAction::Export {
                id: "GOAL1".to_string(),
                target_root: Some(temp_root.clone()),
                output: Some(output.clone()),
            },
        },
    })
    .unwrap();

    let csv = fs::read_to_string(output).unwrap();
    assert!(csv.starts_with("Summary,Description,Work Type,Priority"));
    assert!(csv.contains("The export is complete."));
    fs::remove_dir_all(temp_root).unwrap();
}

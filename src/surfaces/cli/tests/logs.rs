use super::*;

#[test]
fn log_commands_use_shared_activity_service() {
    let temp_root = unique_temp_dir("cli-log-activity");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    let service = FileActivityService::new(&refine_dir);
    let first = service.new_entry(
        "Build failed",
        "error",
        "quality",
        Some("GOAL1".to_string()),
        Some("agent".to_string()),
    );
    let first_id = first.id.clone();
    service.append(first).unwrap();
    service
        .append(service.new_entry("Build passed", "info", "quality", None, None))
        .unwrap();

    for argv in [
        vec![
            "refine",
            "log",
            "list",
            "--target-root",
            target_root.to_str().unwrap(),
            "--limit",
            "2",
        ],
        vec![
            "refine",
            "log",
            "tail",
            "--target-root",
            target_root.to_str().unwrap(),
            "--limit",
            "1",
        ],
        vec![
            "refine",
            "log",
            "query",
            "failed",
            "--target-root",
            target_root.to_str().unwrap(),
            "--severity",
            "error",
            "--goal-id",
            "GOAL1",
        ],
        vec![
            "refine",
            "log",
            "show",
            first_id.as_str(),
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "log",
            "export",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    fs::remove_dir_all(temp_root).unwrap();
}

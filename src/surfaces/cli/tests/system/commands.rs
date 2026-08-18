use super::*;

#[test]
fn project_and_system_doctor_and_migrate_use_observability_services() {
    let temp_root = unique_temp_dir("cli-doctor-migrate");
    let target_root = temp_root.clone();
    let runtime_root = temp_root.join("run");
    fs::create_dir_all(&target_root).unwrap();

    for argv in [
        vec![
            "refine",
            "project",
            "doctor",
            "--target-root",
            target_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--repo-root",
            temp_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "system",
            "doctor",
            "--target-root",
            target_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--repo-root",
            temp_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "project",
            "migrate",
            "--target-root",
            target_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn system_ps_lists_and_stops_nested_agent_processes() {
    let temp_root = unique_temp_dir("cli-system-ps");
    let runtime_root = temp_root.join("run");
    let target_root = temp_root.join("target");
    fs::create_dir_all(&target_root).unwrap();
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("CLI process stop", Some("GOAL-NESTED"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL-NESTED", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-NESTED", GoalStatus::Plan)
        .unwrap();
    let port = 19091;
    let port_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(port);
    fs::create_dir_all(&port_root).unwrap();
    fs::write(
        port_root.join("apps.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "active_app": target_root.display().to_string(),
            "apps": {}
        }))
        .unwrap(),
    )
    .unwrap();
    let supervisor = FileProcessSupervisor::new(&port_root);
    let agent_supervisor = FileProcessSupervisor::new(port_root.join("agents"));
    supervisor
        .register(ManagedProcess {
            id: "running-helper".to_string(),
            owner: ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("helper".to_string()),
            details: Some("{\"kind\":\"ui\"}".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let stoppable = agent_supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: if cfg!(windows) { "cmd" } else { "sleep" }.to_string(),
            args: if cfg!(windows) {
                vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()]
            } else {
                vec!["30".to_string()]
            },
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::Map::from_iter([(
                "goal_id".to_string(),
                serde_json::json!("GOAL-NESTED"),
            )]),
        })
        .unwrap();

    let listed = system_ps_response(runtime_root.clone(), Some(port), None, "terminate").unwrap();
    assert_eq!(listed["process_count"], 2);
    assert!(
        listed["processes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|process| process["id"] == "running-helper" && process["port"] == port)
    );
    assert!(
        listed["processes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|process| process["id"] == "running-helper"
                && process["details"] == "{\"kind\":\"ui\"}")
    );
    assert!(
        listed["processes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|process| process["id"] == stoppable.id
                && process["kind"] == "agent"
                && process["goal_id"] == "GOAL-NESTED"
                && process["port"] == port)
    );

    let stopped = system_ps_response(
        runtime_root.clone(),
        Some(port),
        Some(&stoppable.id),
        "terminate",
    )
    .unwrap();
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["process"]["id"], stoppable.id);
    assert_eq!(stopped["process"]["status"], "stopped");
    assert_eq!(stopped["termination"]["confirmed_exit"], true);
    assert_eq!(stopped["goal"]["id"], "GOAL-NESTED");
    assert_eq!(stopped["goal"]["status"], "failed");
    assert_eq!(stopped["goal_failed"], true);
    assert_eq!(stopped["goal_requeued"], false);
    assert_eq!(stopped["worktrees_retained"], true);
    assert!(agent_supervisor.inspect(&stoppable.id).is_err());
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-NESTED")
            .unwrap()
            .goal
            .status,
        GoalStatus::Failed
    );

    work_items
        .create_goal_summary("CLI cancelled process stop", Some("GOAL-NESTED-CANCELLED"))
        .unwrap();
    work_items
        .cancel_goal_summary("GOAL-NESTED-CANCELLED")
        .unwrap();
    let cancelled_process = agent_supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: if cfg!(windows) { "cmd" } else { "sleep" }.to_string(),
            args: if cfg!(windows) {
                vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()]
            } else {
                vec!["30".to_string()]
            },
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::Map::from_iter([(
                "goal_id".to_string(),
                serde_json::json!("GOAL-NESTED-CANCELLED"),
            )]),
        })
        .unwrap();
    let cancelled_stop = system_ps_response(
        runtime_root.clone(),
        Some(port),
        Some(&cancelled_process.id),
        "terminate",
    )
    .unwrap();
    assert_eq!(cancelled_stop["stopped"], true);
    assert_eq!(cancelled_stop["goal"]["status"], "cancelled");
    assert_eq!(cancelled_stop["goal_requeued"], false);
    assert_eq!(cancelled_stop["worktrees_retained"], true);
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-NESTED-CANCELLED")
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );

    let missing = system_ps_response(
        runtime_root.clone(),
        Some(port),
        Some(&stoppable.id),
        "terminate",
    )
    .unwrap_err();
    assert!(
        matches!(
            &missing,
            crate::error::RefineError::NotFound(message)
                if message.contains(&stoppable.id)
        ),
        "{missing}"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

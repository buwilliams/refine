use super::dispatch::{
    absolute_cli_path, dispatch, explicit_target_root_path, plan_goal_draft_body, run_system_start,
    system_ps_response, system_status_response,
};
use super::*;

#[test]
fn agent_open_parses_goal_instances_and_singleton_profiles() {
    let goal = Cli::try_parse_from(["refine", "agent", "open", "GOAL1"]).unwrap();
    assert!(matches!(
        goal.command,
        Commands::Agent {
            action: AgentAction::Open {
                goal_id: Some(ref goal_id),
                profile: CliAgentProfile::Goal,
                prompt: None,
            }
        } if goal_id == "GOAL1"
    ));

    let plan = Cli::try_parse_from([
        "refine",
        "agent",
        "open",
        "--profile",
        "plan",
        "--prompt",
        "Design retries",
    ])
    .unwrap();
    assert!(matches!(
        plan.command,
        Commands::Agent {
            action: AgentAction::Open {
                goal_id: None,
                profile: CliAgentProfile::Plan,
                prompt: Some(ref prompt),
            }
        } if prompt == "Design retries"
    ));
}
use crate::model::log::LogEntry;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
    ProcessSupervisor, managed_pid_is_alive,
};
use crate::process::supervisor::lifecycle::{DaemonLifecycleService, FileDaemonLifecycleService};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::observability::activity::ActivityService;
use crate::tools::observability::activity::FileActivityService;
use crate::tools::product::project_state::PROJECTION_SNAPSHOT_FILE;
use crate::tools::product::project_state::{FileProjectStateStore, ProjectStateStore};
use crate::tools::product::work_items::FileWorkItemService;
use clap::Parser;
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn explicit_target_root_path_detects_internal_cli_escape_hatch() {
    let target_root = PathBuf::from("/tmp/refine-state");
    let command = Commands::Goal {
        action: GoalAction::Create {
            name: "direct write".to_string(),
            target_root: Some(target_root.clone()),
            id: None,
        },
    };
    assert_eq!(explicit_target_root_path(&command), Some(&target_root));

    let default_daemon_command = Commands::Workflow {
        action: WorkflowAction::Pause {
            runtime_root: PathBuf::from("run"),
        },
    };
    assert_eq!(explicit_target_root_path(&default_daemon_command), None);
}

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
    let service = crate::tools::product::work_items::FileWorkItemService::new(&refine_dir);
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

#[test]
fn system_start_resolves_relative_runtime_root_before_spawning_daemon() {
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(
        absolute_cli_path(PathBuf::from("run")).unwrap(),
        cwd.join("run")
    );
    assert_eq!(
        absolute_cli_path(cwd.join("already-absolute")).unwrap(),
        cwd.join("already-absolute")
    );
}

#[test]
fn default_static_root_finds_checkout_assets() {
    let root = super::helpers::default_static_root().expect("static root should exist");
    assert!(root.join("index.html").is_file());
}

#[test]
fn project_sync_rebuilds_projection_from_cli_surface() {
    let temp_root = unique_temp_dir("cli-project-sync");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    let goal_dir = refine_dir.join("goals").join("01").join("GOAL1");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    fs::create_dir_all(&goal_dir).unwrap();
    fs::write(
        goal_dir.join("goal.json"),
        r#"{
              "id": "GOAL1",
              "name": "CLI visible Goal",
              "status": "done",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "rounds": []
            }"#,
    )
    .unwrap();

    let cli = Cli::try_parse_from([
        "refine",
        "project",
        "sync",
        "--target-root",
        target_root.to_str().unwrap(),
        "--cache-dir",
        cache_dir.to_str().unwrap(),
    ])
    .unwrap();
    dispatch(cli).unwrap();

    assert!(cache_dir.join(PROJECTION_SNAPSHOT_FILE).exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn system_start_owns_foreground_web_options() {
    let parsed = Cli::try_parse_from([
        "refine",
        "system",
        "start",
        "--port",
        "0",
        "--runtime-root",
        "run",
        "--once",
    ])
    .unwrap();
    let Commands::System {
        action:
            SystemAction::Start {
                port,
                bind_address,
                runtime_root,
                once,
                foreground,
                ..
            },
    } = parsed.command
    else {
        panic!("expected system start command");
    };
    assert_eq!(port, 0);
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    assert_eq!(runtime_root, PathBuf::from("run"));
    assert!(once);
    assert!(!foreground);

    let parsed = Cli::try_parse_from([
        "refine",
        "system",
        "start",
        "--bind-address",
        "0.0.0.0",
        "--once",
    ])
    .unwrap();
    let Commands::System {
        action: SystemAction::Start { bind_address, .. },
    } = parsed.command
    else {
        panic!("expected system start command");
    };
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::UNSPECIFIED));

    assert!(Cli::try_parse_from(["refine", "system", "web"]).is_err());
    assert!(Cli::try_parse_from(["refine", "system", "web", "--target-root", ".refine"]).is_err());
    assert!(Cli::try_parse_from(["refine", "system", "serve", "--once"]).is_err());
    assert!(Cli::try_parse_from(["refine", "system", "start", "--token", "secret"]).is_err());
}

#[test]
fn website_command_owns_static_site_options() {
    let parsed = Cli::try_parse_from(["refine", "website"]).unwrap();
    let Commands::Website { bind_address, .. } = parsed.command else {
        panic!("expected website command");
    };
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::LOCALHOST));

    let parsed = Cli::try_parse_from([
        "refine",
        "website",
        "--port",
        "0",
        "--bind-address",
        "0.0.0.0",
        "--static-root",
        ".",
        "--once",
    ])
    .unwrap();
    let Commands::Website {
        port,
        bind_address,
        static_root,
        once,
    } = parsed.command
    else {
        panic!("expected website command");
    };
    assert_eq!(port, 0);
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    assert_eq!(static_root, PathBuf::from("."));
    assert!(once);
    assert_eq!(
        explicit_target_root_path(&Commands::Website {
            port,
            bind_address,
            static_root,
            once,
        }),
        None
    );
}

#[test]
fn system_lifecycle_commands_default_to_8082() {
    for (verb, expected) in [
        ("start", "Start"),
        ("stop", "Stop"),
        ("restart", "Restart"),
        ("status", "Status"),
    ] {
        let parsed = Cli::try_parse_from(["refine", "system", verb]).unwrap();
        let Commands::System { action } = parsed.command else {
            panic!("expected system command");
        };
        let port = match action {
            SystemAction::Start { port, .. }
            | SystemAction::Stop { port, .. }
            | SystemAction::Restart { port, .. }
            | SystemAction::Status { port, .. } => port,
            other => panic!("expected {expected} action, got {other:?}"),
        };
        assert_eq!(port, 8082, "{expected} should default to port 8082");
    }
}

#[test]
fn project_registry_commands_use_shared_file_project_registry_service() {
    let temp_root = unique_temp_dir("cli-project-registry");
    let runtime_root = temp_root.join("run");
    let app_one = temp_root.join("app-one");
    let app_two = temp_root.join("app-two");
    fs::create_dir_all(app_one.join(".refine")).unwrap();
    fs::create_dir_all(app_two.join(".refine")).unwrap();
    git_init(&app_one);
    git_init(&app_two);

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "status",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--target-root",
            app_one.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let registry_path = runtime_root.join("apps.json");
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert_eq!(registry["active_app"], app_one.to_str().unwrap());

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "detach",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert!(registry["active_app"].is_null());

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "attach",
            app_one.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "register",
            "second",
            app_two.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "switch",
            "second",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert_eq!(registry["active_app"], app_two.to_str().unwrap());

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "remove",
            "second",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&registry_path).unwrap()).unwrap();
    assert!(registry["apps"].get(app_two.to_str().unwrap()).is_none());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_clone_uses_shared_file_project_registry_service() {
    let temp_root = unique_temp_dir("cli-project-clone");
    let runtime_root = temp_root.join("run");
    let source = temp_root.join("source");
    let destination = temp_root.join("clone-destination");
    fs::create_dir_all(&source).unwrap();
    let output = std::process::Command::new("git")
        .arg("init")
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "clone",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            "--name",
            "cloned",
            "--make-current",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    assert!(destination.join(".git").exists());
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(runtime_root.join("apps.json")).unwrap()).unwrap();
    assert_eq!(registry["active_app"], destination.to_str().unwrap());
    assert_eq!(
        registry["apps"][destination.to_str().unwrap()]["name"],
        "cloned"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_attach_creates_missing_local_project() {
    let temp_root = unique_temp_dir("cli-project-create-local");
    let runtime_root = temp_root.join("run");
    let destination = temp_root.join("new-app");

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "attach",
            destination.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    assert!(destination.join(".git").exists());
    assert!(
        refine_dir_for_target_root(&destination)
            .unwrap()
            .join("refine.json")
            .exists()
    );
    assert!(!destination.join(".refine").exists());
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(runtime_root.join("apps.json")).unwrap()).unwrap();
    assert_eq!(registry["active_app"], destination.to_str().unwrap());

    fs::remove_dir_all(temp_root).unwrap();
}

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
fn project_attach_requires_an_agent_for_legacy_refine_migration() {
    let temp_root = unique_temp_dir("cli-project-migration");
    let runtime_root = temp_root.join("run");
    let app_root = temp_root.join("legacy-app");
    let target_root = app_root.clone();
    let refine_dir = target_root.join(".refine");
    fs::create_dir_all(refine_dir.join("gaps/GA")).unwrap();
    fs::write(refine_dir.join("gaps/GA/gap.json"), "{}").unwrap();

    let attach_error = dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "attach",
            app_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap_err()
    .to_string();
    assert!(attach_error.contains("migration agent"));
    assert!(!refine_dir.join("refine.json").exists());
    assert!(refine_dir.join("gaps/GA/gap.json").exists());

    let migrate_error = dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "migrate",
            "--target-root",
            target_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap_err()
    .to_string();
    assert!(migrate_error.contains("migration agent"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn system_install_repair_and_uninstall_use_installation_service() {
    let temp_root = unique_temp_dir("cli-installation");
    let runtime_root = temp_root.join("run");

    for argv in [
        ["refine", "system", "install"],
        ["refine", "system", "repair"],
        ["refine", "system", "rollback"],
        ["refine", "system", "uninstall"],
    ] {
        assert!(Cli::try_parse_from(argv).is_err());
    }

    for argv in [
        vec![
            "refine",
            "system",
            "install",
            "--port",
            "4557",
            "--target",
            "linux-cli-web",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.0.0",
        ],
        vec![
            "refine",
            "system",
            "repair",
            "--port",
            "4557",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.0.0",
        ],
        vec![
            "refine",
            "system",
            "uninstall",
            "--port",
            "4557",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.0.0",
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(runtime_root.join("4557").join("install-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["status"]["installed"], false);
    assert_eq!(state["status"]["port"], 4557);
    assert_eq!(state["status"]["version"], "1.0.0");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn system_update_no_longer_accepts_metadata_version_argument() {
    let err = Cli::try_parse_from([
        "refine",
        "system",
        "update",
        "1.1.0",
        "--runtime-root",
        "run",
    ])
    .unwrap_err();

    assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);

    Cli::try_parse_from(["refine", "system", "update", "--runtime-root", "run"]).unwrap();
}

#[test]
fn system_status_reports_current_version_and_running_ports() {
    let temp_root = unique_temp_dir("cli-system-status");
    let runtime_root = temp_root.join("run");
    let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
        root: runtime_root.clone(),
    });
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let live_port = listener.local_addr().unwrap().port();
    let probe_thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 512];
        let _ = stream.read(&mut buffer);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
    });
    let stale_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let stale_port = stale_listener.local_addr().unwrap().port();
    drop(stale_listener);

    lifecycle.start(live_port).unwrap();
    lifecycle.start(stale_port).unwrap();
    lifecycle.start(4556).unwrap();
    lifecycle.stop(4556).unwrap();
    FileProcessSupervisor::new(
        RuntimeRoot {
            root: runtime_root.clone(),
        }
        .port_root(live_port),
    )
    .register(ManagedProcess {
        id: "helper-1".to_string(),
        owner: ProcessOwner::UserHelper,
        pid: Some(std::process::id()),
        state: "running".to_string(),
        label: Some("helper".to_string()),
        details: Some("{\"kind\":\"ui\",\"secret\":\"not-for-status\"}".to_string()),
        stdout_path: None,
        stderr_path: None,
        stdin_path: None,
        limits: None,
        started_at: String::new(),
        exit_code: None,
    })
    .unwrap();
    fs::create_dir_all(runtime_root.join("not-a-port")).unwrap();

    let status = system_status_response(runtime_root).unwrap();
    probe_thread.join().unwrap();
    assert_eq!(status["product"], "refine");
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["current_version"], env!("CARGO_PKG_VERSION"));
    assert!(status["launch_mode"].is_string());
    assert!(status["executable_path"].is_string());
    assert_eq!(status["running_ports"], serde_json::json!([live_port]));
    assert_eq!(status["ports"].as_array().unwrap().len(), 1);
    assert_eq!(status["ports"][0]["port"], live_port);
    assert!(status["ports"][0]["launch_mode"].is_string());
    assert!(status["ports"][0]["executable_path"].is_string());
    assert!(status["ports"][0]["daemon_healthy"].as_bool().unwrap());
    assert_eq!(status["ports"][0]["process_count"], 1);
    let process = status["ports"][0]["processes"][0].as_object().unwrap();
    assert_eq!(process.len(), 3);
    assert!(process.contains_key("pid"));
    assert!(process.contains_key("status"));
    assert!(process.contains_key("label"));
    assert_eq!(process["pid"], serde_json::json!(std::process::id()));
    assert_eq!(process["status"], "running");
    assert_eq!(process["label"], "helper");
    assert!(status["ports"][0].get("process_summary").is_none());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn system_start_recovers_jira_export_and_public_retry_launches_supervised_worker() {
    let temp_root = unique_temp_dir("cli-system-start-jira-recovery");
    let runtime_root = temp_root.join("run");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&target_root).unwrap();
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    FileWorkItemService::new(&refine_dir)
        .create_goal_summary("Restart-safe Jira export", Some("GOAL1"))
        .unwrap();

    let port_probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = port_probe.local_addr().unwrap().port();
    drop(port_probe);
    let port_runtime_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(port);
    let registry = FileOperationRegistry::new(&port_runtime_root);
    let interrupted = registry
        .register_with_request(
            "goals:jira-export",
            serde_json::json!({
                "refine_dir": refine_dir,
                "target_root": target_root,
                "selection": {"selected_ids": ["GOAL1"], "exclude_ids": [], "filter": {}}
            }),
        )
        .unwrap();
    registry
        .append_log(
            &interrupted.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "operation".to_string(),
                message: "Durable log before production restart".to_string(),
                details: None,
                actions: Vec::new(),
                actor: None,
                goal_id: None,
            },
        )
        .unwrap();
    let supervisor = FileProcessSupervisor::new(&port_runtime_root);
    let worker = supervisor
        .launch(cli_operation_helper_process_spec(&interrupted.id))
        .unwrap();
    let worker_pid = worker.pid.unwrap();
    assert!(managed_pid_is_alive(worker_pid).unwrap());

    // Model an abrupt daemon exit: the durable registration and its worker are both still live
    // when the production startup path begins recovery. Deliberately do not call lifecycle.stop.
    assert!(managed_pid_is_alive(worker_pid).unwrap());
    assert_eq!(
        registry.status(&interrupted.id).unwrap().state,
        OperationState::Running
    );

    let start_runtime_root = runtime_root.clone();
    let server = thread::spawn(move || {
        run_system_start(
            port,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            None,
            None,
            start_runtime_root,
            true,
            true,
        )
    });
    let interrupted_after_start =
        wait_for_cli_operation_state(&registry, &interrupted.id, OperationState::Interrupted);
    wait_for_cli_managed_pid_exit(worker_pid);
    assert!(!managed_pid_is_alive(worker_pid).unwrap());
    assert_eq!(
        interrupted_after_start.error.unwrap()["code"],
        "operation_interrupted"
    );
    assert!(
        supervisor
            .list()
            .unwrap()
            .iter()
            .all(|process| process.id != worker.id),
        "startup recovery must remove the original worker before exposing retry"
    );

    let mut stream = connect_to_cli_test_daemon(port);
    let retry_path = format!("/api/goals/export/jira/{}/retry", interrupted.id);
    let request = format!(
        "POST {retry_path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    server.join().unwrap().unwrap();
    assert!(response.starts_with("HTTP/1.1 202"), "{response}");
    let response_body = response.split("\r\n\r\n").nth(1).unwrap();
    let response_body: serde_json::Value = serde_json::from_str(response_body).unwrap();
    let recovered_id = response_body["operation"]["id"].as_str().unwrap();
    let recovered_processes = supervisor
        .list()
        .unwrap()
        .into_iter()
        .filter(|process| {
            process.details.as_deref().is_some_and(|details| {
                serde_json::from_str::<serde_json::Value>(details)
                    .ok()
                    .and_then(|details| details["operation_id"].as_str().map(str::to_string))
                    .as_deref()
                    == Some(recovered_id)
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        recovered_processes.len(),
        1,
        "public retry must launch exactly one replacement worker"
    );
    let recovered_process = &recovered_processes[0];
    assert!(managed_pid_is_alive(recovered_process.pid.unwrap()).unwrap());
    let recovered =
        wait_for_cli_operation_state(&registry, recovered_id, OperationState::Succeeded);
    assert_eq!(recovered.request["recovery_of"], interrupted.id);
    assert_eq!(recovered.result["export"]["goal_count"], 1);
    wait_for_cli_operation_log(&registry, &interrupted.id, "Operation interrupted");
    let (original_logs, _, _) = registry.page_logs(&interrupted.id, 20, 0).unwrap();
    assert!(
        original_logs
            .iter()
            .any(|entry| entry.message == "Durable log before production restart")
    );
    assert!(
        original_logs
            .iter()
            .any(|entry| entry.message == "Operation interrupted")
    );
    assert!(
        original_logs
            .iter()
            .any(|entry| entry.message == "Recovery terminating managed process")
    );
    assert!(
        original_logs
            .iter()
            .any(|entry| entry.message == "Recovery confirmed managed process exit")
    );
    wait_for_cli_operation_log(&registry, recovered_id, "Jira CSV export completed");
    let (recovered_logs, _, _) = registry.page_logs(recovered_id, 20, 0).unwrap();
    assert!(
        recovered_logs
            .iter()
            .any(|entry| entry.message == "Jira CSV export completed")
    );
    wait_for_cli_managed_pid_exit(recovered_process.pid.unwrap());

    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(not(windows))]
#[test]
fn system_start_refuses_jira_retry_when_orphan_exit_cannot_be_confirmed() {
    let temp_root = unique_temp_dir("cli-system-start-jira-recovery-attention");
    let runtime_root = temp_root.join("run");
    let target_root = temp_root.join("app");
    fs::create_dir_all(&target_root).unwrap();
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();

    let port_probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = port_probe.local_addr().unwrap().port();
    drop(port_probe);
    let port_runtime_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(port);
    let registry = FileOperationRegistry::new(&port_runtime_root);
    let operation = registry
        .register_with_request(
            "goals:jira-export",
            serde_json::json!({
                "refine_dir": refine_dir,
                "target_root": target_root,
                "selection": {"selected_ids": [], "exclude_ids": [], "filter": {}}
            }),
        )
        .unwrap();
    registry
        .append_log(
            &operation.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "operation".to_string(),
                message: "Evidence retained before failed recovery".to_string(),
                details: None,
                actions: Vec::new(),
                actor: None,
                goal_id: None,
            },
        )
        .unwrap();
    let supervisor = FileProcessSupervisor::new(&port_runtime_root);
    let ready_path = temp_root.join("stubborn-worker-ready");
    let worker = supervisor
        .launch(cli_stubborn_operation_helper_process_spec(
            &operation.id,
            &ready_path,
        ))
        .unwrap();
    let worker_pid = worker.pid.unwrap();
    let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !ready_path.exists() && std::time::Instant::now() < ready_deadline {
        thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        ready_path.exists(),
        "stubborn recovery helper did not start"
    );
    assert!(managed_pid_is_alive(worker_pid).unwrap());

    let start_runtime_root = runtime_root.clone();
    let server = thread::spawn(move || {
        run_system_start(
            port,
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            None,
            None,
            start_runtime_root,
            true,
            true,
        )
    });
    let failed = wait_for_cli_operation_state(&registry, &operation.id, OperationState::Failed);
    assert_eq!(
        failed.error.as_ref().unwrap()["code"],
        "operation_recovery_process_termination_failed"
    );
    assert_eq!(failed.error.as_ref().unwrap()["retryable"], false);
    assert_eq!(
        failed.request["target_root"],
        target_root.display().to_string()
    );
    assert!(managed_pid_is_alive(worker_pid).unwrap());

    let mut stream = connect_to_cli_test_daemon(port);
    let retry_path = format!("/api/goals/export/jira/{}/retry", operation.id);
    let request = format!(
        "POST {retry_path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    server.join().unwrap().unwrap();
    assert!(response.starts_with("HTTP/1.1 409"), "{response}");
    assert_eq!(
        registry
            .recover()
            .unwrap()
            .iter()
            .filter(|candidate| {
                candidate
                    .request
                    .get("recovery_of")
                    .and_then(serde_json::Value::as_str)
                    == Some(operation.id.as_str())
            })
            .count(),
        0,
        "a recovery-attention operation must not launch a replacement worker"
    );
    let (logs, _, _) = registry.page_logs(&operation.id, 20, 0).unwrap();
    assert!(
        logs.iter()
            .any(|entry| entry.message == "Evidence retained before failed recovery")
    );
    assert!(
        logs.iter()
            .any(|entry| entry.message == "Recovery could not confirm managed process exit")
    );

    supervisor.request_termination(&worker.id, "kill").unwrap();
    wait_for_cli_managed_pid_exit(worker_pid);
    assert!(!managed_pid_is_alive(worker_pid).unwrap());
    fs::remove_dir_all(temp_root).unwrap();
}

fn cli_operation_helper_process_spec(operation_id: &str) -> ManagedProcessSpec {
    #[cfg(windows)]
    let (command, args) = (
        "cmd".to_string(),
        vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()],
    );
    #[cfg(not(windows))]
    let (command, args) = (
        "sh".to_string(),
        vec!["-c".to_string(), "while :; do sleep 1; done".to_string()],
    );
    ManagedProcessSpec {
        owner: ProcessOwner::Runner,
        command,
        args,
        cwd: None,
        env: Vec::new(),
        stdin: None,
        limits: Some(ProcessResourceLimits {
            kill_on_parent_exit: true,
            ..Default::default()
        }),
        authorization_command: Some("refine test restart helper".to_string()),
        sensitive: false,
        metadata: serde_json::from_value(serde_json::json!({
            "kind": "runner",
            "worker_kind": "jira-export-test-helper",
            "operation_id": operation_id
        }))
        .unwrap(),
    }
}

#[cfg(not(windows))]
fn cli_stubborn_operation_helper_process_spec(
    operation_id: &str,
    ready_path: &std::path::Path,
) -> ManagedProcessSpec {
    let mut spec = cli_operation_helper_process_spec(operation_id);
    spec.args = vec![
        "-c".to_string(),
        "trap '' TERM; : > \"$1\"; while :; do sleep 1; done".to_string(),
        "refine-recovery-test".to_string(),
        ready_path.display().to_string(),
    ];
    spec
}

fn wait_for_cli_managed_pid_exit(pid: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while managed_pid_is_alive(pid).unwrap_or(false) && std::time::Instant::now() < deadline {
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wait_for_cli_operation_state(
    registry: &FileOperationRegistry,
    operation_id: &str,
    expected: OperationState,
) -> crate::process::supervisor::operations::OperationHandle {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let operation = registry.status(operation_id).unwrap();
        if operation.state == expected {
            return operation;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "operation {operation_id} remained {:?}",
            operation.state
        );
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wait_for_cli_operation_log(registry: &FileOperationRegistry, operation_id: &str, message: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let (logs, _, _) = registry.page_logs(operation_id, 200, 0).unwrap();
        if logs.iter().any(|entry| entry.message == message) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "operation {operation_id} did not persist log {message:?}"
        );
        thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn connect_to_cli_test_daemon(port: u16) -> std::net::TcpStream {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)) {
            Ok(stream) => return stream,
            Err(_) if std::time::Instant::now() < deadline => {
                thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("test daemon on port {port} did not start: {error}"),
        }
    }
}

#[test]
fn system_ps_lists_and_stops_supervised_processes() {
    let temp_root = unique_temp_dir("cli-system-ps");
    let runtime_root = temp_root.join("run");
    let port = 19091;
    let port_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(port);
    let supervisor = FileProcessSupervisor::new(&port_root);
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
    let stoppable = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::UserHelper,
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
            metadata: Default::default(),
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

    let stopped = system_ps_response(
        runtime_root.clone(),
        Some(port),
        Some(&stoppable.id),
        "terminate",
    )
    .unwrap();
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["process"]["id"], stoppable.id);
    assert!(supervisor.inspect(&stoppable.id).is_err());

    fs::remove_dir_all(temp_root).unwrap();
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

#[test]
fn goal_edit_note_delete_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-goal-edit-note-delete");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Original",
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
            "edit",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
            "--name",
            "Renamed",
            "--priority",
            "medium",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "note",
            "GOAL1",
            "CLI note",
            "--target-root",
            target_root.to_str().unwrap(),
            "--author",
            "Reviewer",
        ])
        .unwrap(),
    )
    .unwrap();

    let written = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(written.contains("\"name\": \"Renamed\""));
    assert!(written.contains("\"priority\": \"medium\""));
    assert!(written.contains("\"body\": \"CLI note\""));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "delete",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    fs::remove_dir_all(temp_root).unwrap();
}

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
fn goal_approve_and_undo_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-goal-merge-undo");
    let target_root = temp_root.clone();
    fs::create_dir_all(&target_root).unwrap();
    run_git(&target_root, &["init", "-b", "main"]);
    run_git(&target_root, &["config", "user.email", "test@example.com"]);
    run_git(&target_root, &["config", "user.name", "Test User"]);
    fs::write(target_root.join("app.txt"), "base\n").unwrap();
    run_git(&target_root, &["add", "app.txt"]);
    run_git(&target_root, &["commit", "-m", "initial"]);
    let refine_dir = refine_dir_for_target_root(&target_root).unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Merge Goal",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();
    let branch = "refine/GOAL1/round-1";
    let worktree = target_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    run_git(
        &target_root,
        &["worktree", "add", "-b", branch, worktree.to_str().unwrap()],
    );
    fs::write(worktree.join("approved.txt"), "approved\n").unwrap();
    run_git(&worktree, &["add", "approved.txt"]);
    run_git(&worktree, &["commit", "-m", "candidate"]);
    let goal_path = refine_dir.join("goals/GO/AL1/goal.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&goal_path).unwrap()).unwrap();
    value["status"] = serde_json::Value::String("review".to_string());
    value["branch_name"] = serde_json::Value::String(branch.to_string());
    fs::write(&goal_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "approve",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let written = fs::read_to_string(&goal_path).unwrap();
    assert!(written.contains("\"status\": \"done\""));
    assert_eq!(
        fs::read_to_string(target_root.join("approved.txt")).unwrap(),
        "approved\n"
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "undo",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let written = fs::read_to_string(&goal_path).unwrap();
    assert!(written.contains("\"status\": \"review\""));

    fs::remove_dir_all(temp_root).unwrap();
}

fn run_git(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

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
fn cli_goal_lifecycle_membership_and_feature_edit_use_tool_services() {
    let temp_root = unique_temp_dir("cli-goal-lifecycle");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    for (command, args) in [
        (
            "goal",
            vec![
                "create",
                "Lifecycle Goal",
                "--target-root",
                target_root.to_str().unwrap(),
                "--id",
                "GOAL1",
            ],
        ),
        (
            "feature",
            vec![
                "create",
                "Feature One",
                "--target-root",
                target_root.to_str().unwrap(),
                "--id",
                "FEA1",
            ],
        ),
    ] {
        let mut argv = vec!["refine", command];
        argv.extend(args);
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "assign-feature",
            "GOAL1",
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
            .contains("\"feature_id\": \"FEA1\"")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "edit",
            "FEA1",
            "--target-root",
            target_root.to_str().unwrap(),
            "--name",
            "Renamed Feature",
            "--description",
            "Edited",
            "--reporter",
            "QA",
        ])
        .unwrap(),
    )
    .unwrap();
    let feature = fs::read_to_string(refine_dir.join("features/FE/A1/feature.json")).unwrap();
    assert!(feature.contains("\"name\": \"Renamed Feature\""));
    assert!(feature.contains("\"reporter\": \"QA\""));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "remove-feature",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"feature_id\": null")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "start",
            "GOAL1",
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

    let snapshot = FileProjectStateStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let goal = snapshot.goals.values().next().unwrap();
    assert_eq!(goal.goal.feature_id.as_deref(), Some("FEA1"));
    assert_eq!(goal.goal.priority.as_str(), "high");
    assert_eq!(goal.goal.reporter.as_deref(), Some("QA"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_import_parses_structured_project_spec_with_shared_import_service() {
    let temp_root = unique_temp_dir("cli-feature-import-project-spec");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "create",
            "Imported Project Feature",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();

    let spec = r#"{
        "project": {
            "name": "Budget App",
            "features": [
                {
                    "name": "Transactions",
                    "goals": [
                        {
                            "title": "Categorize transactions",
                            "current_state": "Transactions are uncategorized.",
                            "desired_state": "Users can categorize each transaction.",
                            "priority": "medium"
                        }
                    ]
                }
            ]
        }
    }"#;
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "import",
            "--target-root",
            target_root.to_str().unwrap(),
            "--text",
            spec,
            "--reporter",
            "Product",
            "--feature-id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();

    let snapshot = FileProjectStateStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let goal = snapshot.goals.values().next().unwrap();
    assert_eq!(goal.goal.name, "Categorize transactions");
    assert_eq!(goal.goal.feature_id.as_deref(), Some("FEA1"));
    assert_eq!(goal.goal.priority.as_str(), "medium");
    assert_eq!(goal.goal.reporter.as_deref(), Some("Product"));

    fs::remove_dir_all(temp_root).unwrap();
}

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

#[test]
fn log_bundle_exports_redacted_support_bundle() {
    let temp_root = unique_temp_dir("cli-log-bundle");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    let runtime_root = temp_root.join("run");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(
        refine_dir.join("nodes.json"),
        r#"{"nodes":[{"id":"default","display_name":"Default","created_at":"2026-06-16T00:00:00Z","updated_at":"2026-06-16T00:00:00Z","settings":{"provider_token":"secret-value","visible":"ok"}}]}"#,
    )
    .unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "log",
            "bundle",
            "--target-root",
            target_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--repo-root",
            temp_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let bundle_dir = refine_dir.join("support-bundles");
    let bundle_path = fs::read_dir(&bundle_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = fs::read_to_string(bundle_path).unwrap();
    assert!(body.contains("[redacted]"));
    assert!(!body.contains("secret-value"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn node_commands_use_shared_node_registry_service() {
    let temp_root = unique_temp_dir("cli-node-registry");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Owned Goal",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();

    for argv in [
        vec![
            "refine",
            "node",
            "list",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "create",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "rename",
            "node-1",
            "Node One",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "activate",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "settings",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "transfer",
            "node-1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "activate",
            "default",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "archive",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let goal = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(goal.contains("\"node_id\": \"node-1\""));
    let nodes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join("nodes.json")).unwrap()).unwrap();
    assert_eq!(nodes["nodes"][1]["display_name"], "Node One");
    assert_eq!(nodes["nodes"][1]["archived"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_transfer_command_moves_feature_and_member_goals_between_nodes() {
    let temp_root = unique_temp_dir("cli-feature-node-transfer");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    for argv in [
        vec![
            "refine",
            "node",
            "create",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "feature",
            "create",
            "Transfer Feature",
            "--id",
            "FEA1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "goal",
            "create",
            "Feature Goal",
            "--id",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "feature",
            "add-goal",
            "FEA1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let direct_goal = dispatch(
        Cli::try_parse_from([
            "refine",
            "node",
            "transfer",
            "node-1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap_err();
    assert!(
        direct_goal
            .to_string()
            .contains("transfer the Feature instead"),
        "{direct_goal}"
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "transfer",
            "FEA1",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let feature = fs::read_to_string(refine_dir.join("features/FE/A1/feature.json")).unwrap();
    assert!(feature.contains("\"node_id\": \"node-1\""));
    let goal = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(goal.contains("\"node_id\": \"node-1\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn cluster_commands_use_shared_cluster_service() {
    let temp_root = unique_temp_dir("cli-cluster-registry");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "goal",
            "create",
            "Cluster Goal",
            "--target-root",
            target_root.to_str().unwrap(),
            "--id",
            "GOAL1",
        ])
        .unwrap(),
    )
    .unwrap();

    for argv in [
        vec![
            "refine",
            "cluster",
            "list",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "add-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "show",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "edit-node",
            "node-1",
            "--ssh-host",
            "example.com",
            "--ssh-user",
            "deploy",
            "--ssh-identity-path",
            "~/.ssh/refine_ed25519",
            "--ssh-port",
            "2222",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "disable-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "enable-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "bootstrap",
            "node-1",
            "--dry-run",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "edit-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "distribute",
            "--dry-run",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "distribute",
            "--to",
            "node-1",
            "--converge",
            "--dry-run",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "transfer",
            "node-1",
            "GOAL1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "sync",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "maintenance",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "remove-node",
            "node-1",
            "--target-root",
            target_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let goal = fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json")).unwrap();
    assert!(goal.contains("\"node_id\": \"node-1\""));
    assert!(!refine_dir.join("cluster.json").exists());
    let nodes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(refine_dir.join("nodes.json")).unwrap()).unwrap();
    let node = nodes["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == "node-1")
        .unwrap();
    assert_eq!(node["ssh_host"], "example.com");
    assert_eq!(node["ssh_user"], "deploy");
    assert_eq!(node["ssh_port"], 2222);
    assert_eq!(node["archived"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn agent_configure_and_diagnose_use_provider_service() {
    let supervisor = Cli::try_parse_from(["refine", "agent", "supervisor"]).unwrap();
    assert!(matches!(
        supervisor.command,
        Commands::Agent {
            action: AgentAction::Supervisor
        }
    ));
    dispatch(
        Cli::try_parse_from(["refine", "agent", "configure", "--provider", "smoke-ai"]).unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from(["refine", "agent", "diagnose", "--provider", "smoke-ai"]).unwrap(),
    )
    .unwrap();
    let invalid = dispatch(
        Cli::try_parse_from(["refine", "agent", "configure", "--provider", "nope"]).unwrap(),
    );
    assert!(invalid.is_err());
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn git_init(root: &std::path::Path) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

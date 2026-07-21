use super::dispatch::{
    absolute_cli_path, dispatch, explicit_target_root_path, plan_goal_draft_body,
    system_ps_response, system_status_response,
};
use super::*;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
};
use crate::process::supervisor::lifecycle::{DaemonLifecycleService, FileDaemonLifecycleService};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::observability::activity::ActivityService;
use crate::tools::observability::activity::FileActivityService;
use crate::tools::product::project_state::PROJECTION_SNAPSHOT_FILE;
use crate::tools::product::project_state::{FileProjectStateStore, ProjectStateStore};
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

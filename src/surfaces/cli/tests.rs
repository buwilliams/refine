use super::dispatch::{
    absolute_cli_path, dispatch, explicit_durable_root_path, system_status_response,
};
use super::*;
use crate::core::host::agent_providers::smoke_ai_env_lock;
use crate::core::observability::activity::ActivityService;
use crate::core::observability::activity::FileActivityService;
use crate::core::product::project_state::PROJECTION_SNAPSHOT_FILE;
use crate::core::product::project_state::{FileProjectStateStore, ProjectStateStore};
use crate::core::supervisor::config::FileSettingsService;
use crate::core::supervisor::lifecycle::{DaemonLifecycleService, FileDaemonLifecycleService};
use crate::core::supervisor::runtime::RuntimeRoot;
use clap::Parser;
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn explicit_durable_root_path_detects_internal_cli_escape_hatch() {
    let durable_root = PathBuf::from("/tmp/refine-state");
    let command = Commands::Gap {
        action: GapAction::Create {
            name: "direct write".to_string(),
            durable_root: Some(durable_root.clone()),
            id: None,
        },
    };
    assert_eq!(explicit_durable_root_path(&command), Some(&durable_root));

    let default_daemon_command = Commands::Workflow {
        action: WorkflowAction::Schedule {
            durable_root: PathBuf::new(),
            runtime_root: PathBuf::from("run"),
        },
    };
    assert_eq!(explicit_durable_root_path(&default_daemon_command), None);
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
    let durable_root = temp_root.join(".refine");
    let gap_dir = durable_root.join("gaps").join("01").join("GAP1");
    let cache_dir = temp_root.join("run").join("8080").join("cache");
    fs::create_dir_all(&gap_dir).unwrap();
    fs::write(
        gap_dir.join("gap.json"),
        r#"{
              "id": "GAP1",
              "name": "CLI visible Gap",
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
        "--durable-root",
        durable_root.to_str().unwrap(),
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
    assert_eq!(bind_address, IpAddr::V4(Ipv4Addr::LOCALHOST));
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
    assert!(Cli::try_parse_from(["refine", "system", "web", "--durable-root", ".refine"]).is_err());
    assert!(Cli::try_parse_from(["refine", "system", "serve", "--once"]).is_err());
    assert!(Cli::try_parse_from(["refine", "system", "start", "--token", "secret"]).is_err());
}

#[test]
fn project_registry_commands_use_shared_file_project_registry_service() {
    let temp_root = unique_temp_dir("cli-project-registry");
    let runtime_root = temp_root.join("run");
    let app_one = temp_root.join("app-one");
    let app_two = temp_root.join("app-two");
    fs::create_dir_all(app_one.join(".refine")).unwrap();
    fs::create_dir_all(app_two.join(".refine")).unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "status",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--durable-root",
            app_one.join(".refine").to_str().unwrap(),
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
    assert!(destination.join(".refine/refine.json").exists());
    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(runtime_root.join("apps.json")).unwrap()).unwrap();
    assert_eq!(registry["active_app"], destination.to_str().unwrap());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_and_system_doctor_and_migrate_use_observability_services() {
    let temp_root = unique_temp_dir("cli-doctor-migrate");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run");
    fs::create_dir_all(&durable_root).unwrap();

    for argv in [
        vec![
            "refine",
            "project",
            "doctor",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--repo-root",
            temp_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "system",
            "doctor",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--repo-root",
            temp_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "project",
            "migrate",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn project_attach_runs_legacy_refine_migration() {
    let temp_root = unique_temp_dir("cli-project-migration");
    let runtime_root = temp_root.join("run");
    let app_root = temp_root.join("legacy-app");
    let durable_root = app_root.join(".refine");
    fs::create_dir_all(durable_root.join("gaps/GA")).unwrap();
    fs::write(durable_root.join("gaps/GA/gap.json"), "{}").unwrap();

    dispatch(
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
    .unwrap();
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(durable_root.join("refine.json")).unwrap())
            .unwrap();
    assert_eq!(config["schema_version"], 1);

    dispatch(
        Cli::try_parse_from([
            "refine",
            "project",
            "migrate",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

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

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn gap_create_list_show_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-gap-create");
    let durable_root = temp_root.join(".refine");

    let create = Cli::try_parse_from([
        "refine",
        "gap",
        "create",
        "CLI Gap",
        "--durable-root",
        durable_root.to_str().unwrap(),
        "--id",
        "GAP1",
    ])
    .unwrap();
    dispatch(create).unwrap();

    let list = Cli::try_parse_from([
        "refine",
        "gap",
        "list",
        "--durable-root",
        durable_root.to_str().unwrap(),
    ])
    .unwrap();
    dispatch(list).unwrap();

    let show = Cli::try_parse_from([
        "refine",
        "gap",
        "show",
        "GAP1",
        "--durable-root",
        durable_root.to_str().unwrap(),
    ])
    .unwrap();
    dispatch(show).unwrap();

    let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"name\": \"CLI Gap\""));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn gap_edit_note_delete_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-gap-edit-note-delete");
    let durable_root = temp_root.join(".refine");

    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Original",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "edit",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
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
            "gap",
            "note",
            "GAP1",
            "CLI note",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--author",
            "Reviewer",
        ])
        .unwrap(),
    )
    .unwrap();

    let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"name\": \"Renamed\""));
    assert!(written.contains("\"priority\": \"medium\""));
    assert!(written.contains("\"body\": \"CLI note\""));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "delete",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn gap_round_append_and_edit_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-gap-rounds");
    let durable_root = temp_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Round Gap",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "round",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--reporter",
            "Reporter",
            "--actual",
            "Actual",
            "--target",
            "Target",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "round",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--edit-latest",
            "--reporter",
            "Reviewer",
            "--actual",
            "Revised",
        ])
        .unwrap(),
    )
    .unwrap();

    let written = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(written.contains("\"reporter\": \"Reviewer\""));
    assert!(written.contains("\"actual\": \"Revised\""));
    assert!(written.contains("\"target\": \"Target\""));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn gap_merge_and_undo_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-gap-merge-undo");
    let durable_root = temp_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Merge Gap",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();
    let gap_path = durable_root.join("gaps/GA/P1/gap.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&gap_path).unwrap()).unwrap();
    value["status"] = serde_json::Value::String("ready-merge".to_string());
    fs::write(&gap_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "merge",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let written = fs::read_to_string(&gap_path).unwrap();
    assert!(written.contains("\"status\": \"done\""));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "undo",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let written = fs::read_to_string(&gap_path).unwrap();
    assert!(written.contains("\"status\": \"review\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn workflow_schedule_uses_file_scheduler_service() {
    let temp_root = unique_temp_dir("cli-workflow-schedule");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    init_test_git_repo(&temp_root);
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '\\n# scheduled by smoke-ai\\n' >> app.py\nprintf '%s\\n' 'smoke-ai gap-agent response'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let _smoke_ai_env_guard = smoke_ai_env_lock().lock().unwrap();
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Schedulable Gap",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();
    FileSettingsService::new(&durable_root)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "workflow",
            "transition",
            "GAP1",
            "todo",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "workflow",
            "schedule",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let scheduler_state = fs::read_to_string(runtime_root.join("scheduler-state.json")).unwrap();
    assert!(scheduler_state.contains("\"gap_id\": \"GAP1\""));
    assert!(scheduler_state.contains("\"state\": \"completed\""));
    let gap = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(gap.contains("\"status\": \"review\""));
    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_create_list_show_and_membership_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-feature-membership");
    let durable_root = temp_root.join(".refine");

    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Gap One",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
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
            "--durable-root",
            durable_root.to_str().unwrap(),
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
            "add-gap",
            "FEA1",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
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
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "list",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let assigned = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(assigned.contains("\"feature_id\": \"FEA1\""));
    assert!(assigned.contains("\"feature_order\": 1"));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "remove-gap",
            "FEA1",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    let removed = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(removed.contains("\"feature_id\": null"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn cli_gap_lifecycle_membership_and_feature_edit_use_core_services() {
    let temp_root = unique_temp_dir("cli-gap-lifecycle");
    let durable_root = temp_root.join(".refine");
    for (command, args) in [
        (
            "gap",
            vec![
                "create",
                "Lifecycle Gap",
                "--durable-root",
                durable_root.to_str().unwrap(),
                "--id",
                "GAP1",
            ],
        ),
        (
            "feature",
            vec![
                "create",
                "Feature One",
                "--durable-root",
                durable_root.to_str().unwrap(),
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
            "gap",
            "assign-feature",
            "GAP1",
            "FEA1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"feature_id\": \"FEA1\"")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "edit",
            "FEA1",
            "--durable-root",
            durable_root.to_str().unwrap(),
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
    let feature = fs::read_to_string(durable_root.join("features/FE/A1/feature.json")).unwrap();
    assert!(feature.contains("\"name\": \"Renamed Feature\""));
    assert!(feature.contains("\"reporter\": \"QA\""));

    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "remove-feature",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"feature_id\": null")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "start",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"status\": \"in-progress\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_reorder_and_move_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-feature-reorder-move");
    let durable_root = temp_root.join(".refine");
    for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                name,
                "--durable-root",
                durable_root.to_str().unwrap(),
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
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();
    for gap_id in ["GAP1", "GAP2"] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "add-gap",
                "FEA1",
                gap_id,
                "--durable-root",
                durable_root.to_str().unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
    }
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "reorder-gap",
            "FEA1",
            "GAP2",
            "1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P2/gap.json"))
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
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_cancel_and_delete_use_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-feature-cancel-delete");
    let durable_root = temp_root.join(".refine");
    for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                name,
                "--durable-root",
                durable_root.to_str().unwrap(),
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
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();
    for gap_id in ["GAP1", "GAP2"] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "feature",
                "add-gap",
                "FEA1",
                gap_id,
                "--durable-root",
                durable_root.to_str().unwrap(),
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
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"status\": \"cancelled\"")
    );

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "delete",
            "FEA1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(!durable_root.join("features/FE/A1/feature.json").exists());
    assert!(!durable_root.join("gaps/GA/P1/gap.json").exists());
    assert!(!durable_root.join("gaps/GA/P2/gap.json").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn feature_import_uses_shared_import_service() {
    let temp_root = unique_temp_dir("cli-feature-import");
    let durable_root = temp_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "create",
            "Imported Feature",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();
    let csv = temp_root.join("import.csv");
    fs::write(
        &csv,
        "actual,target,reporter,priority\nBroken flow,Fixed flow,QA,high\n",
    )
    .unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "feature",
            "import",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--file",
            csv.to_str().unwrap(),
            "--csv",
            "--feature-id",
            "FEA1",
        ])
        .unwrap(),
    )
    .unwrap();

    let snapshot = FileProjectStateStore::new(&durable_root)
        .rebuild_projection()
        .unwrap();
    let gap = snapshot.gaps.values().next().unwrap();
    assert_eq!(gap.gap.feature_id.as_deref(), Some("FEA1"));
    assert_eq!(gap.gap.priority.as_str(), "high");
    assert_eq!(gap.gap.reporter.as_deref(), Some("QA"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn workflow_transition_uses_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-workflow-transition");
    let durable_root = temp_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Workflow Gap",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "workflow",
            "transition",
            "GAP1",
            "todo",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn workflow_bulk_transition_uses_shared_file_work_item_service() {
    let temp_root = unique_temp_dir("cli-workflow-bulk");
    let durable_root = temp_root.join(".refine");
    for (id, name) in [("GAP1", "Gap One"), ("GAP2", "Gap Two")] {
        dispatch(
            Cli::try_parse_from([
                "refine",
                "gap",
                "create",
                name,
                "--durable-root",
                durable_root.to_str().unwrap(),
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
            "workflow",
            "bulk-transition",
            "todo",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--selected-id",
            "GAP1",
            "--selected-id",
            "GAP2",
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P2/gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn workflow_control_commands_use_core_state() {
    let temp_root = unique_temp_dir("cli-workflow-control");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    init_test_git_repo(&temp_root);
    fs::write(
        &smoke_ai,
        "#!/bin/sh\nprintf '\\n# scheduled by smoke-ai control\\n' >> app.py\nprintf '%s\\n' 'smoke-ai gap-agent response'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    let _smoke_ai_env_guard = smoke_ai_env_lock().lock().unwrap();
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Workflow Control Gap",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "workflow",
            "transition",
            "GAP1",
            "todo",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    FileSettingsService::new(&durable_root)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();

    for argv in [
        vec![
            "refine",
            "workflow",
            "schedule",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "workflow",
            "pause",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "workflow",
            "resume",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "workflow",
            "enforce",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }
    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    let pause_state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(runtime_root.join("process-control.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(pause_state["agents_paused"], false);
    assert_eq!(pause_state["background_processes_stopped"], false);

    dispatch(
        Cli::try_parse_from([
            "refine",
            "workflow",
            "bulk-transition",
            "failed",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--selected-id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "workflow",
            "restore",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json"))
            .unwrap()
            .contains("\"status\": \"todo\"")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn log_commands_use_shared_activity_service() {
    let temp_root = unique_temp_dir("cli-log-activity");
    let durable_root = temp_root.join(".refine");
    let service = FileActivityService::new(&durable_root);
    let first = service.new_entry(
        "Build failed",
        "error",
        "quality",
        Some("GAP1".to_string()),
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
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--limit",
            "2",
        ],
        vec![
            "refine",
            "log",
            "tail",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--limit",
            "1",
        ],
        vec![
            "refine",
            "log",
            "query",
            "failed",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--severity",
            "error",
            "--gap-id",
            "GAP1",
        ],
        vec![
            "refine",
            "log",
            "show",
            first_id.as_str(),
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "log",
            "export",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn log_bundle_exports_redacted_support_bundle() {
    let temp_root = unique_temp_dir("cli-log-bundle");
    let durable_root = temp_root.join(".refine");
    let runtime_root = temp_root.join("run");
    fs::create_dir_all(&durable_root).unwrap();
    fs::write(
        durable_root.join("settings.json"),
        r#"{"provider_token":"secret-value","visible":"ok"}"#,
    )
    .unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "log",
            "bundle",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--repo-root",
            temp_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let bundle_dir = durable_root.join("support-bundles");
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
    let durable_root = temp_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Owned Gap",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();

    for argv in [
        vec![
            "refine",
            "node",
            "list",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "create",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "rename",
            "node-1",
            "Node One",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "activate",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "settings",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "transfer",
            "node-1",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "activate",
            "default",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "node",
            "archive",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let gap = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(gap.contains("\"node_id\": \"node-1\""));
    let nodes: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(durable_root.join("nodes.json")).unwrap())
            .unwrap();
    assert_eq!(nodes["nodes"][1]["display_name"], "Node One");
    assert_eq!(nodes["nodes"][1]["archived"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn cluster_commands_use_shared_cluster_registry_service() {
    let temp_root = unique_temp_dir("cli-cluster-registry");
    let durable_root = temp_root.join(".refine");
    dispatch(
        Cli::try_parse_from([
            "refine",
            "gap",
            "create",
            "Cluster Gap",
            "--durable-root",
            durable_root.to_str().unwrap(),
            "--id",
            "GAP1",
        ])
        .unwrap(),
    )
    .unwrap();

    for argv in [
        vec![
            "refine",
            "cluster",
            "list",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "add-node",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "show",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
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
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "disable-node",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "enable-node",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "bootstrap",
            "node-1",
            "--dry-run",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "transfer",
            "node-1",
            "GAP1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "sync",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "maintenance",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "cluster",
            "remove-node",
            "node-1",
            "--durable-root",
            durable_root.to_str().unwrap(),
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let gap = fs::read_to_string(durable_root.join("gaps/GA/P1/gap.json")).unwrap();
    assert!(gap.contains("\"node_id\": \"node-1\""));
    let cluster: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(durable_root.join("cluster.json")).unwrap())
            .unwrap();
    assert_eq!(cluster["nodes"].as_array().unwrap().len(), 0);

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

fn init_test_git_repo(repo: &std::path::Path) {
    fs::write(repo.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "refine-test@example.invalid"],
        vec!["config", "user.name", "Refine Test"],
        vec!["add", "app.py"],
        vec!["commit", "-q", "-m", "Initialize test app"],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git command failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

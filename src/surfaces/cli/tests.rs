use super::dispatch::{absolute_cli_path, dispatch, explicit_durable_root_path};
use super::*;
use crate::core::observability::activity::ActivityService;
use crate::core::observability::activity::FileActivityService;
use crate::core::product::project_state::PROJECTION_SNAPSHOT_FILE;
use crate::core::product::project_state::{FileProjectStateStore, ProjectStateStore};
use clap::Parser;
use std::fs;
use std::path::PathBuf;
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
    assert_eq!(runtime_root, PathBuf::from("run"));
    assert!(once);
    assert!(!foreground);

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
fn system_install_update_rollback_and_uninstall_use_installation_service() {
    let temp_root = unique_temp_dir("cli-installation");
    let runtime_root = temp_root.join("run");

    for argv in [
        vec![
            "refine",
            "system",
            "install",
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
            "update",
            "1.1.0",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ],
        vec![
            "refine",
            "system",
            "rollback",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.1.0",
        ],
        vec![
            "refine",
            "system",
            "repair",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.0.0",
        ],
        vec![
            "refine",
            "system",
            "uninstall",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.0.0",
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let state: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(runtime_root.join("install-state.json")).unwrap())
            .unwrap();
    assert_eq!(state["status"]["installed"], false);
    assert_eq!(state["status"]["version"], "1.0.0");

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
    assert!(scheduler_state.contains("\"state\": \"reserved\""));

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

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

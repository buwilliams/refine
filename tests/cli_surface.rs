mod support;

mod cli_surface {
    pub(super) mod agents;
    pub(super) mod config;
    pub(super) mod daemon_status;
    pub(super) mod features;
    pub(super) mod fleet;
    pub(super) mod goals;
    pub(super) mod hub;
    pub(super) mod logs;
    pub(super) mod nodes;
    pub(super) mod projects;
    pub(super) mod system_diagnostics;
    pub(super) mod todos;
}

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use refine::infrastructure::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessOwner,
};
use serde_json::json;
use support::integration::IntegrationFixture;

use cli_surface::agents::*;
use cli_surface::config::*;
use cli_surface::daemon_status::*;
use cli_surface::features::*;
use cli_surface::fleet::*;
use cli_surface::goals::*;
use cli_surface::hub::*;
use cli_surface::logs::*;
use cli_surface::nodes::*;
use cli_surface::projects::*;
use cli_surface::system_diagnostics::*;
use cli_surface::todos::*;

#[test]
#[ignore = "daemon-backed Hub and workflow test; run with the isolated CLI fixture"]
fn hub_and_workflow_surface_suite() {
    let fixture = IntegrationFixture::start("hub-workflow");
    hub_and_workflow_controls_share_the_daemon_surface(&fixture);
}

#[test]
#[ignore = "daemon-backed surface test; run through `cargo run --manifest-path xtask/Cargo.toml -- test-cli`"]
fn cli_surface_suite() {
    let fixture = IntegrationFixture::start("cli");

    system_status_reports_healthy_daemon(&fixture);
    project_status_is_attached_to_test_app(&fixture);
    config_commands_route_through_the_active_daemon(&fixture);
    provider_configuration_commands(&fixture);
    daemon_backed_project_status_suppresses_ambiguous_default_label(&fixture);
    project_doctor_runs(&fixture);
    project_registry_lifecycle_commands(&fixture);
    system_doctor_and_api_groups_run(&fixture);
    hub_and_workflow_controls_share_the_daemon_surface(&fixture);
    goal_create_list_show_edit_note_round_delete(&fixture);
    goal_feature_assignment_and_round_edit_latest(&fixture);
    goal_workflow_actions_start_retry_and_undo(&fixture);
    goal_cancel_uses_active_node_and_rejects_foreign_owner(&fixture);
    feature_create_membership_rollup_and_delete(&fixture);
    feature_show_edit_reorder_move_cancel_and_import(&fixture);
    todo_commands_share_reporter_scoped_api_capability(&fixture);
    node_create_activate_archive(&fixture);
    node_show_rename_settings_and_transfer(&fixture);
    fleet_local_registry_commands(&fixture);
    log_commands_query_public_activity(&fixture);
    agent_commands_use_smoke_ai(&fixture);
}

#[test]
#[ignore = "daemon-backed provider configuration; run through the xtask CLI fixture"]
fn provider_configuration_surface_suite() {
    let fixture = IntegrationFixture::start("provider-configuration");
    provider_configuration_commands(&fixture);
}

#[test]
#[ignore = "daemon-backed Planning test; requires an isolated runtime and app fixture"]
fn planning_surface_suite() {
    let fixture = IntegrationFixture::start_with_agent_automation("planning");
    let run = |args: &[&str]| {
        let output = fixture.run_refine(args);
        fixture.assert_success(&args.join(" "), &output);
        fixture.json_stdout(&output)
    };
    let created = run(&[
        "planning",
        "apply",
        "board.create",
        "--request-id",
        "shared-board",
        "--data",
        r#"{"name":"Shared work"}"#,
    ]);
    assert_eq!(created["state"], "complete");
    let board = &created["result"];
    let board_id = board["id"].as_str().unwrap();
    let ideas = board["lanes"][0]["id"].as_str().unwrap();
    let done = board["lanes"][1]["id"].as_str().unwrap();
    let request = [
        "planning",
        "apply",
        "card.create",
        "--request-id",
        "shared-card",
        "--board-id",
        board_id,
        "--lane-id",
        ideas,
        "--data",
        r#"{"name":"Read paper","reporter":"Alice"}"#,
    ];
    run(&request);
    let await_action = |id: &str| {
        let deadline = Instant::now() + Duration::from_secs(40);
        loop {
            let action = run(&["planning", "action", id]);
            assert_ne!(action["state"], "failed", "{action}");
            if action["state"] == "complete" {
                break action;
            }
            assert!(
                Instant::now() < deadline,
                "Planning action did not settle: {action}"
            );
            thread::sleep(Duration::from_millis(200));
        }
    };
    let action = await_action("shared-card");
    assert_eq!(
        run(&request)["result"]["goal_id"],
        action["result"]["goal_id"]
    );
    let id = action["result"]["goal_id"].as_str().unwrap();
    let goal = run(&["goal", "show", id]);
    assert_eq!(goal["goal"]["status"], "draft");
    let moved = fixture.api_json("POST", "/mcp", json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"refine_planning_command","arguments":{"operation":"card.move","request_id":"personal-done","goal_id":id,"board_id":board_id,"lane_id":done,"expected_revision":1}}}));
    assert!(moved["error"].is_null(), "{moved}");
    await_action("personal-done");
    let snapshot = run(&["planning", "list"]);
    assert_eq!(snapshot["cards"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["cards"][0]["placement"]["lane_id"], done);
    assert_eq!(snapshot["cards"][0]["goal"]["status"], "draft");
    assert_eq!(snapshot["cards"][0]["goal"]["round_count"], 0);
}

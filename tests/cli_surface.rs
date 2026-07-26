mod support;

mod cli_surface {
    pub(super) mod agents;
    pub(super) mod cluster;
    pub(super) mod daemon_status;
    pub(super) mod features;
    pub(super) mod goals;
    pub(super) mod logs;
    pub(super) mod nodes;
    pub(super) mod projects;
    pub(super) mod system_diagnostics;
    pub(super) mod todos;
}

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use refine::process::subprocess::{FileProcessSupervisor, ManagedProcess, ProcessOwner};
use serde_json::json;
use support::integration::IntegrationFixture;

use cli_surface::agents::*;
use cli_surface::cluster::*;
use cli_surface::daemon_status::*;
use cli_surface::features::*;
use cli_surface::goals::*;
use cli_surface::logs::*;
use cli_surface::nodes::*;
use cli_surface::projects::*;
use cli_surface::system_diagnostics::*;
use cli_surface::todos::*;

#[test]
#[ignore = "daemon-backed surface test; run through `cargo run --manifest-path xtask/Cargo.toml -- test-cli`"]
fn cli_surface_suite() {
    let fixture = IntegrationFixture::start("cli");

    system_status_reports_healthy_daemon(&fixture);
    project_status_is_attached_to_test_app(&fixture);
    project_doctor_runs(&fixture);
    project_registry_lifecycle_commands(&fixture);
    system_doctor_and_api_groups_run(&fixture);
    goal_create_list_show_edit_note_round_delete(&fixture);
    goal_feature_assignment_and_round_edit_latest(&fixture);
    goal_workflow_actions_start_retry_verify_merge_undo(&fixture);
    feature_create_membership_rollup_and_delete(&fixture);
    feature_show_edit_reorder_move_cancel_and_import(&fixture);
    todo_commands_share_reporter_scoped_api_capability(&fixture);
    node_create_activate_archive(&fixture);
    node_show_rename_settings_and_transfer(&fixture);
    cluster_local_registry_commands(&fixture);
    log_commands_query_public_activity(&fixture);
    agent_commands_use_smoke_ai(&fixture);
}

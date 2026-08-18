mod agents_processes;
mod config;
mod features;
mod goals;
mod imports_exports;
mod logs;
mod nodes_fleet;
mod parsing;
mod projects;
mod system;
mod todos;

use super::dispatch::{
    absolute_cli_path, dispatch, dispatch_config, explicit_target_root_path,
    follow_daemon_operation_with, plan_goal_draft_body, resolve_merged_daemon_route,
    run_system_start, system_ps_response, system_status_response,
};
use super::*;

use crate::application::fleet::node_sync::{
    FleetNodeDaemonClient, NodeDaemonReply, install_node_daemon_client,
};
use crate::application::projects::projection::FileProjectProjectionStore;
use crate::application::projects::projection::PROJECTION_SNAPSHOT_FILE;
use crate::application::system::daemon_lifecycle::{
    run_service_managed_daemon_with, stop_service_managed_daemon_with,
};
use crate::application::system::installation::InstalledServiceAction;
use crate::application::todos::FileTodoService;
use crate::application::work_items::FileWorkItemService;
use crate::infrastructure::observability::activity::ActivityService;
use crate::infrastructure::observability::activity::FileActivityService;
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
    ProcessSupervisor, managed_pid_is_alive,
};
use crate::infrastructure::process::supervisor::lifecycle::{
    DaemonReachability, DaemonRuntimeService, FileDaemonLifecycleService,
};
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::infrastructure::process::supervisor::runtime::RuntimeRoot;
use crate::infrastructure::storage::project_layout::refine_dir_for_target_root;
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use clap::Parser;
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(not(windows))]
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
) -> crate::infrastructure::process::supervisor::operations::OperationHandle {
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

fn run_git_stdout(root: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
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

mod activity_exports;
mod agents_chat;
mod features;
mod files_terminal;
mod git_changes;
mod goals;
mod http_transport;
mod imports;
mod imports_parity;
mod nodes_fleet;
mod operations_processes;
mod project_runtime;
mod quality_guidance;
mod static_surface;

use crate::model::log::LogEntry;
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationHandle, OperationRegistry, OperationState,
};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::logs::FileLogService;
use crate::tools::observability::metrics::{FileMetricsService, PerformanceQuery};
use crate::tools::product::chat::{ChatAttachment, ChatService, FileChatService};
use chrono::Utc;
use serde_json::json;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::*;
use crate::model::feature::{FeatureIndexProjection, FeatureRollup};
use crate::model::goal::{GoalIndexProjection, GoalPriority};
use crate::model::log::ActivityEntry;
use crate::model::workflow::GoalStatus;
use crate::process::agent_sessions::{GoalAgentLaunch, run_goal_agent};
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
    ProcessSupervisor, install_after_process_enumeration_hook, managed_pid_is_alive,
};
use crate::process::supervisor::lifecycle::{DaemonRuntimeService, FileDaemonLifecycleService};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::surfaces::web_server::support::{
    recent_operation_sse_events, runtime_process_status_value, runtime_process_summary_value,
};
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::product::project_projection::{
    ActivityProjectionQuery, DashboardProjection, FeatureSummaryProjection,
    FileProjectProjectionStore, GoalSummaryProjection, PROJECTION_SNAPSHOT_FILE,
    PROJECTION_SNAPSHOT_VERSION, PageRequest, ProjectionQuery, ProjectionSnapshot,
    RuntimeProjection,
};
use crate::tools::product::work_items::FileWorkItemService;

fn releases_request_body_accepts_candidate_objects() -> bool {
    let source = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/surfaces/web/static/js/features/settings_releases.js"),
    )
    .unwrap();
    source.contains("{ candidate, confirmed: true }")
}

fn extract_prefixed_string_literals(source: &str, prefix: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = source;
    while let Some(idx) = rest.find(prefix) {
        let after = &rest[idx + prefix.len()..];
        let Some(end) = after.find('"') else { break };
        values.push(after[..end].to_string());
        rest = &after[end + 1..];
    }
    values
}

fn extract_settings_guide_label_ids(source: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = source;
    while let Some(idx) = rest.find("renderSettingsGuideLabel(") {
        let after = &rest[idx + "renderSettingsGuideLabel(".len()..];
        if !after.trim_start().starts_with('"') {
            rest = &after[1..];
            continue;
        }
        let window = &after[..after.len().min(600)];
        let literals = string_literals(window);
        if let Some(id) = literals.get(1).filter(|id| !id.is_empty()) {
            ids.push(id.clone());
        }
        rest = &after[1..];
    }
    ids
}

fn string_literals(source: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }
        let mut value = String::new();
        let mut escaped = false;
        for ch in chars.by_ref() {
            if escaped {
                value.push(ch);
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '"' {
                break;
            }
            value.push(ch);
        }
        values.push(value);
    }
    values
}

fn operation_helper_process_spec(operation_id: &str) -> ManagedProcessSpec {
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
        authorization_command: Some("refine test operation helper".to_string()),
        sensitive: false,
        metadata: serde_json::from_value(json!({
            "kind": "runner",
            "worker_kind": "jira-export-test-helper",
            "operation_id": operation_id
        }))
        .unwrap(),
    }
}

fn wait_for_managed_pid_exit(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while managed_pid_is_alive(pid).unwrap_or(false) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

fn server_with_projection() -> InProcessWebServer {
    let mut goals = BTreeMap::new();
    goals.insert(
        "GOAL1".to_string(),
        GoalSummaryProjection {
            goal: GoalIndexProjection {
                id: "GOAL1".to_string(),
                name: "Projection route".to_string(),
                status: GoalStatus::Todo,
                priority: GoalPriority::Medium,
                reporter: Some("Buddy".to_string()),
                assignee: Some("Buddy".to_string()),
                round_count: 1,
                created: "created".to_string(),
                updated: "updated".to_string(),
                branch_name: None,
                node_id: Some("default".to_string()),
                feature_id: None,
                feature_order: None,
                json_path: "goals/01/GOAL1/goal.json".to_string(),
            },
            node_display_name: None,
            latest_round_prompt: None,
            searchable_text: "Projection route".to_string(),
            activity_ids: Vec::new(),
        },
    );

    InProcessWebServer {
        status: DaemonStatus {
            port: 8080,
            daemon_healthy: true,
            web_available: true,
            worker_state: "idle".to_string(),
            target_app_state: "detached".to_string(),
            launch_mode: "cargo".to_string(),
            executable_path: Some("cargo".to_string()),
            active_operations: Vec::new(),
            degraded_integrations: Vec::new(),
            lifecycle_evidence: None,
        },
        projection: ProjectionSnapshot {
            refine_dir: None,
            version: PROJECTION_SNAPSHOT_VERSION,
            generated_at: "now".to_string(),
            source_fingerprints: BTreeMap::new(),
            goals,
            features: BTreeMap::new(),
            activity: BTreeMap::new(),
            changes: BTreeMap::new(),
            dashboard: DashboardProjection::default(),
            runtime: RuntimeProjection::default(),
        },
        target_root: None,
        app_registry_root: None,
        runtime_root: None,
    }
}

fn git(repo: &Path, args: &[&str]) -> RefineResult<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RefineError::Conflict(
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .trim()
            .to_string(),
        ))
    }
}

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn init_git_app(repo: &Path) {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-b", "main"]).unwrap();
    git(repo, &["config", "user.email", "test@example.com"]).unwrap();
    git(repo, &["config", "user.name", "Test User"]).unwrap();
    fs::write(repo.join("app.txt"), "base\n").unwrap();
    git(repo, &["add", "app.txt"]).unwrap();
    git(repo, &["commit", "-m", "initial"]).unwrap();
    fs::create_dir_all(refine_dir_for_target_root(repo).unwrap()).unwrap();
}

fn seeded_remote_clone(temp_root: &Path) -> (PathBuf, PathBuf) {
    let remote = temp_root.join("remote.git");
    let seed = temp_root.join("seed");
    let app_root = temp_root.join("app");
    fs::create_dir_all(temp_root).unwrap();
    git(
        temp_root,
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    )
    .unwrap();
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-b", "main"]).unwrap();
    git(&seed, &["config", "user.email", "test@example.com"]).unwrap();
    git(&seed, &["config", "user.name", "Test User"]).unwrap();
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    )
    .unwrap();
    fs::write(seed.join("app.txt"), "initial\n").unwrap();
    git(&seed, &["add", "app.txt"]).unwrap();
    git(&seed, &["commit", "-m", "initial"]).unwrap();
    git(&seed, &["push", "-u", "origin", "main"]).unwrap();
    git(
        temp_root,
        &[
            "clone",
            remote.to_str().unwrap(),
            app_root.to_str().unwrap(),
        ],
    )
    .unwrap();
    (seed, app_root)
}

fn wait_for_http_request_metrics(
    runtime_root: &Path,
) -> Vec<crate::tools::observability::metrics::PerformanceEvent> {
    wait_for_http_request_metric_count(runtime_root, 1)
}

fn wait_for_http_request_metric_count(
    runtime_root: &Path,
    expected: usize,
) -> Vec<crate::tools::observability::metrics::PerformanceEvent> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let report = FileMetricsService::new(runtime_root)
            .report(PerformanceQuery {
                operation: Some("http.request".to_string()),
                ..PerformanceQuery::default()
            })
            .unwrap();
        if report.events.len() >= expected {
            return report.events;
        }
        if Instant::now() >= deadline {
            return report.events;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_project_sync_operation(
    runtime_root: &Path,
    response: &ApiResponse,
    expected: OperationState,
) -> OperationHandle {
    assert_eq!(response.status, 202, "{:#}", response.body);
    let operation_id = response.body["operation"]["id"]
        .as_str()
        .expect("project sync response should include an operation id");
    wait_for_operation_status(
        &FileOperationRegistry::new(runtime_root),
        operation_id,
        expected,
    )
}

fn wait_for_operation_status(
    registry: &FileOperationRegistry,
    operation_id: &str,
    expected: OperationState,
) -> OperationHandle {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(operation) = registry.status(operation_id)
            && operation.state == expected
        {
            return operation;
        }
        if Instant::now() >= deadline {
            let latest = registry.status(operation_id).ok();
            panic!(
                "timed out waiting for operation {operation_id} to reach {expected:?}: {latest:?}"
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_chat_read_line(
    server: &InProcessWebServer,
    session_id: &str,
    needle: &str,
) -> ApiResponse {
    let mut lines = Vec::new();
    let mut progress_lines = Vec::new();
    for _ in 0..100 {
        let mut read = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/chat/{session_id}/read"),
            body: None,
        });
        if let Some(values) = read.body.get("lines").and_then(|value| value.as_array()) {
            lines.extend(values.iter().cloned());
        }
        if let Some(values) = read
            .body
            .get("progress_lines")
            .and_then(|value| value.as_array())
        {
            progress_lines.extend(values.iter().cloned());
        }
        let has_line = lines
            .iter()
            .any(|line| line.as_str().unwrap_or("").contains(needle));
        if has_line {
            read.body["lines"] = serde_json::Value::Array(lines);
            read.body["progress_lines"] = serde_json::Value::Array(progress_lines);
            return read;
        }
        thread::sleep(Duration::from_millis(25));
    }
    server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/chat/{session_id}/read"),
        body: None,
    })
}

fn write_fake_provider(refine_dir: &Path, name: &str, exit_code: i32, output: &str) {
    let bin_dir = refine_dir.join("provider-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let path = bin_dir.join(name);
    let mut file = fs::File::create(&path).unwrap();
    writeln!(
        file,
        "#!/bin/sh\nprintf '%s\\n' {output:?}\nexit {exit_code}"
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_root = std::env::temp_dir()
        .canonicalize()
        .unwrap_or_else(|_| std::env::temp_dir());
    temp_root.join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

/// Remove a test's temp tree, tolerating writers that outlive the request.
///
/// Request metrics are recorded on a detached thread so they stay off the
/// response path (`record_http_request_metric`), so a metric line can land in
/// `runtime_root` just after the assertions finish. Landing mid-walk makes
/// `remove_dir_all` fail with `DirectoryNotEmpty`, and landing after it
/// recreates the tree, so retry briefly instead of failing the test for it.
fn remove_temp_dir(temp_root: impl AsRef<Path>) {
    let temp_root = temp_root.as_ref();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let outcome = fs::remove_dir_all(temp_root);
        match &outcome {
            Ok(()) if !temp_root.exists() => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            _ => {}
        }
        if std::time::Instant::now() >= deadline {
            if let Err(error) = outcome {
                panic!("failed to remove {}: {error}", temp_root.display());
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn percent_encode_for_test(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

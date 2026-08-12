mod configuration;
mod execution;
mod results;

use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessOwner, ProcessSupervisor,
};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::RefineError;
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::agent_providers::smoke_ai_env_lock;
use crate::tools::host::git_worktrees::FileGitWorktreeService;
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::work_items::FileWorkItemService;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::service::SETTINGS_MIGRATION_VERSION;
use super::*;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

fn quality_operation_metadata(runtime_root: &PathBuf) -> serde_json::Map<String, Value> {
    let operation = FileOperationRegistry::new(runtime_root)
        .register("quality:test")
        .unwrap();
    serde_json::Map::from_iter([("operation_id".to_string(), json!(operation.id))])
}

fn legacy_quality_node(id: &str, timing: &str, commands: &str) -> Value {
    json!({
        "id": id,
        "display_name": id,
        "created_at": "2026-07-22T00:00:00Z",
        "updated_at": "2026-07-22T00:00:00Z",
        "settings": {
            "quality_enabled": "1",
            "quality_timing": timing,
            "target_app_test_commands": commands
        }
    })
}

fn init_git_candidate(root: &PathBuf) -> String {
    fs::create_dir_all(root).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "quality@example.com"],
        vec!["config", "user.name", "Quality Test"],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(root.join("candidate.txt"), "candidate\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "."])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "-m", "candidate"])
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn git_output(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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
    String::from_utf8(output.stdout).unwrap()
}

fn make_executable(path: &PathBuf) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn restore_smoke_ai(previous: Option<std::ffi::OsString>) {
    unsafe {
        if let Some(previous) = previous {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
}

struct GoalQualityFixture {
    temp_root: PathBuf,
    candidate_root: PathBuf,
    refine_dir: PathBuf,
    runtime_root: PathBuf,
    smoke_ai: PathBuf,
}

struct LinkedGoalQualityFixture {
    temp_root: PathBuf,
    target_root: PathBuf,
    candidate_root: PathBuf,
    refine_dir: PathBuf,
    runtime_root: PathBuf,
    branch: String,
    candidate: String,
}

impl GoalQualityFixture {
    fn runner(&self) -> QualityOperationRunner {
        QualityOperationRunner::new(&self.refine_dir, &self.runtime_root, &self.candidate_root)
    }
}

fn goal_quality_fixture(prefix: &str, provider_body: &str) -> GoalQualityFixture {
    let temp_root = unique_temp_dir(prefix);
    let candidate_root = temp_root.join("candidate");
    let refine_dir = temp_root.join("state");
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(&smoke_ai, format!("#!/bin/sh\n{provider_body}\n")).unwrap();
    make_executable(&smoke_ai);
    let candidate_commit = init_git_candidate(&candidate_root);
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&candidate_root)
            .args(["branch", "-m", "refine/GOAL1/round-1"])
            .status()
            .unwrap()
            .success()
    );
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Quality candidate", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "test", "Verify candidate")
        .unwrap();
    work_items
        .update_goal_git_refs(
            "GOAL1",
            "refine/GOAL1/round-1",
            "main",
            &candidate_commit,
            Some(&candidate_commit),
        )
        .unwrap();
    FileQualityService::new(&refine_dir)
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Outcome works".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    GoalQualityFixture {
        temp_root,
        candidate_root,
        refine_dir,
        runtime_root,
        smoke_ai,
    }
}

fn linked_goal_quality_fixture(prefix: &str) -> LinkedGoalQualityFixture {
    let temp_root = unique_temp_dir(prefix);
    let target_root = temp_root.join("target");
    let runtime_root = temp_root.join("run/8080");
    let candidate = init_git_candidate(&target_root);
    let branch = "refine/GOAL1/round-1".to_string();
    let candidate_root = target_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    fs::create_dir_all(candidate_root.parent().unwrap()).unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&target_root)
            .args([
                "worktree",
                "add",
                "-b",
                &branch,
                candidate_root.to_str().unwrap()
            ])
            .status()
            .unwrap()
            .success()
    );
    let refine_dir =
        crate::tools::host::project_layout::refine_dir_for_target_root(&target_root).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Quality candidate", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Verify candidate")
        .unwrap();
    work_items
        .update_goal_git_refs("GOAL1", &branch, "main", &candidate, Some(&candidate))
        .unwrap();
    FileQualityService::new(&refine_dir)
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Outcome works".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();
    LinkedGoalQualityFixture {
        temp_root,
        target_root,
        candidate_root,
        refine_dir,
        runtime_root,
        branch,
        candidate,
    }
}

fn set_fixture_goal_node(fixture: &GoalQualityFixture, node_id: &str) {
    let summary = FileWorkItemService::new(&fixture.refine_dir)
        .show_goal_summary("GOAL1")
        .unwrap();
    let path = fixture.refine_dir.join(summary.goal.json_path);
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["node_id"] = json!(node_id);
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

fn wait_for_operation_process(
    runtime_root: &PathBuf,
    operation_id: &str,
) -> crate::process::subprocess::ManagedProcess {
    for _ in 0..200 {
        if let Some(process) = FileProcessSupervisor::new(runtime_root)
            .list()
            .unwrap()
            .into_iter()
            .find(|process| {
                process
                    .details
                    .as_deref()
                    .unwrap_or("")
                    .contains(operation_id)
            })
        {
            return process;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("managed process for operation {operation_id} was not observed");
}

fn wait_for_path(path: &Path) {
    for _ in 0..200 {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("path {} was not created", path.display());
}

fn wait_for_quality_state(refine_dir: &PathBuf, expected: &str) {
    for _ in 0..200 {
        if FileWorkItemService::new(refine_dir)
            .show_goal_detail("GOAL1")
            .ok()
            .and_then(|detail| {
                detail["rounds"][0]["quality_state"]
                    .as_str()
                    .map(str::to_string)
            })
            .as_deref()
            == Some(expected)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("Goal Quality state did not reach {expected}");
}

fn wait_for_process_exit(process: &crate::process::subprocess::ManagedProcess) {
    for _ in 0..200 {
        if !FileProcessSupervisor::process_is_alive(process).unwrap() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("managed process {} did not exit", process.id);
}

fn wait_for_operation_state(runtime_root: &PathBuf, operation_id: &str, expected: OperationState) {
    for _ in 0..200 {
        let state = FileOperationRegistry::new(runtime_root)
            .status(operation_id)
            .unwrap()
            .state;
        if state == expected {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("operation {operation_id} did not reach {expected:?}");
}

fn wait_for_no_operation_process(runtime_root: &PathBuf, operation_id: &str) {
    for _ in 0..200 {
        if FileProcessSupervisor::new(runtime_root)
            .list()
            .unwrap()
            .iter()
            .all(|process| {
                !process
                    .details
                    .as_deref()
                    .unwrap_or("")
                    .contains(operation_id)
            })
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("managed process for operation {operation_id} did not exit");
}

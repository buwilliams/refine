mod bulk;
mod cleanup;
mod intent;
mod ownership;
mod settlement;
mod termination;
mod worktree_retention;

use std::fs;
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::*;
use crate::process::subprocess::{ManagedProcessSpec, managed_pid_is_alive};
use crate::workflow::capacity::{AgentCapacityRequest, AgentCapacityService};
use crate::workflow::{WorkflowAutomation, WorkflowClaim, WorkflowPolicy};

const TEST_AGENT_LIFETIME_SECONDS: &str = "300";

#[cfg(unix)]
fn launch_agent(
    supervisor: &FileProcessSupervisor,
    goal_id: &str,
    command: Option<(&str, Vec<&str>)>,
) -> ManagedProcess {
    launch_agent_with_metadata(supervisor, goal_id, command, Map::new())
}

fn launch_workflow_agent(
    supervisor: &FileProcessSupervisor,
    goal_id: &str,
    claim_id: &str,
    execution_id: &str,
    round_idx: usize,
) -> ManagedProcess {
    let runtime_root = supervisor
        .runtime_root
        .parent()
        .unwrap_or(&supervisor.runtime_root);
    write_workflow_state(
        runtime_root,
        json!([{
            "claim_id": claim_id,
            "goal_id": goal_id,
            "execution_id": execution_id,
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );
    launch_agent_with_metadata(
        supervisor,
        goal_id,
        None,
        Map::from_iter([
            ("claim_id".to_string(), json!(claim_id)),
            ("execution_id".to_string(), json!(execution_id)),
            ("round_idx".to_string(), json!(round_idx)),
            ("workflow_state".to_string(), json!("in-progress")),
        ]),
    )
}

fn register_workflow_agent(
    supervisor: &FileProcessSupervisor,
    goal_id: &str,
    claim_id: &str,
    execution_id: &str,
    round_idx: usize,
) -> ManagedProcess {
    let child = if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", "ping -n 300 127.0.0.1 >NUL"])
            .spawn()
            .unwrap()
    } else {
        Command::new("sleep")
            .arg(TEST_AGENT_LIFETIME_SECONDS)
            .spawn()
            .unwrap()
    };
    supervisor
        .register(ManagedProcess {
            id: format!("registered-{goal_id}"),
            owner: ProcessOwner::Agent,
            pid: Some(child.id()),
            state: "running".to_string(),
            label: Some("registered workflow agent".to_string()),
            details: Some(
                json!({
                    "kind": "workflow",
                    "goal_id": goal_id,
                    "claim_id": claim_id,
                    "execution_id": execution_id,
                    "round_idx": round_idx,
                    "workflow_state": "in-progress"
                })
                .to_string(),
            ),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: "registered-for-process-control-test".to_string(),
            exit_code: None,
        })
        .unwrap()
}

fn launch_agent_with_metadata(
    supervisor: &FileProcessSupervisor,
    goal_id: &str,
    command: Option<(&str, Vec<&str>)>,
    extra_metadata: Map<String, Value>,
) -> ManagedProcess {
    let (command, args) = command
        .map(|(command, args)| {
            (
                command.to_string(),
                args.into_iter().map(str::to_string).collect(),
            )
        })
        .unwrap_or_else(|| {
            if cfg!(windows) {
                (
                    "cmd".to_string(),
                    vec!["/C".to_string(), "ping -n 300 127.0.0.1 >NUL".to_string()],
                )
            } else {
                (
                    "sleep".to_string(),
                    vec![TEST_AGENT_LIFETIME_SECONDS.to_string()],
                )
            }
        });
    let mut metadata = Map::from_iter([
        ("kind".to_string(), json!("interactive_session")),
        ("provider".to_string(), json!("smoke-ai")),
        ("goal_id".to_string(), json!(goal_id)),
    ]);
    metadata.extend(extra_metadata);
    supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command,
            args,
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata,
        })
        .unwrap()
}

fn create_in_progress_goal(refine_dir: &Path, goal_id: &str) {
    let service = FileWorkItemService::new(refine_dir);
    service
        .create_goal_summary("Process control test", Some(goal_id))
        .unwrap();
    service
        .transition_goal_status(goal_id, GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status(goal_id, GoalStatus::InProgress)
        .unwrap();
}

fn create_in_progress_goal_with_rounds(refine_dir: &Path, goal_id: &str, rounds: usize) {
    create_in_progress_goal_with_rounds_for_node(refine_dir, goal_id, rounds, "default");
}

fn create_in_progress_goal_with_rounds_for_node(
    refine_dir: &Path,
    goal_id: &str,
    rounds: usize,
    node_id: &str,
) {
    let service = FileWorkItemService::for_node(refine_dir, node_id);
    service
        .create_goal_summary("Process control workflow test", Some(goal_id))
        .unwrap();
    for round in 0..rounds {
        service
            .append_goal_round_summary(goal_id, "Process Control", &format!("Round {}", round + 1))
            .unwrap();
    }
    service
        .transition_goal_status(goal_id, GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status(goal_id, GoalStatus::InProgress)
        .unwrap();
}

fn write_workflow_state(runtime_root: &Path, claims: Value) {
    write_workflow_state_with_policy(runtime_root, claims, &WorkflowPolicy::default());
}

fn write_workflow_state_with_policy(runtime_root: &Path, claims: Value, policy: &WorkflowPolicy) {
    fs::create_dir_all(runtime_root).unwrap();
    fs::write(
        runtime_root.join(crate::workflow::WORKFLOW_AUTOMATION_STATE_FILE),
        serde_json::to_vec_pretty(&json!({
            "policy": policy,
            "claims": claims,
            "updated_at": "2026-07-23T00:02:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
}

fn reserve_workflow_capacity(runtime_root: &Path, claim_id: &str) {
    reserve_workflow_capacity_with_policy(runtime_root, claim_id, &WorkflowPolicy::default());
}

fn reserve_workflow_capacity_with_policy(
    runtime_root: &Path,
    claim_id: &str,
    policy: &WorkflowPolicy,
) {
    let acquired = AgentCapacityService::new(runtime_root)
        .try_acquire(
            policy,
            AgentCapacityRequest {
                owner_id: format!("workflow:{claim_id}"),
                role: "workflow".to_string(),
                node_id: policy.active_node_id.clone(),
                provider: policy.provider.clone(),
                target_app_id: policy.target_app_id.clone(),
            },
        )
        .unwrap();
    assert!(acquired);
}

fn non_default_workflow_policy() -> WorkflowPolicy {
    WorkflowPolicy {
        global_limit: 7,
        per_node_limit: 5,
        per_provider_limit: 4,
        per_target_app_limit: 3,
        active_node_id: "node-policy".to_string(),
        provider: "provider-policy".to_string(),
        target_app_id: "/srv/non-default-target".to_string(),
    }
}

fn assert_partial_cleanup_receipt(
    runtime_root: &Path,
    process_id: &str,
    registry_cleanup_completed: bool,
    identity_cleanup_completed: bool,
    cause: &str,
) {
    let receipt: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{process_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["state"], "partial_failure");
    assert_eq!(receipt["confirmed_exit"], true);
    assert_eq!(
        receipt["registry_cleanup_completed"],
        registry_cleanup_completed
    );
    assert_eq!(
        receipt["identity_cleanup_completed"],
        identity_cleanup_completed
    );
    assert_eq!(receipt["goal_cancelled"], false);
    assert!(receipt["cause"].as_str().unwrap().contains(cause));
    assert!(
        receipt["recovery"]
            .as_str()
            .unwrap()
            .contains("shared Process capability")
    );
}

fn init_git_target(temp_root: &Path) -> (PathBuf, PathBuf) {
    let target_root = temp_root.join("target");
    let refine_dir = target_root.join(".refine");
    fs::create_dir_all(&target_root).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "refine@example.invalid"],
        vec!["config", "user.name", "Refine Test"],
        vec!["commit", "--allow-empty", "-q", "-m", "base"],
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(&target_root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    (target_root, refine_dir)
}

fn add_test_worktree(target_root: &Path, branch: &str, name: &str) -> PathBuf {
    let worktree = target_root.join(".git/refine-worktrees").join(name);
    let output = Command::new("git")
        .arg("-C")
        .arg(target_root)
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            branch,
            worktree.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    worktree
}

fn assert_branch_exists(target_root: &Path, branch: &str) {
    let output = Command::new("git")
        .arg("-C")
        .arg(target_root)
        .args(["rev-parse", "--verify", &format!("refs/heads/{branch}")])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_for_exit(pid: u32) {
    for _ in 0..100 {
        if !managed_pid_is_alive(pid).unwrap_or(false) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn force_kill(pid: u32) {
    #[cfg(windows)]
    Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .unwrap();
    #[cfg(not(windows))]
    Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

/// Remove a test's temp tree, tolerating the supervisor's reaper thread.
///
/// `launch` spawns a thread that waits on the child and then cleans up that
/// process's artifacts, so it keeps deleting files under the temp root after
/// `stop` has already returned. Racing `remove_dir_all` against it fails the
/// walk on an entry that vanished underneath it, which surfaces as a rare
/// teardown panic in whichever test the scheduler happened to delay. Retry
/// briefly instead of failing a test for it; every assertion has already run.
fn remove_temp_dir(temp_root: impl AsRef<Path>) {
    let temp_root = temp_root.as_ref();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
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
        std::thread::sleep(Duration::from_millis(10));
    }
}

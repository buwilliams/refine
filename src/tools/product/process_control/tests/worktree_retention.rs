use super::*;
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};

#[derive(Clone, Copy)]
enum WorktreeContents {
    Pristine,
    TrackedDirty,
    OrdinaryUntracked,
    IgnoredUntracked,
    ConcurrentlyMutating,
}

#[test]
fn goal_agent_stop_never_removes_any_workflow_worktree() {
    for (suffix, contents) in [
        ("pristine", WorktreeContents::Pristine),
        ("tracked-dirty", WorktreeContents::TrackedDirty),
        ("ordinary-untracked", WorktreeContents::OrdinaryUntracked),
        ("ignored-untracked", WorktreeContents::IgnoredUntracked),
        (
            "concurrently-mutating",
            WorktreeContents::ConcurrentlyMutating,
        ),
    ] {
        assert_stop_retains_worktree(suffix, contents);
    }
}

fn assert_stop_retains_worktree(suffix: &str, contents: WorktreeContents) {
    let temp_root = unique_temp_dir(&format!("process-control-retain-{suffix}"));
    let runtime_root = temp_root.join("run/8080");
    let (target_root, refine_dir) = init_git_target(&temp_root);
    let goal_id = format!("GOAL-RETAIN-{}", suffix.to_uppercase());
    let claim_id = format!("claim-retain-{suffix}");
    let execution_id = format!("exec-retain-{suffix}");
    let branch = format!("refine/{goal_id}/round-1");
    let worktree = add_test_worktree(&target_root, &branch, &format!("refine-{goal_id}-round-1"));

    match contents {
        WorktreeContents::Pristine | WorktreeContents::ConcurrentlyMutating => {}
        WorktreeContents::TrackedDirty => {
            let tracked = worktree.join("tracked-agent-work.txt");
            fs::write(&tracked, "committed\n").unwrap();
            run_git(&worktree, &["add", "tracked-agent-work.txt"]);
            run_git(&worktree, &["commit", "-q", "-m", "tracked fixture"]);
            fs::write(&tracked, "uncommitted\n").unwrap();
        }
        WorktreeContents::OrdinaryUntracked => {
            fs::write(worktree.join("untracked-agent-work.txt"), "preserve me\n").unwrap();
        }
        WorktreeContents::IgnoredUntracked => {
            fs::write(worktree.join(".gitignore"), "ignored-agent-output.log\n").unwrap();
            fs::write(
                worktree.join("ignored-agent-output.log"),
                "preserve ignored output\n",
            )
            .unwrap();
        }
    }

    create_in_progress_goal_with_rounds_for_node(&refine_dir, &goal_id, 1, "remote-owner");
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": claim_id,
            "goal_id": goal_id,
            "node_id": "remote-owner",
            "execution_id": execution_id,
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );
    reserve_workflow_capacity(&runtime_root, &claim_id);
    let operation = FileOperationRegistry::new(&runtime_root)
        .register_with_request("stop-retention-test", json!({"execution_id": execution_id}))
        .unwrap();
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent_with_metadata(
        &supervisor,
        &goal_id,
        None,
        Map::from_iter([
            ("claim_id".to_string(), json!(claim_id)),
            ("execution_id".to_string(), json!(execution_id)),
            ("round_idx".to_string(), json!(0)),
            ("workflow_state".to_string(), json!("in-progress")),
            ("cwd".to_string(), json!(worktree.display().to_string())),
            (
                "worktree".to_string(),
                json!({"path": worktree, "branch": branch}),
            ),
        ]),
    );
    let mut control = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir);
    if matches!(contents, WorktreeContents::ConcurrentlyMutating) {
        let concurrent_worktree = worktree.clone();
        control = control.with_post_exit_hook(move || {
            fs::write(
                concurrent_worktree.join("concurrent-agent-output.log"),
                "arrived after process exit\n",
            )
            .unwrap();
        });
    }

    let stopped = control.stop(&process.id, "terminate").unwrap();

    assert_eq!(stopped["stopped"], true, "{suffix}");
    assert_eq!(stopped["goal"]["status"], "todo", "{suffix}");
    assert_eq!(stopped["worktree_retention"]["retained"], true, "{suffix}");
    assert_eq!(
        stopped["worktree_retention"]["worktrees"][0]["path"],
        worktree.canonicalize().unwrap().display().to_string(),
        "{suffix}"
    );
    assert!(stopped.get("partial_outcome").is_none(), "{suffix}");
    assert!(stopped.get("worktree_cleanup").is_none(), "{suffix}");
    assert!(worktree.exists(), "{suffix}");
    assert_branch_exists(&target_root, &branch);
    if matches!(contents, WorktreeContents::ConcurrentlyMutating) {
        assert_eq!(
            fs::read_to_string(worktree.join("concurrent-agent-output.log")).unwrap(),
            "arrived after process exit\n"
        );
    }
    assert_eq!(
        FileWorkItemService::for_node(&refine_dir, "remote-owner")
            .show_goal_summary(&goal_id)
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo,
        "{suffix}"
    );
    assert_eq!(
        WorkflowEngine::new(&runtime_root)
            .load_state()
            .unwrap()
            .claims[0]
            .state,
        WorkflowClaimState::Cancelled,
        "{suffix}"
    );
    assert!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty(),
        "{suffix}"
    );
    assert_eq!(
        FileOperationRegistry::new(&runtime_root)
            .status(&operation.id)
            .unwrap()
            .state,
        OperationState::Cancelled,
        "{suffix}"
    );

    let process_receipt: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{}.json", process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(process_receipt["state"], "completed", "{suffix}");
    assert_eq!(process_receipt["goal_requeued"], true, "{suffix}");
    assert_eq!(process_receipt["claim_cancelled"], true, "{suffix}");
    assert_eq!(
        process_receipt["worktree_retention"]["retained"], true,
        "{suffix}"
    );
    assert!(
        process_receipt["recovery"]
            .as_str()
            .is_some_and(|message| message.contains("human-controlled cleanup")),
        "{suffix}"
    );

    let settlement_receipt: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("workflow-cancellation-{goal_id}-{claim_id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(settlement_receipt["schema_version"], 5, "{suffix}");
    assert_eq!(settlement_receipt["state"], "committed", "{suffix}");
    assert_eq!(settlement_receipt["goal_requeued"], true, "{suffix}");
    assert_eq!(settlement_receipt["claim_cancelled"], true, "{suffix}");
    assert_eq!(settlement_receipt["capacity_released"], true, "{suffix}");
    assert_eq!(settlement_receipt["worktrees_retained"], true, "{suffix}");
    assert!(
        settlement_receipt.get("worktree_cleanup").is_none(),
        "{suffix}"
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn legacy_multi_target_cleanup_journal_replays_as_retention_without_removal() {
    let temp_root = unique_temp_dir("process-control-legacy-multi-retention");
    let runtime_root = temp_root.join("run/8080");
    let (target_root, refine_dir) = init_git_target(&temp_root);
    let goal_id = "GOAL-LEGACY-MULTI";
    let claim_id = "claim-legacy-multi";
    let execution_id = "exec-legacy-multi";
    let first_branch = "refine/GOAL-LEGACY-MULTI/round-1-a";
    let second_branch = "refine/GOAL-LEGACY-MULTI/round-1-b";
    let first = add_test_worktree(&target_root, first_branch, "refine-legacy-multi-a");
    let second = add_test_worktree(&target_root, second_branch, "refine-legacy-multi-b");
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    write_workflow_state(
        &runtime_root,
        json!([{
            "claim_id": claim_id,
            "goal_id": goal_id,
            "execution_id": execution_id,
            "state": "running",
            "created_at": "2026-07-23T00:00:00Z",
            "updated_at": "2026-07-23T00:00:00Z"
        }]),
    );
    reserve_workflow_capacity(&runtime_root, claim_id);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_agent_with_metadata(
        &supervisor,
        goal_id,
        None,
        Map::from_iter([
            ("claim_id".to_string(), json!(claim_id)),
            ("execution_id".to_string(), json!(execution_id)),
            ("round_idx".to_string(), json!(0)),
            ("workflow_state".to_string(), json!("in-progress")),
            ("cwd".to_string(), json!(first.display().to_string())),
            (
                "worktree".to_string(),
                json!({"path": first, "branch": first_branch}),
            ),
        ]),
    );

    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .with_settlement_interruption(CancellationSettlementFailureStage::AfterGoalPersistence)
            .stop(&process.id, "terminate")
            .unwrap();
    }));
    assert!(interrupted.is_err());

    let journal_path = runtime_root
        .join("process-stop-outcomes")
        .join(format!("workflow-cancellation-{goal_id}-{claim_id}.json"));
    let mut legacy: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    legacy["schema_version"] = json!(3);
    legacy["state"] = json!("worktree_cleanup_progress");
    legacy["worktrees"] = json!([
        {"path": first.canonicalize().unwrap(), "branch": first_branch, "repository_root": target_root},
        {"path": second.canonicalize().unwrap(), "branch": second_branch, "repository_root": target_root}
    ]);
    legacy["worktree_cleanup"] = json!([{
        "worktree": legacy["worktrees"][0].clone(),
        "state": "removed",
        "message": "legacy cleanup intended removal before interruption"
    }]);
    legacy["worktree_cleanup_completed"] = json!(false);
    legacy.as_object_mut().unwrap().remove("worktrees_retained");
    write_json_receipt(&journal_path, &legacy).unwrap();

    let replayed = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
        .cancel_workflow_execution(execution_id)
        .unwrap();

    assert_eq!(replayed["settled_after_claim_cancellation"], true);
    assert_eq!(replayed["goal"]["status"], "cancelled");
    assert_eq!(replayed["worktree_retention"]["retained"], true);
    assert_eq!(
        replayed["worktree_retention"]["worktrees"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(first.exists());
    assert!(second.exists());
    assert_branch_exists(&target_root, first_branch);
    assert_branch_exists(&target_root, second_branch);

    let upgraded: Value = serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(upgraded["schema_version"], 5);
    assert_eq!(upgraded["state"], "committed");
    assert_eq!(upgraded["worktrees_retained"], true);
    assert!(upgraded.get("worktree_cleanup").is_none());
    let completed: Value = serde_json::from_slice(
        &fs::read(
            runtime_root
                .join("process-stop-outcomes")
                .join(format!("{}.json", process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(completed["state"], "completed");
    assert_eq!(completed["worktree_retention"]["retained"], true);

    remove_temp_dir(&temp_root);
}

#[test]
fn explicit_workflow_cancellation_remains_terminal() {
    let temp_root = unique_temp_dir("process-control-explicit-cancel-terminal");
    let runtime_root = temp_root.join("run/8080");
    let refine_dir = temp_root.join(".refine");
    let goal_id = "GOAL-EXPLICIT-CANCEL";
    let claim_id = "claim-explicit-cancel";
    let execution_id = "exec-explicit-cancel";
    create_in_progress_goal_with_rounds(&refine_dir, goal_id, 1);
    let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
    let process = launch_workflow_agent(&supervisor, goal_id, claim_id, execution_id, 0);
    reserve_workflow_capacity(&runtime_root, claim_id);

    let control = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir);
    let cancelled = control.cancel_workflow_execution(execution_id).unwrap();
    assert_eq!(cancelled["cancelled"], true);
    assert_eq!(cancelled["goal"]["status"], "cancelled");
    assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary(goal_id)
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );
    assert_eq!(
        WorkflowEngine::new(&runtime_root)
            .load_state()
            .unwrap()
            .claims[0]
            .state,
        WorkflowClaimState::Cancelled
    );
    assert!(
        AgentCapacityService::new(&runtime_root)
            .snapshot()
            .unwrap()
            .leases
            .is_empty()
    );

    let repeated = control.cancel_workflow_execution(execution_id).unwrap();
    assert_eq!(repeated["cancelled"], true);
    assert_eq!(repeated["goal"]["status"], "cancelled");

    remove_temp_dir(&temp_root);
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

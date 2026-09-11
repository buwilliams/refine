use super::*;
use crate::application::workflow::health::{assess_worker, assess_workflow_health};
use crate::infrastructure::process::subprocess::scheduler_observation::{
    SchedulerObservation, current_os_identity,
};

pub(super) fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir()
        .join(format!("refine-{name}-{}", uuid::Uuid::new_v4()))
        .join("run/8080");
    std::fs::create_dir_all(&root).unwrap();
    root
}
fn wait_tick(root: &Path, process: &ManagedProcess) -> SchedulerObservation {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(tick) = SchedulerObservation::read(root, &workflow_incarnation(process).unwrap())
        {
            if tick.sequence > 0 && tick.completed_cycle_ms.is_some() {
                return tick;
            }
        }
        assert!(
            Instant::now() < deadline,
            "worker did not tick: {:?}",
            FileProcessSupervisor::new(root).stream(&process.id)
        );
        thread::sleep(Duration::from_millis(20));
    }
}
fn stop_all(root: &Path) {
    let supervisor = FileProcessSupervisor::new(root);
    for group in supervisor.owned_groups().unwrap() {
        supervisor
            .stop_owned_group(&group, Duration::from_secs(2))
            .unwrap();
    }
}
fn launch(root: &Path, token: &str, command: &str, args: &[&str], worker: bool) -> ManagedProcess {
    FileProcessSupervisor::new(root).launch(ManagedProcessSpec { owner: if worker { ProcessOwner::Runner } else { ProcessOwner::Maintenance },
        command: command.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: None, env: Vec::new(), stdin: None, limits: None,
        authorization_command: None, sensitive: false, metadata: serde_json::from_value(json!({"worker_kind": if worker { "workflow" } else { "agent" }, "workflow_incarnation": token, "isolated_process_group": true, "goal_id": if worker { None } else { Some("GOAL1") }})).unwrap() }).unwrap()
}
fn observation(root: &Path, process: &ManagedProcess) -> SchedulerObservation {
    let now = chrono::Utc::now().timestamp_millis();
    SchedulerObservation {
        runtime_root: root.canonicalize().unwrap(),
        process_id: process.id.clone(),
        pid: process.pid.unwrap(),
        os_identity: current_os_identity(process.pid.unwrap()).unwrap().unwrap(),
        incarnation: workflow_incarnation(process).unwrap(),
        target_root: None,
        node_id: None,
        sequence: 1,
        tick_ms: now,
        completed_cycle_ms: Some(now),
        active_attempts: Default::default(),
        failure: None,
        retry_delays: Default::default(),
    }
}

#[test]
fn workflow_process_helper() {
    let Ok(root) = std::env::var("REFINE_TEST_WORKFLOW_ROOT") else {
        return;
    };
    run_worker(WORKFLOW_RUNNER, root.into(), None, None, None).unwrap();
}

#[test]
fn actual_non_ticking_worker_is_stopped_before_replacement_tick_is_accepted() {
    let root = root("actual-workflow-watchdog");
    let service = FileRunnerWorkerService::new(&root);
    let BackgroundWorkerEnsure::Running(old) =
        service.ensure_background_worker(WORKFLOW_RUNNER).unwrap()
    else {
        panic!()
    };
    let mut tick = wait_tick(&root, &old);
    assert!(assess_workflow_health(&root).healthy);
    assert_eq!(
        unsafe { libc::kill(old.pid.unwrap() as i32, libc::SIGSTOP) },
        0
    );
    tick.tick_ms -= 60_000;
    tick.completed_cycle_ms = Some(tick.tick_ms);
    tick.write(&root).unwrap();
    assert!(!assess_workflow_health(&root).healthy);
    assert!(service.ensure_background_worker(WORKFLOW_RUNNER).is_err());
    assert!(!FileProcessSupervisor::process_is_alive(&old).unwrap());
    // The bounded backoff cannot launch another process immediately.
    assert!(service.ensure_background_worker(WORKFLOW_RUNNER).is_err());
    thread::sleep(Duration::from_millis(1100));
    let BackgroundWorkerEnsure::Running(replacement) =
        service.ensure_background_worker(WORKFLOW_RUNNER).unwrap()
    else {
        panic!()
    };
    assert_ne!(old.id, replacement.id);
    assert_ne!(
        workflow_incarnation(&old),
        workflow_incarnation(&replacement)
    );
    wait_tick(&root, &replacement);
    service.ensure_background_worker(WORKFLOW_RUNNER).unwrap();
    assert!(assess_workflow_health(&root).healthy);
    assert!(root.join("workflow-recovery.json").exists());
    stop_all(&root);
    std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn ownership_pid_reuse_and_duplicate_workers_fail_closed() {
    let root = root("workflow-ownership");
    let token = uuid::Uuid::new_v4().to_string();
    let worker = launch(&root, &token, "/bin/sleep", &["60"], true);
    let mut tick = observation(&root, &worker);
    tick.write(&root).unwrap();
    assert!(assess_worker(&root, &worker, None, tick.tick_ms).healthy);
    tick.process_id = "another-registration".into();
    tick.write(&root).unwrap();
    assert!(!assess_worker(&root, &worker, None, tick.tick_ms).healthy);
    let supervisor = FileProcessSupervisor::new(&root);
    let mut group = supervisor.owned_groups().unwrap().remove(0);
    group
        .witnesses
        .insert(worker.pid.unwrap(), "reused-pid".into());
    assert!(
        supervisor
            .stop_owned_group(&group, Duration::from_millis(20))
            .is_err()
    );
    assert!(FileProcessSupervisor::process_is_alive(&worker).unwrap());
    let second = launch(
        &root,
        &uuid::Uuid::new_v4().to_string(),
        "/bin/sleep",
        &["60"],
        true,
    );
    assert!(
        FileRunnerWorkerService::new(&root)
            .ensure_background_worker(WORKFLOW_RUNNER)
            .unwrap_err()
            .to_string()
            .contains("multiple")
    );
    assert!(FileProcessSupervisor::process_is_alive(&second).unwrap());
    stop_all(&root);
    std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn stale_ticks_missing_corrupt_target_changed_and_long_work_are_distinct() {
    let root = root("workflow-health-cases");
    let token = uuid::Uuid::new_v4().to_string();
    let worker = launch(&root, &token, "/bin/sleep", &["60"], true);
    let now = chrono::Utc::now().timestamp_millis();
    assert_eq!(assess_worker(&root, &worker, None, now).state, "starting");
    assert!(!assess_worker(&root, &worker, None, now + 31_000).healthy);
    let mut tick = observation(&root, &worker);
    tick.node_id = Some("foreign-node".into());
    tick.write(&root).unwrap();
    assert_eq!(
        assess_worker(&root, &worker, None, now).state,
        "unavailable"
    );
    tick.node_id = None;
    tick.active_attempts.insert("LONG-GOAL".into());
    tick.write(&root).unwrap();
    assert!(assess_worker(&root, &worker, None, tick.tick_ms + 29_999).healthy);
    assert_eq!(
        assess_worker(&root, &worker, None, tick.tick_ms + 30_000).state,
        "stalled"
    );
    assert_eq!(
        assess_worker(&root, &worker, Some(&root), tick.tick_ms).state,
        "draining"
    );
    tick.active_attempts.clear();
    tick.write(&root).unwrap();
    assert_eq!(
        assess_worker(&root, &worker, Some(&root), tick.tick_ms).state,
        "target_changed"
    );
    FileProcessSupervisor::new(&root)
        .set_workflow_paused(true)
        .unwrap();
    assert_eq!(assess_workflow_health(&root).state, "paused");
    let path = crate::infrastructure::process::subprocess::scheduler_observation::scheduler_observation_path(&root, &token).unwrap();
    std::fs::write(path, "{bad").unwrap();
    assert!(!assess_workflow_health(&root).healthy);
    FileProcessSupervisor::new(&root)
        .set_background_worker_enabled("workflow", false)
        .unwrap();
    assert_eq!(assess_workflow_health(&root).state, "disabled");
    assert_eq!(restart_delay_ms(1), 1000);
    assert_eq!(restart_delay_ms(20), 300_000);
    stop_all(&root);
    std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn surviving_descendants_hold_capacity_until_group_exit() {
    let root = root("workflow-descendants");
    let process = launch(
        &root,
        &uuid::Uuid::new_v4().to_string(),
        "/bin/sh",
        &["-c", "sleep 60 & setsid sleep 60 & wait"],
        false,
    );
    let supervisor = FileProcessSupervisor::new(&root);
    let mut group = supervisor.owned_groups().unwrap().remove(0);
    let stale = group.clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        group = supervisor.observe_owned_group(&group).unwrap();
        if group.witnesses.len() >= 3 {
            break;
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        libc::kill(process.pid.unwrap() as i32, libc::SIGKILL);
    }
    thread::sleep(Duration::from_millis(30));
    assert!(supervisor.group_pending(&process).unwrap());
    let after_stale_observer = supervisor.observe_owned_group(&stale).unwrap();
    assert!(
        after_stale_observer.witnesses.len() >= 2,
        "a stale observer erased a surviving escaped descendant"
    );
    std::fs::remove_file(
        supervisor
            .processes_dir()
            .join(format!("{}.json", process.id)),
    )
    .unwrap();

    assert_eq!(
        WorkflowEngine::new(&root)
            .observed_execution_load()
            .unwrap()
            .global,
        1
    );
    assert!(
        supervisor
            .stop_owned_group(&group, Duration::from_secs(2))
            .unwrap()
            .confirmed_exit
    );
    assert!(!supervisor.group_pending(&process).unwrap());
    assert_eq!(
        WorkflowEngine::new(&root)
            .observed_execution_load()
            .unwrap()
            .global,
        0
    );
    std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
}
#[test]
fn termination_failure_and_launch_contention_preserve_worker_and_evidence() {
    let root = root("workflow-stop-failure");
    let service = FileRunnerWorkerService::new(&root);
    let mut spec = background_worker_spec(Path::new("/bin/sleep"), &root, None, WORKFLOW_RUNNER);
    spec.command = "/bin/sleep".into();
    spec.args = vec!["60".into()];
    spec.metadata
        .insert("test_termination_failure".into(), json!(true));
    let supervisor = FileProcessSupervisor::new(&root);
    let worker = supervisor.launch(spec).unwrap();
    let mut tick = observation(&root, &worker);
    tick.tick_ms -= 60_000;
    tick.completed_cycle_ms = Some(tick.tick_ms);
    tick.write(&root).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("workflow-supervision.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    assert!(
        service
            .ensure_background_worker(WORKFLOW_RUNNER)
            .unwrap_err()
            .to_string()
            .contains("already in progress")
    );
    assert_eq!(supervisor.list().unwrap().len(), 1);
    FileExt::unlock(&lock).unwrap();
    assert!(
        service
            .ensure_background_worker(WORKFLOW_RUNNER)
            .unwrap_err()
            .to_string()
            .contains("injected group termination")
    );
    assert!(FileProcessSupervisor::process_is_alive(&worker).unwrap());
    let recovery: Value =
        serde_json::from_slice(&std::fs::read(root.join("workflow-recovery.json")).unwrap())
            .unwrap();
    assert_eq!(recovery["pending"], true);
    assert_eq!(recovery["stopped"], false);
    assert!(!assess_workflow_health(&root).healthy);
    // Remove only the test injection from the retained ownership snapshot to stop our fixture.
    let mut group = supervisor.owned_groups().unwrap().remove(0);
    let mut details: Value =
        serde_json::from_str(group.process.details.as_deref().unwrap()).unwrap();
    details["test_termination_failure"] = json!(false);
    group.process.details = Some(details.to_string());
    supervisor
        .stop_owned_group(&group, Duration::from_secs(2))
        .unwrap();
    std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn recovery_checks_unobserved_descendants_after_parent_reaping_and_registration_loss() {
    use crate::infrastructure::process::subprocess::owned_groups::test_fixture::UnobservedChild;
    for tracked in [false, true] {
        for remove_primary in [false, true] {
            let root = root("unobserved-recovery");
            let token = uuid::Uuid::new_v4().to_string();
            let worker = launch(&root, &token, "/bin/sleep", &["60"], true);
            let supervisor = FileProcessSupervisor::new(&root);
            let operations = FileOperationRegistry::new(&root);
            let operation = operations.register("unobserved:test").unwrap();
            let fixture = UnobservedChild::launch(
                &supervisor,
                tracked,
                json!({
                    "workflow_incarnation":token, "goal_id":"GOAL1", "operation_id":operation.id,
                    "agent_hard_cap_millis": if tracked { 600_000 } else { 1 }
                }),
            );
            assert!(fixture.child_alive());
            assert!(!fixture.group.witnesses.contains_key(&fixture.child_pid));
            if remove_primary {
                std::fs::remove_file(
                    supervisor
                        .processes_dir()
                        .join(format!("{}.json", fixture.group.process.id)),
                )
                .unwrap();
            }
            assert_eq!(
                WorkflowEngine::new(&root)
                    .observed_execution_load()
                    .unwrap()
                    .global,
                1
            );
            // For unavailable lifetime coverage, the real daemon maintenance lane must retain
            // the operation and report uncertainty without pretending the deadline was settled.
            if !tracked {
                let health = maintenance::maintain_daemon(&root);
                assert!(
                    health
                        .failures
                        .iter()
                        .any(|f| f.contains(&fixture.group.process.id))
                );
                assert_eq!(operations.status(&operation.id).unwrap(), operation);
                assert!(fixture.child_alive());
            }
            let mut tick = observation(&root, &worker);
            tick.tick_ms -= 60_000;
            tick.completed_cycle_ms = Some(tick.tick_ms);
            tick.write(&root).unwrap();
            let service = FileRunnerWorkerService::new(&root);
            assert!(service.ensure_background_worker(WORKFLOW_RUNNER).is_err());
            let recovery: Value = serde_json::from_slice(
                &std::fs::read(root.join("workflow-recovery.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(recovery["worker"]["id"], worker.id);
            assert_eq!(recovery["pending"], true);
            assert_eq!(recovery["stopped"], tracked);
            assert!(!FileProcessSupervisor::process_is_alive(&worker).unwrap());
            assert_eq!(
                WorkflowEngine::new(&root)
                    .observed_execution_load()
                    .unwrap()
                    .global,
                usize::from(!tracked)
            );
            assert_eq!(
                supervisor.group_pending(&fixture.group.process).unwrap(),
                !tracked
            );
            if tracked {
                assert!(!fixture.child_alive());
                assert!(
                    supervisor
                        .observe_owned_group(&fixture.group)
                        .unwrap()
                        .confirmed_exit
                );
            } else {
                assert!(fixture.child_alive());
                let health = assess_workflow_health(&root);
                assert!(!health.healthy);
                assert_eq!(health.state, "ownership_unverified");
                assert!(health.reason.contains(&fixture.group.process.id));
                assert!(health.remedy.contains("refine system doctor"));
                assert!(service.ensure_background_worker(WORKFLOW_RUNNER).is_err());
                fixture.kill_child();
                assert!(supervisor.group_pending(&fixture.group.process).unwrap());
                assert!(
                    root.join("owned-groups")
                        .join(format!("{}.json", fixture.group.process.id))
                        .exists()
                );
                assert_eq!(operations.status(&operation.id).unwrap(), operation);
            }
            drop(fixture);
            std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
        }
    }
}

#[test]
fn target_transition_grace_is_bounded_for_an_exact_incarnation() {
    let root = root("target-transition-bound");
    let worker = launch(&root, "target-transition", "/bin/sleep", &["60"], true);
    let service = FileRunnerWorkerService::new(&root);
    let now = chrono::Utc::now().timestamp_millis();
    assert!(
        service
            .target_transition_pending(&worker, Some(Path::new("new-target")), now)
            .unwrap()
    );
    assert!(
        service
            .target_transition_pending(&worker, Some(Path::new("new-target")), now + 29_999)
            .unwrap()
    );
    assert!(
        !service
            .target_transition_pending(&worker, Some(Path::new("new-target")), now + 30_000)
            .unwrap()
    );
    let mut newer = worker.clone();
    newer.id = "replacement".into();
    assert!(
        service
            .target_transition_pending(&newer, Some(Path::new("new-target")), now + 30_000)
            .unwrap()
    );
    stop_all(&root);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn ticking_worker_with_stalled_admission_cycle_enters_bounded_recovery() {
    let root = root("stalled-cycle");
    let worker = launch(
        &root,
        &uuid::Uuid::new_v4().to_string(),
        "/bin/sleep",
        &["60"],
        true,
    );
    let mut tick = observation(&root, &worker);
    tick.completed_cycle_ms = Some(tick.tick_ms - 60_000);
    tick.write(&root).unwrap();
    let service = FileRunnerWorkerService::new(&root);
    assert!(service.ensure_background_worker(WORKFLOW_RUNNER).is_err());
    let record: Value =
        serde_json::from_slice(&std::fs::read(root.join("workflow-recovery.json")).unwrap())
            .unwrap();
    assert_eq!(record["worker"]["id"], worker.id);
    assert_eq!(record["stopped"], true);
    assert_eq!(record["pending"], true);
    assert!(!FileProcessSupervisor::process_is_alive(&worker).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_group_evidence_cannot_authorize_replacement_after_leader_exit() {
    use crate::infrastructure::process::subprocess::owned_groups::test_fixture::UnobservedChild;
    let root = root("missing-group-proof");
    let owner = FileProcessSupervisor::new(root.join("agents"));
    let fixture = UnobservedChild::launch(
        &owner,
        false,
        json!({"workflow_incarnation": uuid::Uuid::new_v4().to_string(), "goal_id": "GOAL1"}),
    );
    std::fs::remove_file(
        root.join("agents/owned-groups")
            .join(format!("{}.json", fixture.group.process.id)),
    )
    .unwrap();
    assert!(fixture.child_alive());
    let result = FileRunnerWorkerService::new(&root).ensure_background_worker(WORKFLOW_RUNNER);
    assert!(result.unwrap_err().to_string().contains("still owns work"));
    assert!(owner.group_pending(&fixture.group.process).unwrap());
    let health = assess_workflow_health(&root);
    assert_eq!(health.state, "ownership_unverified");
    assert!(health.reason.contains("evidence is missing"));
    assert!(health.remedy.contains("system doctor"));
    drop(fixture);
    std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
}

#[test]
fn complete_exit_receipt_recovers_lost_group_before_launching_a_replacement_worker() {
    use crate::infrastructure::process::subprocess::owned_groups::test_fixture::UnobservedChild;
    let root = root("lost-group-complete-proof");
    let owner = FileProcessSupervisor::new(root.join("agents"));
    let fixture = UnobservedChild::launch(
        &owner,
        true,
        json!({"workflow_incarnation": "old-worker", "goal_id": "GOAL1"}),
    );
    let mut process = fixture.group.process.clone();
    // Drop kills and reaps the escaped child and waits for the launching reaper.
    drop(fixture);
    process.state = "exited".into();
    std::fs::write(
        owner.processes_dir().join(format!("{}.json", process.id)),
        serde_json::to_vec(&process).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(
        root.join("agents/owned-groups")
            .join(format!("{}.json", process.id)),
    )
    .unwrap();
    let service = FileRunnerWorkerService::new(&root);
    let BackgroundWorkerEnsure::Running(replacement) =
        service.ensure_background_worker(WORKFLOW_RUNNER).unwrap()
    else {
        panic!("replacement was not launched")
    };
    assert!(!owner.group_pending(&process).unwrap());
    wait_tick(&root, &replacement);
    service.ensure_background_worker(WORKFLOW_RUNNER).unwrap();
    assert!(assess_workflow_health(&root).healthy);
    assert!(
        root.join("agents/owned-groups")
            .join(format!("{}.json", process.id))
            .exists()
    );
    stop_all(&root);
    std::fs::remove_dir_all(root.parent().unwrap().parent().unwrap()).unwrap();
}

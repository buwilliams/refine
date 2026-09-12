use super::*;
#[test]
fn scheduler_recovers_transient_discovery_error_while_other_goal_is_alive() {
    let (root, workflow, items) = fixture("discovery-transient", 2);
    let (tx, rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release = Mutex::new(release_rx);
    let cycle = Arc::new(AtomicUsize::new(0));
    let seen = cycle.clone();
    test_hooks::install(
        &workflow.runtime_root,
        Arc::new(move |_, goal, stage, _| {
            if stage == "scheduler" && seen.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(RefineError::Io("temporary discovery read".into()));
            }
            if stage == "claimed" && goal == "GOAL1" {
                tx.send(()).unwrap();
                release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }
            if stage == "executing" {
                return Err(RefineError::Conflict("task ended".into()));
            }
            Ok(())
        }),
    );
    std::thread::scope(|scope| {
        let run = scope.spawn(|| workflow.execute_work());
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while cycle.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            cycle.load(Ordering::SeqCst) >= 3,
            "scheduler latched its transient error"
        );
        assert_eq!(
            items.show_goal_summary("GOAL2").unwrap().goal.status,
            GoalStatus::Todo
        );
        release_tx.send(()).unwrap();
        assert!(run.join().unwrap().is_err());
    });
    assert_eq!(
        items.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Failed
    );
    test_hooks::remove(&workflow.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn blocked_preparation_keeps_real_controller_ticks_and_reserves_its_slot() {
    use crate::infrastructure::process::subprocess::scheduler_observation::{
        SchedulerObservation, current_os_identity,
    };
    let (root, workflow, items) = fixture("ticking-preparation", 2);
    fs::create_dir_all(&workflow.runtime_root).unwrap();
    let token = uuid::Uuid::new_v4().to_string();
    let observation = SchedulerObservation {
        runtime_root: workflow.runtime_root.canonicalize().unwrap(),
        process_id: "test-controller".into(),
        pid: std::process::id(),
        os_identity: current_os_identity(std::process::id()).unwrap().unwrap(),
        incarnation: token.clone(),
        target_root: None,
        node_id: None,
        sequence: 0,
        tick_ms: 0,
        tick_monotonic_ms: None,
        completed_cycle_monotonic_ms: None,
        completed_cycle_ms: None,
        active_attempts: Default::default(),
        failure: None,
        retry_delays: Default::default(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release = Mutex::new(release_rx);
    test_hooks::install(
        &workflow.runtime_root,
        Arc::new(move |_, goal, stage, _| {
            if stage == "claimed" {
                if goal == "GOAL1" {
                    tx.send(()).unwrap();
                    release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                }
                return Err(RefineError::Io("preparation completed with failure".into()));
            }
            Ok(())
        }),
    );
    std::thread::scope(|scope| {
        let run = scope.spawn(|| {
            crate::application::workflow::health::install_test_observation(observation);
            workflow.execute_work()
        });
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let first = SchedulerObservation::read(&workflow.runtime_root, &token).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let later = SchedulerObservation::read(&workflow.runtime_root, &token).unwrap();
        assert!(later.sequence > first.sequence);
        assert!(later.active_attempts.contains("GOAL1"));
        assert_eq!(
            items.show_goal_summary("GOAL2").unwrap().goal.status,
            GoalStatus::Todo
        );
        release_tx.send(()).unwrap();
        assert!(run.join().unwrap().is_err());
    });
    assert_eq!(
        items.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Failed
    );
    test_hooks::remove(&workflow.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_invalidates_an_empty_discovery_captured_before_capacity_was_released() {
    let (root, workflow, items) = fixture("completion-discovery-race", 2);
    let (discovery_tx, discovery_rx) = std::sync::mpsc::channel();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel();
    let (scan_tx, scan_rx) = std::sync::mpsc::channel();
    let finish_rx = Mutex::new(finish_rx);
    let scan_rx = Mutex::new(scan_rx);
    let cycles = AtomicUsize::new(0);
    test_hooks::install(
        &workflow.runtime_root,
        Arc::new(move |_, goal, stage, _| {
            if stage == "scheduler" && cycles.fetch_add(1, Ordering::SeqCst) == 1 {
                discovery_tx.send(()).unwrap();
                scan_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
            }
            if stage == "executing" {
                if goal == "GOAL1" {
                    finish_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                }
                return Err(RefineError::Conflict("task completed".into()));
            }
            Ok(())
        }),
    );
    std::thread::scope(|scope| {
        let run = scope.spawn(|| workflow.execute_work());
        discovery_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        finish_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while items.show_goal_summary("GOAL1").unwrap().goal.status != GoalStatus::Failed {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        // Allow the controller to drain the ended attempt while discovery still holds its
        // original active-set snapshot. This ordering previously ended the pass early.
        std::thread::sleep(Duration::from_millis(100));
        let available = Instant::now();
        scan_tx.send(()).unwrap();
        assert!(run.join().unwrap().is_err());
        assert_eq!(
            items.show_goal_summary("GOAL2").unwrap().goal.status,
            GoalStatus::Failed
        );
        assert!(available.elapsed() < Duration::from_secs(1));
    });
    test_hooks::remove(&workflow.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_failed_attempt_releases_capacity_within_one_poll_while_another_goal_stays_active() {
    let (root, workflow, items) = fixture("same-controller-three-goals", 3);
    FileSettingsService::new(&items.refine_dir)
        .update(&json!({"parallel_run_cap":"2"}))
        .unwrap();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_one, wait_one) = std::sync::mpsc::channel();
    let (release_two, wait_two) = std::sync::mpsc::channel();
    let waits = [Mutex::new(wait_one), Mutex::new(wait_two)];
    let cycles = Arc::new(AtomicUsize::new(0));
    let count = cycles.clone();
    let released = Arc::new(Mutex::new(None::<Instant>));
    let clock = released.clone();
    let records = items.clone();
    test_hooks::install(
        &workflow.runtime_root,
        Arc::new(move |_, goal, stage, _| {
            if stage == "scheduler" && count.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(RefineError::Io(
                    "transient admission fault with two active Goals".into(),
                ));
            }
            if goal == "GOAL1" && stage == "delivery" {
                *clock.lock().unwrap() = Some(Instant::now());
            }
            if stage == "executing" {
                if goal == "GOAL3" {
                    assert!(clock.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1));
                    assert_eq!(
                        records.show_goal_summary("GOAL2").unwrap().goal.status,
                        GoalStatus::Plan
                    );
                    ready_tx.send(goal.to_string()).unwrap();
                } else {
                    ready_tx.send(goal.to_string()).unwrap();
                    waits[usize::from(goal == "GOAL2")]
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap();
                }
                return Err(RefineError::Conflict("test attempt ended".into()));
            }
            Ok(())
        }),
    );
    std::thread::scope(|scope| {
        let run = scope.spawn(|| workflow.execute_work());
        let mut ready = vec![
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        ];
        ready.sort();
        assert_eq!(ready, vec!["GOAL1", "GOAL2"]);
        let deadline = Instant::now() + Duration::from_secs(5);
        while cycles.load(Ordering::SeqCst) < 3 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        release_one.send(()).unwrap();
        assert_eq!(
            ready_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "GOAL3"
        );
        release_two.send(()).unwrap();
        assert!(run.join().unwrap().is_err());
    });
    test_hooks::remove(&workflow.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

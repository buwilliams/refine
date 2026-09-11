//! Standard workload result delivery and retained descendant capacity in one runner.
use super::*;
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner,
};
use std::thread;

#[test]
fn standard_capture_returns_while_pending_scope_blocks_followup_until_proven_release() {
    let (root, workflow, items) = fixture("standard-capture-admission", 3);
    FileSettingsService::new(&items.refine_dir)
        .update(&json!({"parallel_run_cap":"2"}))
        .unwrap();
    let returned = Arc::new(AtomicUsize::new(0));
    let unrelated_progress = Arc::new(AtomicUsize::new(0));
    let admitted = Arc::new(AtomicUsize::new(0));
    let release_clock = Arc::new(Mutex::new(None::<Instant>));
    let release_handle = Arc::new(Mutex::new(None));
    let records = items.clone();
    let (done, progress, next, clock, handle) = (
        returned.clone(),
        unrelated_progress.clone(),
        admitted.clone(),
        release_clock.clone(),
        release_handle.clone(),
    );
    test_hooks::install(
        &workflow.runtime_root,
        Arc::new(move |engine, goal, stage, _| {
            if stage != "executing" {
                return Ok(());
            }
            if goal == "GOAL3" {
                let elapsed = clock.lock().unwrap().unwrap().elapsed();
                assert!(
                    elapsed < Duration::from_secs(1),
                    "Followup admission took {elapsed:?} after scope release"
                );
                next.fetch_add(1, Ordering::SeqCst);
                return Err(RefineError::Conflict("followup admitted".into()));
            }
            if goal == "GOAL2" {
                let deadline = Instant::now() + Duration::from_secs(8);
                while done.load(Ordering::SeqCst) == 0 {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
                progress.store(1, Ordering::SeqCst);
                while next.load(Ordering::SeqCst) == 0 {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
                return Err(RefineError::Conflict("unrelated work completed".into()));
            }
            let cwd = engine.target_root.as_ref().unwrap().clone();
            let supervisor = FileProcessSupervisor::new(engine.runtime_root.join("agents"));
            let start = Instant::now();
            let output = supervisor.run_to_completion(ManagedProcessSpec {
            owner:ProcessOwner::Agent, command:"python3".into(), args:vec!["-c".into(), "import os,time\nfrom pathlib import Path\nif os.fork()==0:\n os.setsid(); Path('standard-child').write_text(str(os.getpid()))\n while not Path('standard-release').exists(): time.sleep(0.005)\n os._exit(0)\nwhile not Path('standard-child').exists(): time.sleep(0.005)\nos.write(1,b'workload-result'); os._exit(9)".into()],
            cwd:Some(cwd.display().to_string()), env:Vec::new(), stdin:None, limits:None, authorization_command:None, sensitive:false,
            metadata:serde_json::from_value(json!({"goal_id":goal,"workflow_incarnation":"standard-runner","isolated_process_group":true,"completion_timeout_seconds":5})).unwrap()
        })?;
            assert!(start.elapsed() < Duration::from_secs(2));
            assert_eq!(output.process.exit_code, Some(9));
            assert_eq!(output.stdout, "workload-result");
            assert!(!output.capture_complete());
            assert_eq!(supervisor.capacity_processes()?.len(), 1);
            let error =
                crate::application::workers::FileRunnerWorkerService::new(&engine.runtime_root)
                    .ensure_background_worker("workflow")
                    .unwrap_err()
                    .to_string();
            assert!(error.contains("still owns work"), "{error}");
            done.store(1, Ordering::SeqCst);
            let (progress, next, clock, records) = (
                progress.clone(),
                next.clone(),
                clock.clone(),
                records.clone(),
            );
            *handle.lock().unwrap() = Some(thread::spawn(move || {
                thread::sleep(Duration::from_millis(250));
                assert_eq!(
                    progress.load(Ordering::SeqCst),
                    1,
                    "unrelated work did not progress after capture returned"
                );
                assert_eq!(next.load(Ordering::SeqCst), 0);
                assert_eq!(
                    records.show_goal_summary("GOAL3").unwrap().goal.status,
                    GoalStatus::Todo
                );
                let child = fs::read_to_string(cwd.join("standard-child")).unwrap();
                assert!(Path::new(&format!("/proc/{child}")).exists());
                assert!(supervisor.group_pending(&output.process).unwrap());
                *clock.lock().unwrap() = Some(Instant::now());
                fs::write(cwd.join("standard-release"), "").unwrap();
                let deadline = Instant::now() + Duration::from_secs(1);
                while supervisor.group_pending(&output.process).unwrap() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
                assert!(supervisor.capacity_processes().unwrap().is_empty());
                assert!(!Path::new(&format!("/proc/{child}")).exists());
            }));
            Err(RefineError::Conflict(
                "standard workload completed with exit 9".into(),
            ))
        }),
    );
    assert!(workflow.execute_work().is_err());
    if let Some(handle) = release_handle.lock().unwrap().take() {
        handle.join().unwrap();
    }
    assert_eq!(returned.load(Ordering::SeqCst), 1);
    assert_eq!(admitted.load(Ordering::SeqCst), 1);
    test_hooks::remove(&workflow.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

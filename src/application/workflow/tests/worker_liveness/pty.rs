//! The same admission controller consumes a real PTY failure and retained capacity.
use super::*;
use crate::application::agents::sessions::{GoalAgentLaunch, run_goal_agent};
use crate::infrastructure::process::subprocess::FileProcessSupervisor;
use std::os::unix::fs::PermissionsExt;
use std::thread;

#[test]
fn pty_failure_admits_followup_within_poll_only_after_scope_release() {
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for uncertain in [false, true] {
        let (root, workflow, items) = fixture("pty-admission", 3);
        FileSettingsService::new(&items.refine_dir)
            .update(&json!({"parallel_run_cap":"2"}))
            .unwrap();
        let provider = root.join("provider");
        fs::write(&provider, "#!/bin/sh\nsetsid sh -c 'echo $$ > child.pid; exec sleep 60' &\nwhile [ ! -s child.pid ]; do sleep 0.005; done\necho workload-exited\nexit 7\n").unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
        }
        let released = Arc::new(Mutex::new(None::<Instant>));
        let clock = released.clone();
        let admitted = Arc::new(AtomicUsize::new(0));
        let next = admitted.clone();
        let release_thread = Arc::new(Mutex::new(None));
        let handle = release_thread.clone();
        let records = items.clone();
        test_hooks::install(
            &workflow.runtime_root,
            Arc::new(move |engine, goal, stage, _| {
                if stage != "executing" {
                    return Ok(());
                }
                if goal == "GOAL3" {
                    assert!(clock.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1));
                    next.fetch_add(1, Ordering::SeqCst);
                    return Err(RefineError::Conflict("followup admitted".into()));
                }
                if goal == "GOAL2" {
                    let deadline = Instant::now() + Duration::from_secs(8);
                    while next.load(Ordering::SeqCst) == 0 {
                        assert!(Instant::now() < deadline, "followup was not admitted");
                        thread::sleep(Duration::from_millis(10));
                    }
                    return Err(RefineError::Conflict("parallel task ended".into()));
                }
                let root = engine.runtime_root.join("agents");
                let cwd = engine.target_root.as_ref().unwrap().clone();
                let supervisor = FileProcessSupervisor::new(&root);
                let result = run_goal_agent(GoalAgentLaunch { runtime_root: root.clone(), cwd: cwd.clone(), provider: "smoke-ai".into(), prompt: "fail the PTY workload".into(), metadata: serde_json::from_value(json!({"goal_id":goal,"workflow_incarnation":"pty-runner", "test_termination_failure":uncertain})).unwrap(), completion_timeout: Some(Duration::from_secs(2)), idle_timeout: None, provider_session: None }, |_| {});
                let error = result.unwrap_err();
                let group = supervisor.owned_groups()?.remove(0);
                let child: u32 = fs::read_to_string(cwd.join("child.pid"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if uncertain {
                    assert_eq!(supervisor.capacity_processes()?.len(), 1);
                    let workers = crate::application::workers::FileRunnerWorkerService::new(
                        &engine.runtime_root,
                    );
                    let replacement = workers
                        .ensure_background_worker("workflow")
                        .unwrap_err()
                        .to_string();
                    assert!(replacement.contains("still owns work"), "{replacement}");
                    let next = next.clone();
                    let clock = clock.clone();
                    let records = records.clone();
                    *handle.lock().unwrap() = Some(thread::spawn(move || {
                        thread::sleep(Duration::from_millis(250));
                        assert_eq!(next.load(Ordering::SeqCst), 0);
                        assert_eq!(
                            records.show_goal_summary("GOAL3").unwrap().goal.status,
                            GoalStatus::Todo
                        );
                        assert!(Path::new(&format!("/proc/{child}")).exists());
                        let mut group = group;
                        let mut details: Value =
                            serde_json::from_str(group.process.details.as_ref().unwrap()).unwrap();
                        details["test_termination_failure"] = json!(false);
                        group.process.details = Some(details.to_string());
                        fs::write(
                            root.join("owned-groups")
                                .join(format!("{}.json", group.process.id)),
                            serde_json::to_vec(&group).unwrap(),
                        )
                        .unwrap();
                        supervisor
                            .stop_owned_group(&group, Duration::from_secs(2))
                            .unwrap();
                        assert!(!Path::new(&format!("/proc/{child}")).exists());
                        assert!(supervisor.capacity_processes().unwrap().is_empty());
                        *clock.lock().unwrap() = Some(Instant::now());
                    }));
                } else {
                    assert!(!Path::new(&format!("/proc/{child}")).exists());
                    assert!(!supervisor.group_pending(&group.process)?);
                    assert!(supervisor.capacity_processes()?.is_empty());
                    *clock.lock().unwrap() = Some(Instant::now());
                }
                Err(error)
            }),
        );
        assert!(workflow.execute_work().is_err());
        if let Some(handle) = release_thread.lock().unwrap().take() {
            handle.join().unwrap();
        }
        assert_eq!(admitted.load(Ordering::SeqCst), 1);
        assert_eq!(
            items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Failed
        );
        test_hooks::remove(&workflow.runtime_root);
        unsafe {
            match previous {
                Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
                None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
}

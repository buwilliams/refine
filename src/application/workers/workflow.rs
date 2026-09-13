use super::*;

pub(super) fn run_workflow_worker(
    runtime_root: &Path,
    project_registry_root: Option<&Path>,
) -> RefineResult<()> {
    let mut recovered_root = None;
    let mut retired_supervisor_root = None;
    loop {
        crate::application::workflow::health::scheduler_tick(
            runtime_root,
            recovered_root.as_deref(),
            &Default::default(),
            false,
            None,
        );
        // Every step here is retried on the next interval rather than propagated.
        // A transient failure — the app detaching, a registry read losing a race,
        // a lock held for a moment — must not end the tick loop: nothing restarts
        // it in place, so returning here silences automation until the daemon is
        // restarted, with a full queue and no surfaced error.
        let target_root = match current_target_root(runtime_root, project_registry_root) {
            Ok(target_root) => target_root,
            Err(error) => {
                eprintln!("refine workflow runner: failed to read the active app: {error}");
                thread::sleep(WORKFLOW_INTERVAL);
                continue;
            }
        };
        if target_root.is_none() {
            recovered_root = None;
            crate::application::workflow::health::scheduler_tick(
                runtime_root,
                None,
                &Default::default(),
                true,
                None,
            );
        }
        if let Some(target_root) = target_root {
            let root = target_root
                .canonicalize()
                .unwrap_or_else(|_| target_root.clone());
            let workflow = WorkflowEngine::with_target_root(runtime_root, &target_root);
            if retired_supervisor_root.as_ref() != Some(&root) {
                match retire_legacy_supervisor(runtime_root, &target_root) {
                    // Only record the root as retired once it actually succeeded,
                    // so a failed attempt is retried instead of skipped forever.
                    Ok(()) => retired_supervisor_root = Some(root.clone()),
                    Err(error) => {
                        eprintln!("refine workflow runner: {error}");
                        thread::sleep(WORKFLOW_INTERVAL);
                        continue;
                    }
                }
            }
            if recovered_root.as_ref() != Some(&root) {
                match recover_root(&mut recovered_root, &root, || {
                    workflow.recover_interrupted_goals(
                    "workflow runner restarted; nonterminal work remains schedulable from its synchronized state",
                )
                }) {
                    Ok(count) if count > 0 => {
                        let _ = refresh_projection(runtime_root, &target_root);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("refine workflow recovery: {error}");
                        crate::application::workflow::health::scheduler_tick(
                            runtime_root,
                            Some(&target_root),
                            &Default::default(),
                            false,
                            Some(&error.to_string()),
                        );
                        // Do not launch replacement writers until interrupted
                        // process cleanup succeeds. The same pass is retried.
                        thread::sleep(WORKFLOW_INTERVAL);
                        continue;
                    }
                }
            }
            if workflow.workflow_paused().unwrap_or(false) {
                crate::application::workflow::health::scheduler_tick(
                    runtime_root,
                    Some(&target_root),
                    &Default::default(),
                    true,
                    None,
                );
            }
            match workflow.evaluate_worker_workflow(project_registry_root.unwrap_or(runtime_root)) {
                Ok(result) if result.changed_projection() => {
                    let _ = refresh_projection(runtime_root, &target_root);
                }
                Ok(_) => {}
                Err(RefineError::Conflict(message)) if message.contains("paused") => {}
                Err(error) => {
                    eprintln!("refine workflow runner: {error}");
                }
            }
        }
        thread::sleep(WORKFLOW_INTERVAL);
    }
}

fn recover_root(
    recovered_root: &mut Option<PathBuf>,
    root: &Path,
    recover: impl FnOnce() -> RefineResult<usize>,
) -> RefineResult<usize> {
    let count = recover()?;
    *recovered_root = Some(root.to_path_buf());
    Ok(count)
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn failed_recovery_remains_pending_until_a_successful_pass() {
        let fixture =
            std::env::temp_dir().join(format!("refine-recovery-retry-{}", uuid::Uuid::new_v4()));
        let root = fixture.join("target");
        let runtime = fixture.join("runtime");
        std::fs::create_dir_all(&root).unwrap();
        let failure = runtime.join("workflow-automation-state.json");
        std::fs::create_dir_all(&failure).unwrap();
        let engine = WorkflowEngine::with_target_root(&runtime, &root);
        let mut recovered = None;
        assert!(
            recover_root(&mut recovered, &root, || engine
                .recover_interrupted_goals("retry after storage failure"))
            .is_err()
        );
        assert_eq!(recovered, None);
        std::fs::remove_dir(failure).unwrap();
        assert_eq!(
            recover_root(&mut recovered, &root, || engine
                .recover_interrupted_goals("retry after storage recovery"))
            .unwrap(),
            0
        );
        assert_eq!(recovered.as_deref(), Some(root.as_path()));
        std::fs::remove_dir_all(fixture).unwrap();
    }
}

pub(super) fn retire_legacy_supervisor(
    runtime_root: &Path,
    target_root: &Path,
) -> RefineResult<()> {
    let mut process_ids = Vec::new();
    for process_root in [runtime_root.to_path_buf(), runtime_root.join("agents")] {
        let supervisor = FileProcessSupervisor::new(&process_root);
        for process in supervisor.list()? {
            let details = process
                .details
                .as_deref()
                .and_then(|details| serde_json::from_str::<Value>(details).ok())
                .unwrap_or_else(|| json!({}));
            let retired = details.get("agent_role").and_then(Value::as_str) == Some("supervisor")
                || details.get("mode").and_then(Value::as_str) == Some("supervisor")
                || details.get("profile").and_then(Value::as_str) == Some("supervisor");
            if retired && FileProcessSupervisor::process_is_alive(&process)? {
                supervisor.request_termination(&process.id, "terminate")?;
                process_ids.push((process_root.clone(), process.id));
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    for (process_root, process_id) in process_ids {
        let supervisor = FileProcessSupervisor::new(process_root);
        loop {
            match supervisor.inspect(&process_id) {
                Ok(process) if FileProcessSupervisor::process_is_alive(&process)? => {
                    if Instant::now() >= deadline {
                        return Err(RefineError::Conflict(format!(
                            "retired Supervisor process {process_id} did not confirm exit; workflow automation remains stopped"
                        )));
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(_) | Err(RefineError::NotFound(_)) => break,
                Err(error) => return Err(error),
            }
        }
    }

    let refine_dir = prepare_refine_dir(target_root)?;
    FileChatService::with_runtime_root(&refine_dir, runtime_root).purge_supervisor_sessions()?;
    for name in ["supervisor-agent.json", "supervisor-agent.lock"] {
        let path = refine_dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to purge retired Supervisor state {}: {error}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

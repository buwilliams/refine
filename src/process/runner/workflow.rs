use super::*;

pub(super) fn run_workflow_worker(
    runtime_root: &Path,
    project_registry_root: Option<&Path>,
) -> RefineResult<()> {
    let mut recovered_root = None;
    let mut retired_supervisor_root = None;
    loop {
        if automation_is_paused(runtime_root)? {
            return Ok(());
        }
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
                match workflow.recover_interrupted_goals(
                    "workflow runner stopped before the Goal completed; restart the Goal when ready",
                ) {
                    Ok(count) if count > 0 => {
                        let _ = refresh_projection(runtime_root, &target_root);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("refine workflow recovery: {error}");
                    }
                }
                recovered_root = Some(root);
            }
            match workflow.evaluate_workflow() {
                Ok(_) => {
                    let _ = refresh_projection(runtime_root, &target_root);
                }
                Err(RefineError::Conflict(message)) if message.contains("paused") => {}
                Err(error) => {
                    eprintln!("refine workflow runner: {error}");
                }
            }
        }
        thread::sleep(WORKFLOW_INTERVAL);
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
    let capacity = crate::workflow::capacity::AgentCapacityService::new(runtime_root);
    let leases = capacity.snapshot()?;
    for lease in leases
        .leases
        .into_iter()
        .filter(|lease| lease.owner_id.starts_with("supervisor:"))
    {
        capacity.release(&lease.owner_id)?;
    }
    Ok(())
}

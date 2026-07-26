use super::*;

pub fn managed_pid_is_alive(pid: u32) -> RefineResult<bool> {
    pid_alive(pid)
}

pub(super) enum ProcessOutputEvent {
    Chunk {
        stream: ManagedProcessOutputStream,
        bytes: Vec<u8>,
    },
    Done,
    Error(RefineError),
}

pub(super) fn spawn_output_reader<R>(
    mut reader: R,
    stream: ManagedProcessOutputStream,
    tx: mpsc::Sender<ProcessOutputEvent>,
) -> std::thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = tx.send(ProcessOutputEvent::Done);
                    return;
                }
                Ok(read) => {
                    let _ = tx.send(ProcessOutputEvent::Chunk {
                        stream,
                        bytes: buffer[..read].to_vec(),
                    });
                }
                Err(error) => {
                    let _ = tx.send(ProcessOutputEvent::Error(RefineError::Io(format!(
                        "failed to read managed process {stream:?}: {error}"
                    ))));
                    return;
                }
            }
        }
    })
}

impl FileProcessSupervisor {
    pub(super) fn recover_running_process(
        &self,
        process: &mut ManagedProcess,
    ) -> RefineResult<bool> {
        match process.pid {
            Some(pid) if pid_alive(pid)? => {}
            Some(_) => {
                process.state = "exited".to_string();
                process.details = Some(append_detail(
                    process.details.take(),
                    "process was not alive during recovery",
                ));
                self.write_process(process)?;
                self.archive_terminal_process(process)?;
                return Ok(false);
            }
            None => {
                process.state = "interrupted".to_string();
                process.details = Some(append_detail(
                    process.details.take(),
                    "running process had no pid during recovery",
                ));
                self.write_process(process)?;
                self.archive_terminal_process(process)?;
                return Ok(false);
            }
        }
        Ok(true)
    }
}

pub(crate) fn acquire_workflow_process_registration_lock(
    runtime_root: &Path,
) -> RefineResult<WorkflowProcessRegistrationLock> {
    fs::create_dir_all(runtime_root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create workflow process registration root {}: {error}",
            runtime_root.display()
        ))
    })?;
    let path = runtime_root.join(WORKFLOW_PROCESS_REGISTRATION_LOCK_FILE);
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open workflow process registration lock {}: {error}",
                path.display()
            ))
        })?;
    file.lock_exclusive().map_err(|error| {
        RefineError::Io(format!(
            "failed to lock workflow process registration {}: {error}",
            path.display()
        ))
    })?;
    Ok(WorkflowProcessRegistrationLock { file })
}

pub(super) fn workflow_process_identity(
    metadata: &Map<String, Value>,
) -> Option<(&str, &str, &str)> {
    let claim_id = metadata
        .get("claim_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let execution_id = metadata
        .get("execution_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let goal_id = metadata
        .get("goal_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some((claim_id, execution_id, goal_id))
}

pub(super) fn workflow_runtime_root(process_root: &Path) -> PathBuf {
    if process_root.join(WORKFLOW_AUTOMATION_STATE_FILE).exists() {
        return process_root.to_path_buf();
    }
    process_root
        .parent()
        .filter(|parent| parent.join(WORKFLOW_AUTOMATION_STATE_FILE).exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| process_root.to_path_buf())
}

pub(super) fn validate_running_workflow_claim(
    runtime_root: &Path,
    claim_id: &str,
    execution_id: &str,
    goal_id: &str,
) -> RefineResult<()> {
    let path = runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE);
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Conflict(format!(
            "workflow process launch for Goal {goal_id} could not validate claim {claim_id} execution {execution_id} against {}: {error}; the process was not started",
            path.display()
        ))
    })?;
    let state: Value = serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse workflow process launch state {}: {error}",
            path.display()
        ))
    })?;
    let running = state
        .get("claims")
        .and_then(Value::as_array)
        .is_some_and(|claims| {
            claims.iter().any(|claim| {
                claim.get("claim_id").and_then(Value::as_str) == Some(claim_id)
                    && claim.get("execution_id").and_then(Value::as_str) == Some(execution_id)
                    && claim.get("goal_id").and_then(Value::as_str) == Some(goal_id)
                    && claim.get("state").and_then(Value::as_str) == Some("running")
            })
        });
    if running {
        Ok(())
    } else {
        Err(RefineError::Conflict(format!(
            "workflow process launch for Goal {goal_id} no longer owns running claim {claim_id} execution {execution_id}; the process was not started"
        )))
    }
}

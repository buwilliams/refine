use super::*;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const PTY_EOF_RETRY_INITIAL: Duration = Duration::from_millis(20);
const PTY_EOF_RETRY_MAX: Duration = Duration::from_millis(500);

/// Move a failed session's transcript out of the artifact set that supervisor
/// cleanup deletes. Renamed in place (same directory, `.failed` suffix) so the
/// evidence stays node-local under the runtime tree.
fn preserve_failed_transcript(stdout_path: &Path) -> Option<PathBuf> {
    let file_name = stdout_path.file_name()?.to_str()?;
    let preserved = stdout_path.with_file_name(format!("{file_name}.failed"));
    fs::rename(stdout_path, &preserved).ok()?;
    Some(preserved)
}

/// Copy PTY output into the transcript until the child is confirmed gone.
///
/// A zero-byte read is not trusted as EOF: on Linux the master reads EIO —
/// which the vendored PTY surfaces as `Ok(0)` — whenever no process
/// momentarily holds the slave side, and the agent spawns and reaps its own
/// subprocesses. Treating that transient state as EOF once froze the activity
/// clock and let the idle watchdog kill a live agent, so zero-byte reads are
/// retried with capped backoff until the poll loop confirms via `child_exited`
/// that the child was actually reaped.
pub(super) fn pump_pty_output(
    reader: &mut dyn Read,
    transcript: &mut fs::File,
    transcript_path: &Path,
    activity: &Mutex<Instant>,
    child_exited: &AtomicBool,
) -> RefineResult<()> {
    let mut buffer = [0_u8; 4096];
    let mut eof_retry = PTY_EOF_RETRY_INITIAL;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                if child_exited.load(Ordering::SeqCst) {
                    return Ok(());
                }
                thread::sleep(eof_retry);
                eof_retry = (eof_retry * 2).min(PTY_EOF_RETRY_MAX);
            }
            Ok(count) => {
                eof_retry = PTY_EOF_RETRY_INITIAL;
                *activity.lock().expect("Goal Agent activity clock poisoned") = Instant::now();
                transcript.write_all(&buffer[..count]).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to append Goal Agent transcript {}: {error}",
                        transcript_path.display()
                    ))
                })?;
                transcript.flush().map_err(|error| {
                    RefineError::Io(format!(
                        "failed to flush Goal Agent transcript {}: {error}",
                        transcript_path.display()
                    ))
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "Goal Agent output stream failed: {error}"
                )));
            }
        }
    }
}

/// The distinct failure for a reader thread that died while the child was
/// still running. The activity clock is frozen from that moment, so letting
/// the idle watchdog speak would blame a live agent for the harness's own
/// capture fault.
pub(super) fn transcript_capture_failure(
    join_result: std::thread::Result<RefineResult<()>>,
) -> RefineError {
    let cause = match join_result {
        Ok(Err(error)) => error.to_string(),
        Ok(Ok(())) => "transcript reader stopped without an error".to_string(),
        Err(_) => "transcript reader panicked".to_string(),
    };
    RefineError::Io(format!(
        "Goal Agent transcript capture failed while the agent was still running: {cause}"
    ))
}

/// The PTY is owned by the workflow runner, while its process record, transcript,
/// command queue, and signal file are ordinary runtime artifacts. That split lets
/// the daemon, browser, and CLI attach to the same Goal Agent without making a
/// browser connection part of workflow execution.
pub fn run_goal_agent<F>(launch: GoalAgentLaunch, on_attention: F) -> RefineResult<GoalAgentResult>
where
    F: FnMut(GoalAgentAttention),
{
    run_goal_agent_with_settlement(launch, on_attention, |_| Ok(()))
}

pub fn run_goal_agent_with_settlement<F, O>(
    launch: GoalAgentLaunch,
    on_attention: F,
    mut on_settlement: O,
) -> RefineResult<GoalAgentResult>
where
    F: FnMut(GoalAgentAttention),
    O: FnMut(&GoalAgentSettlement) -> RefineResult<()>,
{
    run_goal_agent_session(launch, on_attention, |_, _, settlement| {
        on_settlement(settlement)
    })
}

pub(super) fn run_goal_agent_session<F, O>(
    launch: GoalAgentLaunch,
    mut on_attention: F,
    mut on_process_settlement: O,
) -> RefineResult<GoalAgentResult>
where
    F: FnMut(GoalAgentAttention),
    O: FnMut(&FileProcessSupervisor, &ManagedProcess, &GoalAgentSettlement) -> RefineResult<()>,
{
    let cwd = launch.cwd.canonicalize().map_err(|error| {
        RefineError::InvalidInput(format!(
            "Goal Agent cwd {} is not available: {error}",
            launch.cwd.display()
        ))
    })?;
    let session_id = Uuid::new_v4().to_string();
    let process_id = format!("goal-agent-{session_id}");
    let supervisor = FileProcessSupervisor::new(&launch.runtime_root);
    fs::create_dir_all(supervisor.processes_dir()).map_err(|error| {
        RefineError::Io(format!(
            "failed to create Goal Agent process registry {}: {error}",
            supervisor.processes_dir().display()
        ))
    })?;
    let launch_lock_path = supervisor.processes_dir().join(".goal-agent-launch.lock");
    let launch_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&launch_lock_path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open Goal Agent launch lock {}: {error}",
                launch_lock_path.display()
            ))
        })?;
    launch_lock.lock_exclusive().map_err(|error| {
        RefineError::Io(format!(
            "failed to lock Goal Agent launch coordination {}: {error}",
            launch_lock_path.display()
        ))
    })?;
    if let Some(goal_id) = launch
        .metadata
        .get("goal_id")
        .and_then(Value::as_str)
        .filter(|goal_id| !goal_id.trim().is_empty())
    {
        match find_goal_agent_session(&launch.runtime_root, goal_id) {
            Ok(_) => {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} already has a running Goal Agent"
                )));
            }
            Err(RefineError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    let stdout_path = supervisor
        .processes_dir()
        .join(format!("{process_id}.stdout.log"));
    let command_path = supervisor
        .processes_dir()
        .join(format!("{process_id}.commands.jsonl"));
    let signal_path = supervisor
        .processes_dir()
        .join(format!("{process_id}.signal.json"));
    fs::File::create(&stdout_path).map_err(|error| {
        RefineError::Io(format!(
            "failed to create Goal Agent transcript {}: {error}",
            stdout_path.display()
        ))
    })?;
    if let Err(error) = fs::File::create(&command_path) {
        let _ = fs::remove_file(&stdout_path);
        return Err(RefineError::Io(format!(
            "failed to create Goal Agent command queue {}: {error}",
            command_path.display()
        )));
    }

    let provider_service = HostAgentProviderService::with_runtime_root(&launch.runtime_root);
    let implementation_phase = launch
        .metadata
        .get("implementation_phase")
        .and_then(Value::as_str);
    let requires_planning_result =
        matches!(implementation_phase, Some("plan" | "criticize" | "revise"));
    let protocol_prompt =
        goal_agent_protocol_prompt(&launch.prompt, &signal_path, implementation_phase);
    let launch_env_overrides = vec![
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("COLORTERM".to_string(), "truecolor".to_string()),
        ("REFINE_TERMINAL".to_string(), "1".to_string()),
        ("REFINE_SESSION_ROLE".to_string(), "goal".to_string()),
        ("REFINE_AGENT_SESSION_ID".to_string(), session_id.clone()),
        (
            "REFINE_AGENT_SIGNAL_PATH".to_string(),
            signal_path.display().to_string(),
        ),
    ];
    let command = match provider_service.interactive_command_with_session_and_environment(
        &launch.provider,
        &protocol_prompt,
        launch.provider_session.as_ref(),
        &launch_env_overrides,
    ) {
        Ok(command) => command,
        Err(error) => {
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(error);
        }
    };
    if let Err(error) = command.validate_prompt_artifact() {
        cleanup_session_artifacts(&command_path, &signal_path);
        let _ = fs::remove_file(&stdout_path);
        return Err(error);
    }
    let completion_timeout = launch.completion_timeout;
    let idle_timeout = launch.idle_timeout;
    let mut metadata = launch.metadata;
    metadata.insert("kind".to_string(), json!("interactive_session"));
    metadata.insert("profile".to_string(), json!("goal"));
    metadata.insert("role".to_string(), json!("goal"));
    metadata.insert("mode".to_string(), json!("goal"));
    metadata.insert("provider".to_string(), json!(&launch.provider));
    metadata.insert("session_id".to_string(), json!(&session_id));
    metadata.insert("cwd".to_string(), json!(cwd.display().to_string()));
    metadata.insert("attention_state".to_string(), json!("working"));
    metadata.insert(
        "prompt_transport".to_string(),
        serde_json::to_value(&command.prompt_transport).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode Goal Agent prompt transport metadata: {error}"
            ))
        })?,
    );
    metadata.insert(
        "command_path".to_string(),
        json!(command_path.display().to_string()),
    );
    metadata.insert(
        "signal_path".to_string(),
        json!(signal_path.display().to_string()),
    );

    let managed_spec = ManagedProcessSpec {
        owner: ProcessOwner::Agent,
        command: command.binary.clone(),
        args: command.args.clone(),
        cwd: Some(cwd.display().to_string()),
        env: launch_env_overrides,
        stdin: command.stdin.clone(),
        limits: Some(ProcessResourceLimits {
            kill_on_parent_exit: true,
            ..Default::default()
        }),
        authorization_command: Some(command.authorization_command.clone()),
        sensitive: false,
        metadata: metadata.clone(),
    };
    if let Err(error) = supervisor.validate_interactive_launch(&managed_spec) {
        cleanup_session_artifacts(&command_path, &signal_path);
        let _ = fs::remove_file(&stdout_path);
        return Err(error);
    }
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(pty_size(DEFAULT_COLS, DEFAULT_ROWS)) {
        Ok(pair) => pair,
        Err(error) => {
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(RefineError::Io(format!(
                "failed to open Goal Agent PTY: {error}"
            )));
        }
    };
    let mut pty_command = CommandBuilder::new(&command.binary);
    pty_command.args(&command.args);
    pty_command.cwd(&cwd);
    command.launch_environment.apply_to_pty(&mut pty_command);
    let mut child = match pair.slave.spawn_command(pty_command) {
        Ok(child) => child,
        Err(error) => {
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(RefineError::Io(format!(
                "failed to start interactive Goal Agent with {}: {error}",
                launch.provider
            )));
        }
    };
    let pid = child.process_id();
    let mut reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(RefineError::Io(format!(
                "failed to read Goal Agent output: {error}"
            )));
        }
    };
    let mut writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(RefineError::Io(format!(
                "failed to open Goal Agent input: {error}"
            )));
        }
    };
    drop(pair.slave);

    let details = match encode_metadata(&metadata) {
        Ok(details) => details,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(error);
        }
    };
    let artifact_handoff = match supervisor.begin_artifact_handoff(&process_id) {
        Ok(handoff) => handoff,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(error);
        }
    };
    let mut process = ManagedProcess {
        id: process_id.clone(),
        owner: ProcessOwner::Agent,
        pid,
        state: "running".to_string(),
        label: Some(format!(
            "Goal {} agent",
            metadata
                .get("goal_id")
                .and_then(Value::as_str)
                .unwrap_or("workflow")
        )),
        details: Some(details),
        stdout_path: Some(stdout_path.display().to_string()),
        stderr_path: None,
        stdin_path: Some(command_path.display().to_string()),
        limits: managed_spec.limits,
        started_at: Utc::now().to_rfc3339(),
        exit_code: None,
    };
    if let Err(error) = supervisor.register(process.clone()) {
        let _ = child.kill();
        let _ = child.wait();
        let _ = supervisor.finish_artifact_handoff(artifact_handoff);
        let _ = fs::remove_file(supervisor.artifact_handoff_path(&process_id));
        cleanup_session_artifacts(&command_path, &signal_path);
        let _ = fs::remove_file(&stdout_path);
        return Err(error);
    }
    let _ = FileExt::unlock(&launch_lock);
    drop(launch_lock);

    let reader_path = stdout_path.clone();
    // Written by the reader thread on every PTY chunk and read by the poll
    // loop's idle watchdog; the transcript is the liveness signal, so "no
    // chunk for the idle budget" means the agent has stalled.
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let reader_activity = Arc::clone(&last_activity);
    // Raised by the poll loop once `child.try_wait()` or `child.wait()` has
    // reaped the child; only then may the reader treat a zero-byte read as EOF
    // instead of a transient PTY EIO.
    let child_exited = Arc::new(AtomicBool::new(false));
    let reader_child_exited = Arc::clone(&child_exited);
    let mut reader_thread = Some(thread::spawn(move || -> RefineResult<()> {
        let mut output = OpenOptions::new()
            .append(true)
            .open(&reader_path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open Goal Agent transcript {}: {error}",
                    reader_path.display()
                ))
            })?;
        pump_pty_output(
            &mut reader,
            &mut output,
            &reader_path,
            &reader_activity,
            &reader_child_exited,
        )
    }));

    let mut command_offset = 0_u64;
    let mut completed_by_signal = false;
    let mut completion_report = None;
    let mut guidance_applied = None;
    let mut implementation_evidence = None;
    let mut planning_result = None;
    let completion_started_at = std::time::Instant::now();
    let mut signal_reader =
        SignalReader::default().requiring_planning_result(requires_planning_result);
    let mut invalid_signal_recovery = InvalidSignalRecovery::default();
    let status_result = (|| -> RefineResult<_> {
        loop {
            for command in read_commands_since(&command_path, &mut command_offset)? {
                match command {
                    AgentSessionCommand::Input { data } => {
                        *last_activity
                            .lock()
                            .expect("Goal Agent activity clock poisoned") =
                            std::time::Instant::now();
                        writer
                            .write_all(data.as_bytes())
                            .and_then(|_| writer.flush())
                            .map_err(|error| {
                                RefineError::Io(format!(
                                    "failed to send attached input to Goal Agent: {error}"
                                ))
                            })?;
                        if metadata.get("attention_state").and_then(Value::as_str)
                            == Some("needs_input")
                        {
                            metadata.insert("attention_state".to_string(), json!("working"));
                            metadata.remove("attention_message");
                            metadata.remove("attention_reason");
                            process.details = Some(encode_metadata(&metadata)?);
                            supervisor.register(process.clone())?;
                        }
                    }
                    AgentSessionCommand::Resize { cols, rows } => {
                        // A resize is a human at the attached terminal, which
                        // counts as activity just like typed input.
                        *last_activity
                            .lock()
                            .expect("Goal Agent activity clock poisoned") =
                            std::time::Instant::now();
                        pair.master.resize(pty_size(cols, rows)).map_err(|error| {
                            RefineError::Io(format!("failed to resize Goal Agent PTY: {error}"))
                        })?;
                    }
                }
            }

            let process_exit = child.try_wait().map_err(|error| {
                RefineError::Io(format!("failed to inspect Goal Agent process: {error}"))
            })?;
            if process_exit.is_some() {
                child_exited.store(true, Ordering::SeqCst);
            }
            let signal_read = if process_exit.is_some() {
                signal_reader.finish(&signal_path)?
            } else {
                signal_reader.take(&signal_path)?
            };
            match signal_read {
                SignalRead::Pending => {
                    if process_exit.is_some()
                        && let Some(error) =
                            invalid_signal_recovery.agent_exited_without_replacement(&signal_path)
                    {
                        return Err(error);
                    }
                }
                SignalRead::Rejected(diagnostic) => {
                    match invalid_signal_recovery.reject(
                        &signal_path,
                        &diagnostic,
                        process_exit.is_none(),
                    )? {
                        InvalidSignalDisposition::Retry(instruction) => {
                            writer
                                .write_all(instruction.as_bytes())
                                .and_then(|_| writer.flush())
                                .map_err(|error| {
                                    RefineError::Io(format!(
                                        "failed to send invalid-signal rewrite instruction to Goal Agent: {error}"
                                    ))
                                })?;
                        }
                        InvalidSignalDisposition::Fail(error) => return Err(error),
                    }
                }
                SignalRead::Valid(signal) => {
                    invalid_signal_recovery.accept_valid();
                    match signal.state {
                        AgentSessionState::Completed => {
                            completed_by_signal = true;
                            completion_report = (!signal.message.trim().is_empty())
                                .then(|| signal.message.trim().to_string());
                            guidance_applied = signal.guidance_applied;
                            implementation_evidence = signal.implementation_evidence;
                            planning_result = signal.planning_result;
                            metadata.insert("attention_state".to_string(), json!("completed"));
                            metadata.remove("attention_message");
                            process.details = Some(encode_metadata(&metadata)?);
                            supervisor.register(process.clone())?;
                            // The PTY child is a setsid session leader; kill
                            // its whole group first so descendants holding the
                            // slave die with it and the reader sees EOF.
                            if let Some(pid) = pid {
                                let _ = signal_os_process(pid, "kill", true);
                            }
                            let _ = child.kill();
                        }
                        AgentSessionState::NeedsInput => {
                            let message = if signal.message.trim().is_empty() {
                                "The Goal Agent needs user input before it can continue."
                                    .to_string()
                            } else {
                                signal.message.trim().to_string()
                            };
                            metadata.insert("attention_state".to_string(), json!("needs_input"));
                            metadata.insert("attention_message".to_string(), json!(&message));
                            metadata.insert("attention_reason".to_string(), json!("agent_signal"));
                            process.details = Some(encode_metadata(&metadata)?);
                            supervisor.register(process.clone())?;
                            on_attention(GoalAgentAttention { message });
                        }
                    }
                }
            }
            // Signal-file writes are the agent demonstrably working even
            // before the payload parses, so they reset the idle clock; a
            // provider streaming its completion signal must not be idle-killed
            // for quiet PTY output.
            if signal_reader.take_observed_change() {
                *last_activity
                    .lock()
                    .expect("Goal Agent activity clock poisoned") = std::time::Instant::now();
            }

            if let Some(status) = process_exit {
                break Ok(status);
            }
            // The child is still running here. If the reader thread is gone,
            // the activity clock is frozen and an idle verdict would blame a
            // live agent for the harness's capture failure — name that fault
            // instead of ever reaching the idle branch.
            if reader_thread
                .as_ref()
                .is_some_and(|handle| handle.is_finished())
            {
                let handle = reader_thread
                    .take()
                    .expect("reader thread handle observed finished");
                return Err(transcript_capture_failure(handle.join()));
            }
            if let Some(timeout) =
                completion_timeout.filter(|timeout| completion_started_at.elapsed() >= *timeout)
            {
                return Err(RefineError::Degraded(format!(
                    "Goal Agent did not produce a valid completion signal within {} seconds",
                    timeout.as_secs()
                )));
            }
            // The idle watchdog is suspended while the agent has signalled it
            // is waiting on a human: silence there is expected, and killing it
            // would punish exactly the session that behaved correctly.
            if metadata.get("attention_state").and_then(Value::as_str) != Some("needs_input")
                && let Some(idle) = idle_timeout.filter(|idle| {
                    last_activity
                        .lock()
                        .expect("Goal Agent activity clock poisoned")
                        .elapsed()
                        >= *idle
                })
            {
                return Err(RefineError::Degraded(format!(
                    "Goal Agent produced no output for {} seconds; failing fast instead of waiting out the completion cap",
                    idle.as_secs()
                )));
            }
            thread::sleep(COMMAND_POLL_INTERVAL);
        }
    })();
    let status = match status_result {
        Ok(status) => status,
        Err(error) => {
            // Kill the whole process group first: the PTY child is a setsid
            // session leader, and killing only the leader leaves descendants
            // holding the slave, so the reader would block on the terminal
            // until they finished on their own.
            if let Some(pid) = pid {
                let _ = signal_os_process(pid, "kill", true);
            }
            let _ = child.kill();
            let _ = child.wait();
            child_exited.store(true, Ordering::SeqCst);
            let capture_error = reader_thread.take().and_then(|handle| match handle.join() {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(_) => Some("transcript reader panicked".to_string()),
            });
            let error = match capture_error {
                Some(capture) => match error {
                    RefineError::Degraded(message) => RefineError::Degraded(format!(
                        "{message}; transcript capture error: {capture}"
                    )),
                    RefineError::Io(message) => {
                        RefineError::Io(format!("{message}; transcript capture error: {capture}"))
                    }
                    other => other,
                },
                None => error,
            };
            // Preserve the transcript before artifact cleanup deletes it: for
            // a timed-out or stalled agent it is the only evidence of what the
            // session actually did, and the failure record points here.
            let error = match preserve_failed_transcript(&stdout_path) {
                Some(preserved) => {
                    let note = format!("transcript preserved at {}", preserved.display());
                    match error {
                        RefineError::Degraded(message) => {
                            RefineError::Degraded(format!("{message}; {note}"))
                        }
                        RefineError::Io(message) => RefineError::Io(format!("{message}; {note}")),
                        other => other,
                    }
                }
                None => error,
            };
            process.state = "failed".to_string();
            let _ = supervisor.finish_artifact_handoff(artifact_handoff);
            let process_id = process.id.clone();
            let _ = supervisor.register(process);
            let _ = supervisor.cleanup(&process_id);
            cleanup_session_artifacts(&command_path, &signal_path);
            return Err(error);
        }
    };
    let reader_result = reader_thread
        .take()
        .expect("Goal Agent reader was joined before the session settled")
        .join()
        .map_err(|_| RefineError::Io("Goal Agent output reader panicked".to_string()))
        .and_then(|result| result);
    if let Err(error) = reader_result {
        process.state = "failed".to_string();
        let _ = supervisor.finish_artifact_handoff(artifact_handoff);
        let process_id = process.id.clone();
        let _ = supervisor.register(process);
        let _ = supervisor.cleanup(&process_id);
        cleanup_session_artifacts(&command_path, &signal_path);
        return Err(error);
    }
    let output = match fs::read(&stdout_path) {
        Ok(output) => String::from_utf8_lossy(&output).into_owned(),
        Err(error) => {
            process.state = "failed".to_string();
            let _ = supervisor.finish_artifact_handoff(artifact_handoff);
            let process_id = process.id.clone();
            let _ = supervisor.register(process);
            let _ = supervisor.cleanup(&process_id);
            cleanup_session_artifacts(&command_path, &signal_path);
            return Err(RefineError::Io(format!(
                "failed to read Goal Agent transcript {}: {error}",
                stdout_path.display()
            )));
        }
    };
    let result_output = completion_report
        .clone()
        .unwrap_or_else(|| strip_terminal_control(&output).trim().to_string());
    process.state = if status.success() || completed_by_signal {
        "exited".to_string()
    } else {
        "failed".to_string()
    };
    process.exit_code = i32::try_from(status.exit_code()).ok();
    let process_id = process.id.clone();
    supervisor.register(process.clone())?;
    let settlement = GoalAgentSettlement {
        output: result_output.clone(),
        process_id: process_id.clone(),
        session_id: session_id.clone(),
        state: process.state.clone(),
        exit_code: process.exit_code,
        guidance_applied: guidance_applied.clone(),
        implementation_evidence: implementation_evidence.clone(),
        planning_result: planning_result.clone(),
    };
    if let Err(error) = on_process_settlement(&supervisor, &process, &settlement) {
        cleanup_session_artifacts(&command_path, &signal_path);
        let _ = supervisor.finish_artifact_handoff(artifact_handoff);
        let _ = supervisor.cleanup(&process_id);
        return Err(error);
    }
    // Give attached SSE readers one final polling interval to consume the fully
    // flushed transcript after durable workflow evidence has consumed it.
    thread::sleep(Duration::from_millis(120));
    cleanup_session_artifacts(&command_path, &signal_path);
    supervisor.finish_artifact_handoff(artifact_handoff)?;
    supervisor.cleanup(&process_id)?;

    if !status.success() && !completed_by_signal {
        // Name the cause when the CLI could not authenticate. Otherwise a total
        // auth failure reads as an opaque non-zero exit, which is what made this
        // look like a capacity or liveness problem instead of a config one.
        let detail = crate::process::agent_env::auth_failure_hint(&output)
            .map(|hint| format!("; {hint}"))
            .unwrap_or_default();
        return Err(RefineError::Degraded(format!(
            "Goal Agent exited unsuccessfully: {}{detail}",
            status.exit_code()
        )));
    }
    Ok(GoalAgentResult {
        output: result_output,
        session_id,
        process_id,
        guidance_applied,
        implementation_evidence,
        planning_result,
    })
}

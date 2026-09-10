use super::*;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::output_capture::*;

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
    let super::pty_lifecycle::StartedSession {
        supervisor,
        session_id,
        stdout_path,
        command_path,
        signal_path,
        mut metadata,
        mut process,
        mut lifecycle,
        master,
        mut reader,
        mut writer,
        artifact_handoff,
        completion_timeout,
        idle_timeout,
        requires_planning_result,
    } = super::pty_lifecycle::launch_session(launch)?;

    let reader_path = stdout_path.clone();
    // Written by the reader thread on every PTY chunk and read by the poll
    // loop's idle watchdog; the transcript is the liveness signal, so "no
    // chunk for the idle budget" means the agent has stalled.
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let reader_activity = Arc::clone(&last_activity);
    // Raised after bounded scope settlement to request a bounded final drain.
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
    let mut toolbar_timeout_protected = false;
    let mut status_result = (|| -> RefineResult<_> {
        loop {
            #[cfg(all(test, target_os = "linux"))]
            super::ownership_tests::hook(&supervisor.runtime_root, "poll", &process)?;
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
                        master.resize(pty_size(cols, rows)).map_err(|error| {
                            RefineError::Io(format!("failed to resize Goal Agent PTY: {error}"))
                        })?;
                    }
                    AgentSessionCommand::ToolbarAttach { acknowledgment_id } => {
                        toolbar_timeout_protected = true;
                        metadata.insert(TOOLBAR_TIMEOUT_PROTECTED_KEY.to_string(), json!(true));
                        let acknowledgments = metadata
                            .entry(TOOLBAR_ATTACHMENT_ACKS_KEY.to_string())
                            .or_insert_with(|| json!([]))
                            .as_array_mut()
                            .ok_or_else(|| {
                                RefineError::Serialization(
                                    "Goal Agent Toolbar acknowledgment state is not an array"
                                        .to_string(),
                                )
                            })?;
                        if !acknowledgments
                            .iter()
                            .any(|value| value.as_str() == Some(&acknowledgment_id))
                        {
                            acknowledgments.push(json!(acknowledgment_id));
                            let overflow = acknowledgments
                                .len()
                                .saturating_sub(MAX_TOOLBAR_ATTACHMENT_ACKS);
                            if overflow > 0 {
                                acknowledgments.drain(..overflow);
                            }
                        }
                        // Persist the protected state and matching identity in
                        // one process-record update before either watchdog is
                        // evaluated below.
                        process.details = Some(encode_metadata(&metadata)?);
                        supervisor.register(process.clone())?;
                    }
                }
            }

            let process_exit = lifecycle.workload_status(&process)?;
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
                SignalRead::MalformedTransport(diagnostic) => {
                    match invalid_signal_recovery.reject_malformed_transport(
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
                SignalRead::InvalidContract(diagnostic) => {
                    return Err(invalid_signal_recovery
                        .reject_invalid_contract(&signal_path, &diagnostic)?);
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
                            metadata.insert("workload_result".into(), json!({"completed_by_signal":true,"output":completion_report,"guidance_applied":guidance_applied,"implementation_evidence":implementation_evidence,"planning_result":planning_result}));
                            process.details = Some(encode_metadata(&metadata)?);
                            supervisor.register(process.clone())?;
                            break Ok(process_exit
                                .unwrap_or_else(|| portable_pty::ExitStatus::with_exit_code(0)));
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
            if !toolbar_timeout_protected
                && let Some(timeout) =
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
            if !toolbar_timeout_protected
                && metadata.get("attention_state").and_then(Value::as_str) != Some("needs_input")
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
    // Workload result and whole-scope exit are independent. Always settle owned
    // descendants through the shared owner, including natural leader exit.
    if let Ok(status) = &status_result {
        process.exit_code = i32::try_from(status.exit_code()).ok();
    }
    let termination = lifecycle.settle(&process);
    if completed_by_signal && termination.is_ok() {
        status_result = lifecycle.workload_status(&process).and_then(|status| {
            status.ok_or_else(|| {
                RefineError::Degraded("completed PTY scope has no workload-status receipt".into())
            })
        });
    }
    if let Err(error) = &termination {
        metadata.insert("scope_settlement_error".into(), json!(error.to_string()));
        process.details = Some(encode_metadata(&metadata)?);
    }
    child_exited.store(true, Ordering::SeqCst);
    let capture = finish_capture(&mut reader_thread);
    let status_result = match (status_result, termination, capture) {
        (Err(error), stop, capture) => {
            Err(append_settlement_faults(error, stop.err(), capture.err()))
        }
        (Ok(status), Err(error), capture) => {
            if !status.success() && !completed_by_signal {
                Err(append_settlement_faults(
                    RefineError::Degraded(format!(
                        "Goal Agent exited unsuccessfully: {}",
                        status.exit_code()
                    )),
                    Some(error),
                    capture.err(),
                ))
            } else {
                Err(append_settlement_faults(error, None, capture.err()))
            }
        }
        (Ok(_), Ok(()), Err(error)) => Err(error),
        (Ok(status), Ok(()), Ok(())) => Ok(status),
    };
    let status = match status_result {
        Ok(status) => status,
        Err(error) => {
            // Preserve the transcript before artifact cleanup deletes it: for
            // a timed-out or stalled agent it is the only evidence of what the
            // session actually did, and the failure record points here.
            let error = match supervisor
                .group_pending(&process)
                .is_ok_and(|p| !p)
                .then(|| preserve_failed_transcript(&stdout_path))
                .flatten()
            {
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
            if let Err(registration_error) = supervisor.register(process.clone()) {
                return Err(append_settlement_faults(
                    error,
                    Some(registration_error),
                    None,
                ));
            }
            let _ = supervisor.cleanup(&process_id);
            return Err(error);
        }
    };
    let output = match fs::read(&stdout_path) {
        Ok(output) => String::from_utf8_lossy(&output).into_owned(),
        Err(error) => {
            process.state = "failed".to_string();
            let _ = supervisor.finish_artifact_handoff(artifact_handoff);
            let process_id = process.id.clone();
            let error = RefineError::Io(format!(
                "failed to read Goal Agent transcript {}: {error}",
                stdout_path.display()
            ));
            if let Err(registration_error) = supervisor.register(process.clone()) {
                return Err(append_settlement_faults(
                    error,
                    Some(registration_error),
                    None,
                ));
            }
            let _ = supervisor.cleanup(&process_id);
            return Err(error);
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
        let _ = supervisor.finish_artifact_handoff(artifact_handoff);
        let _ = supervisor.cleanup(&process_id);
        return Err(error);
    }
    // Give attached SSE readers one final polling interval to consume the fully
    // flushed transcript after durable workflow evidence has consumed it.
    thread::sleep(Duration::from_millis(120));
    supervisor.finish_artifact_handoff(artifact_handoff)?;
    supervisor.cleanup(&process_id)?;

    if !status.success() && !completed_by_signal {
        // Name the cause when the CLI could not authenticate. Otherwise a total
        // auth failure reads as an opaque non-zero exit, which is what made this
        // look like a capacity or liveness problem instead of a config one.
        let detail = crate::infrastructure::process::agent_env::auth_failure_hint(&output)
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

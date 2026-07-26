use super::*;

/// The PTY is owned by the workflow runner, while its process record, transcript,
/// command queue, and signal file are ordinary runtime artifacts. That split lets
/// the daemon, browser, and CLI attach to the same Goal Agent without making a
/// browser connection part of workflow execution.
pub fn run_goal_agent<F>(launch: GoalAgentLaunch, on_attention: F) -> RefineResult<GoalAgentResult>
where
    F: FnMut(GoalAgentAttention),
{
    run_goal_agent_session(launch, on_attention, |_, _| Ok(()))
}

pub(super) fn run_goal_agent_session<F, O>(
    launch: GoalAgentLaunch,
    mut on_attention: F,
    mut on_process_exit: O,
) -> RefineResult<GoalAgentResult>
where
    F: FnMut(GoalAgentAttention),
    O: FnMut(&FileProcessSupervisor, &ManagedProcess) -> RefineResult<()>,
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
    let protocol_prompt = goal_agent_protocol_prompt(&launch.prompt, &signal_path);
    let command = match provider_service.interactive_command(&launch.provider, &protocol_prompt) {
        Ok(command) => command,
        Err(error) => {
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(error);
        }
    };
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
        env: vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("REFINE_TERMINAL".to_string(), "1".to_string()),
            ("REFINE_SESSION_ROLE".to_string(), "goal".to_string()),
            ("REFINE_AGENT_SESSION_ID".to_string(), session_id.clone()),
            (
                "REFINE_AGENT_SIGNAL_PATH".to_string(),
                signal_path.display().to_string(),
            ),
        ],
        stdin: None,
        limits: Some(ProcessResourceLimits {
            kill_on_parent_exit: true,
            ..Default::default()
        }),
        authorization_command: Some(
            std::iter::once(command.binary.as_str())
                .chain(command.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        sensitive: false,
        metadata: metadata.clone(),
    };
    if let Err(error) = supervisor.validate_interactive_launch(&managed_spec) {
        cleanup_session_artifacts(&command_path, &signal_path);
        let _ = fs::remove_file(&stdout_path);
        return Err(error);
    }
    let workflow_registration_guard =
        match supervisor.workflow_process_registration_guard(&managed_spec) {
            Ok(guard) => guard,
            Err(error) => {
                cleanup_session_artifacts(&command_path, &signal_path);
                let _ = fs::remove_file(&stdout_path);
                return Err(error);
            }
        };

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
    // Same precedence as the managed-process path: the user's configured
    // environment first, then refine's own per-process variables.
    for (key, value) in crate::process::agent_env::agent_env_overlay(None) {
        pty_command.env(key, value);
    }
    for (key, value) in &managed_spec.env {
        pty_command.env(key, value);
    }
    for key in [
        "ANTHROPIC_API_KEY",
        "CLAUDE_API_KEY",
        "CODEX_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "GOOGLE_GENAI_API_KEY",
        "OPENAI_API_KEY",
    ] {
        pty_command.env_remove(key);
    }
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
    drop(workflow_registration_guard);
    let _ = FileExt::unlock(&launch_lock);
    drop(launch_lock);

    let reader_path = stdout_path.clone();
    let reader_thread = thread::spawn(move || -> RefineResult<()> {
        let mut output = OpenOptions::new()
            .append(true)
            .open(&reader_path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open Goal Agent transcript {}: {error}",
                    reader_path.display()
                ))
            })?;
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(count) => {
                    output.write_all(&buffer[..count]).map_err(|error| {
                        RefineError::Io(format!(
                            "failed to append Goal Agent transcript {}: {error}",
                            reader_path.display()
                        ))
                    })?;
                    output.flush().map_err(|error| {
                        RefineError::Io(format!(
                            "failed to flush Goal Agent transcript {}: {error}",
                            reader_path.display()
                        ))
                    })?;
                }
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "Goal Agent output stream failed: {error}"
                    )));
                }
            }
        }
    });

    let mut command_offset = 0_u64;
    let mut completed_by_signal = false;
    let mut completion_report = None;
    let mut guidance_applied = None;
    let status_result = (|| -> RefineResult<_> {
        loop {
            for command in read_commands_since(&command_path, &mut command_offset)? {
                match command {
                    AgentSessionCommand::Input { data } => {
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
                        pair.master.resize(pty_size(cols, rows)).map_err(|error| {
                            RefineError::Io(format!("failed to resize Goal Agent PTY: {error}"))
                        })?;
                    }
                }
            }

            if let Some(signal) = take_signal(&signal_path)? {
                match signal.state.trim() {
                    "completed" | "complete" => {
                        completed_by_signal = true;
                        completion_report = (!signal.message.trim().is_empty())
                            .then(|| signal.message.trim().to_string());
                        guidance_applied = signal.guidance_applied;
                        metadata.insert("attention_state".to_string(), json!("completed"));
                        metadata.remove("attention_message");
                        process.details = Some(encode_metadata(&metadata)?);
                        supervisor.register(process.clone())?;
                        let _ = child.kill();
                    }
                    "needs_input" | "waiting_for_user" => {
                        let message = if signal.message.trim().is_empty() {
                            "The Goal Agent needs user input before it can continue.".to_string()
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
                    other => {
                        return Err(RefineError::InvalidInput(format!(
                            "Goal Agent wrote unsupported session state {other}"
                        )));
                    }
                }
            }

            if let Some(status) = child.try_wait().map_err(|error| {
                RefineError::Io(format!("failed to inspect Goal Agent process: {error}"))
            })? {
                break Ok(status);
            }
            thread::sleep(COMMAND_POLL_INTERVAL);
        }
    })();
    let status = match status_result {
        Ok(status) => status,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader_thread.join();
            process.state = "failed".to_string();
            let _ = supervisor.finish_artifact_handoff(artifact_handoff);
            let process_id = process.id.clone();
            let _ = supervisor.register(process);
            let _ = supervisor.cleanup(&process_id);
            cleanup_session_artifacts(&command_path, &signal_path);
            return Err(error);
        }
    };
    if let Err(error) = on_process_exit(&supervisor, &process) {
        let _ = reader_thread.join();
        process.state = "failed".to_string();
        let _ = supervisor.finish_artifact_handoff(artifact_handoff);
        let process_id = process.id.clone();
        let _ = supervisor.register(process);
        let _ = supervisor.cleanup(&process_id);
        cleanup_session_artifacts(&command_path, &signal_path);
        return Err(error);
    }

    let reader_result = reader_thread
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
    // Give attached SSE readers one final polling interval to consume the fully
    // flushed transcript before process cleanup removes transient channels.
    thread::sleep(Duration::from_millis(120));
    process.state = if status.success() || completed_by_signal {
        "exited".to_string()
    } else {
        "failed".to_string()
    };
    process.exit_code = i32::try_from(status.exit_code()).ok();
    let process_id = process.id.clone();
    let _ = supervisor.register(process);
    cleanup_session_artifacts(&command_path, &signal_path);
    let _ = supervisor.finish_artifact_handoff(artifact_handoff);
    let _ = supervisor.cleanup(&process_id);

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
        output: completion_report
            .unwrap_or_else(|| strip_terminal_control(&output).trim().to_string()),
        session_id,
        process_id,
        guidance_applied,
    })
}

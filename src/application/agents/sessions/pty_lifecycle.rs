//! PTY workload and guardian lifetimes. The supervisor alone proves scope exit.
use super::*;
use portable_pty::{Child, ExitStatus};
use std::time::Instant;

const STOP_BUDGET: Duration = Duration::from_secs(2);

pub(super) struct PtyLifecycle {
    child: Option<Box<dyn Child + Send + Sync>>,
    supervisor: FileProcessSupervisor,
    process: Option<ManagedProcess>,
    released: bool,
    settled: bool,
    command: Option<crate::infrastructure::agents::invocation::PreparedProviderLaunch>,
    #[cfg(target_os = "linux")]
    scope:
        Option<crate::infrastructure::process::subprocess::owned_groups::launch_scope::ScopeLaunch>,
}
impl PtyLifecycle {
    pub(super) fn new(
        child: Box<dyn Child + Send + Sync>,
        supervisor: FileProcessSupervisor,
        #[cfg(target_os = "linux")] scope: Option<
            crate::infrastructure::process::subprocess::owned_groups::launch_scope::ScopeLaunch,
        >,
    ) -> Self {
        Self {
            child: Some(child),
            supervisor,
            process: None,
            released: false,
            settled: false,
            command: None,
            #[cfg(target_os = "linux")]
            scope,
        }
    }
    pub(super) fn attach(
        &mut self,
        metadata: &mut Map<String, Value>,
    ) -> RefineResult<Option<u32>> {
        let pid = self.child.as_ref().unwrap().process_id();
        #[cfg(target_os = "linux")]
        if let Some(scope) = self.scope.as_mut() {
            let mut details = encode_metadata(metadata)?;
            let workload = scope.attach_pid(
                pid.ok_or_else(|| RefineError::Degraded("PTY guardian has no PID".into()))?,
                &mut details,
            )?;
            *metadata = serde_json::from_str(&details)
                .map_err(|e| RefineError::Serialization(e.to_string()))?;
            return Ok(Some(workload));
        }
        Ok(pid)
    }
    pub(super) fn registered(&mut self, process: ManagedProcess) {
        self.process = Some(process);
    }
    pub(super) fn release(&mut self) -> RefineResult<()> {
        // A failed write may have delivered the start byte. From here, cleanup
        // requires ownership evidence rather than treating the launch as gated.
        self.released = true;
        #[cfg(target_os = "linux")]
        if let Some(scope) = self.scope.as_mut() {
            scope.release()?;
        }
        Ok(())
    }
    pub(super) fn workload_status(
        &mut self,
        process: &ManagedProcess,
    ) -> RefineResult<Option<ExitStatus>> {
        #[cfg(target_os = "linux")]
        if let Some(scope) = &self.scope {
            if let Some(status) = scope
                .scope()
                .workload_status(process, &self.supervisor.runtime_root)?
            {
                return Ok(Some(status.into()));
            }
            if self
                .child
                .as_mut()
                .unwrap()
                .try_wait()
                .map_err(|e| RefineError::Io(e.to_string()))?
                .is_some()
            {
                // The helper can finish between the receipt and status probes.
                return scope.scope().workload_status(process, &self.supervisor.runtime_root)?
                    .map(|s| Some(s.into())).ok_or_else(|| RefineError::Degraded(format!(
                        "PTY guardian exited without workload status for {}; retain ownership evidence and inspect process diagnostics", process.id)));
            }
            return Ok(None);
        }
        self.child
            .as_mut()
            .unwrap()
            .try_wait()
            .map_err(|e| RefineError::Io(e.to_string()))
    }
    pub(super) fn settle(&mut self, process: &ManagedProcess) -> RefineResult<()> {
        self.settled = true;
        self.supervisor
            .terminate_and_confirm_exit(process, STOP_BUDGET)
    }
    pub(super) fn abort(&mut self) {
        if self.released {
            return;
        }
        #[cfg(target_os = "linux")]
        if self.scope.take().is_some() {
            // Closing the handshake asks the guardian to kill and reap the
            // gated workload. Never kill the guardian while it owns that child.
            let deadline = Instant::now() + STOP_BUDGET;
            while Instant::now() < deadline {
                if self
                    .child
                    .as_mut()
                    .is_none_or(|child| child.try_wait().ok().flatten().is_some())
                {
                    return;
                }
                thread::sleep(Duration::from_millis(10));
            }
            return;
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }
}
impl Drop for PtyLifecycle {
    fn drop(&mut self) {
        if self.released {
            if !self.settled
                && let Some(process) = self.process.clone()
            {
                let _ = self.settle(&process);
            }
        } else {
            self.abort();
        }
        if let Some(child) = self.child.take() {
            self.supervisor.reap_pty_handle(child, self.command.take());
        }
    }
}

pub(super) struct StartedSession {
    pub supervisor: FileProcessSupervisor,
    pub session_id: String,
    pub stdout_path: PathBuf,
    pub command_path: PathBuf,
    pub signal_path: PathBuf,
    pub metadata: Map<String, Value>,
    pub process: ManagedProcess,
    pub lifecycle: PtyLifecycle,
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub artifact_handoff: fs::File,
    pub completion_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    pub requires_planning_result: bool,
}

pub(super) fn launch_session(launch: GoalAgentLaunch) -> RefineResult<StartedSession> {
    crate::infrastructure::git::worktrees::validate_workspace_launch(
        &launch.metadata,
        Some(&launch.cwd),
    )?;
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
    crate::infrastructure::process::supervisor::coordination::lock_exclusive_before(
        &launch_lock,
        &launch_lock_path,
        STOP_BUDGET,
    )
    .map_err(|error| {
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
    if let Ok(token) = std::env::var("REFINE_WORKFLOW_INCARNATION") {
        metadata.insert("workflow_incarnation".into(), json!(token));
    }
    metadata.insert(
        "agent_hard_cap_millis".into(),
        json!(completion_timeout.map(|d| d.as_millis() as u64)),
    );
    metadata.insert(
        "agent_idle_timeout_millis".into(),
        json!(idle_timeout.map(|d| d.as_millis() as u64)),
    );
    metadata.insert("kind".to_string(), json!("interactive_session"));
    metadata.insert("profile".to_string(), json!("goal"));
    metadata.insert("role".to_string(), json!("goal"));
    metadata.insert("mode".to_string(), json!("goal"));
    metadata.insert("provider".to_string(), json!(&launch.provider));
    metadata.insert("session_id".to_string(), json!(&session_id));
    metadata.insert("cwd".to_string(), json!(cwd.display().to_string()));
    metadata.insert("attention_state".to_string(), json!("working"));
    // Launch metadata cannot grant the Toolbar exemption. Only an attachment
    // command accepted by this live runtime may make the state one-way true.
    metadata.insert(TOOLBAR_TIMEOUT_PROTECTED_KEY.to_string(), json!(false));
    metadata.remove(TOOLBAR_ATTACHMENT_ACKS_KEY);
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
    #[cfg(target_os = "linux")]
    let mut scope_launch = crate::infrastructure::process::subprocess::owned_groups::launch_scope::ScopeLaunch::prepare_pty(&supervisor,&process_id,&managed_spec,&mut pty_command)?;
    crate::infrastructure::git::worktrees::validate_workspace_launch(&metadata, Some(&cwd))?;
    let child = match pair.slave.spawn_command(pty_command) {
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
    let mut lifecycle = PtyLifecycle::new(
        child,
        supervisor.clone(),
        #[cfg(target_os = "linux")]
        scope_launch.take(),
    );
    lifecycle.command = Some(command.clone());
    let pid = lifecycle.attach(&mut metadata)?;
    let reader = match super::output_capture::interruptible_reader(pair.master.as_ref()) {
        Ok(reader) => reader,
        Err(error) => {
            lifecycle.abort();
            return Err(RefineError::Io(format!(
                "failed to read Goal Agent output: {error}"
            )));
        }
    };
    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            lifecycle.abort();
            return Err(RefineError::Io(format!(
                "failed to open Goal Agent input: {error}"
            )));
        }
    };
    drop(pair.slave);

    let details = match encode_metadata(&metadata) {
        Ok(details) => details,
        Err(error) => {
            lifecycle.abort();
            return Err(error);
        }
    };
    let artifact_handoff = match supervisor.begin_artifact_handoff(&process_id) {
        Ok(handoff) => handoff,
        Err(error) => {
            lifecycle.abort();
            return Err(error);
        }
    };
    let process = ManagedProcess {
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
    #[cfg(all(test, target_os = "linux"))]
    super::ownership_tests::hook(&supervisor.runtime_root, "register", &process)?;
    if let Err(error) = supervisor.register(process.clone()) {
        lifecycle.abort();
        let _ = supervisor.finish_artifact_handoff(artifact_handoff);
        return Err(error);
    }
    lifecycle.registered(process.clone());
    #[cfg(all(test, target_os = "linux"))]
    super::ownership_tests::hook(&supervisor.runtime_root, "release", &process)?;
    if let Err(error) = lifecycle.release() {
        let stop = lifecycle.settle(&process).err();
        return Err(super::output_capture::append_settlement_faults(
            error, stop, None,
        ));
    }
    #[cfg(all(test, target_os = "linux"))]
    super::ownership_tests::hook(&supervisor.runtime_root, "released", &process)?;
    #[cfg(all(test, target_os = "linux"))]
    super::ownership_tests::hook(&supervisor.runtime_root, "capture", &process)?;
    let _ = FileExt::unlock(&launch_lock);
    drop(launch_lock);

    Ok(StartedSession {
        supervisor,
        session_id,
        stdout_path,
        command_path,
        signal_path,
        metadata,
        process,
        lifecycle,
        master: pair.master,
        reader,
        writer,
        artifact_handoff,
        completion_timeout,
        idle_timeout,
        requires_planning_result,
    })
}

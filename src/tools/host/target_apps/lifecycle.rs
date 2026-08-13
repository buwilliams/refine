use super::*;

impl FileTargetAppService {
    pub fn new(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
            target_root: target_root.into(),
        }
    }

    pub fn status(&self) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let mut snapshot = self.load_snapshot()?;
        let check = self.run_configured_checks(&settings)?;
        snapshot.ok = check.ok;
        snapshot.state = if check.ok {
            "running".to_string()
        } else if snapshot.state == "running" {
            "degraded".to_string()
        } else {
            "stopped".to_string()
        };
        snapshot.message = check.message.clone();
        snapshot.last_check_at = now_timestamp();
        snapshot.last_check_ok = check.ok;
        snapshot.last_check_message = check.message.clone();
        snapshot.last_health_at = snapshot.last_check_at.clone();
        snapshot.last_health_ok = check.ok;
        snapshot.last_health_message = check.message;
        snapshot.last_error = if check.ok {
            String::new()
        } else {
            snapshot.last_check_message.clone()
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn health(&self) -> RefineResult<TargetAppSnapshot> {
        self.status()
    }

    pub fn snapshot(&self) -> RefineResult<TargetAppSnapshot> {
        self.load_snapshot()
    }

    pub fn start(&self) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let instructions = setting(&settings, "target_app_start_instructions");
        let command = setting(&settings, "target_app_start_command");
        if !instructions.trim().is_empty() {
            let operation =
                self.run_agent_lifecycle("start", &instructions, &settings, Default::default());
            let ok = operation.exit_code == Some(0);
            let snapshot = TargetAppSnapshot {
                ok,
                state: if ok { "running" } else { "failed" }.to_string(),
                message: operation_message(&operation),
                last_check_at: String::new(),
                last_check_ok: ok,
                last_check_message: operation_message(&operation),
                last_health_at: String::new(),
                last_health_ok: ok,
                last_health_message: operation_message(&operation),
                last_error: if ok {
                    String::new()
                } else {
                    operation_message(&operation)
                },
                last_operation_id: operation.id.clone(),
                last_operation: Some(operation),
                process_id: None,
                pid: None,
            };
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        if command.trim().is_empty() {
            let mut snapshot = self.load_snapshot()?;
            snapshot.ok = true;
            snapshot.message = "No target-app start instructions are configured.".to_string();
            snapshot.state = "unknown".to_string();
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        let (shell, args) = shell_program_args(&command);
        let security =
            FileSecurityService::from_project_settings(&self.runtime_root, &self.refine_dir)?;
        let process = FileProcessSupervisor::with_allowed_commands(
            &self.runtime_root,
            security.allowed_commands.iter().cloned(),
        )
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::TargetApp,
            command: shell,
            args,
            cwd: Some(self.command_cwd(&settings).display().to_string()),
            env: command_env(&settings)?,
            stdin: None,
            limits: None,
            authorization_command: Some(command.clone()),
            sensitive: false,
            metadata: Default::default(),
        })?;
        let operation = TargetAppOperation {
            id: new_operation_id("target-start"),
            kind: "start".to_string(),
            state: "running".to_string(),
            started_at: now_timestamp(),
            finished_at: String::new(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        };
        let snapshot = TargetAppSnapshot {
            ok: true,
            state: "running".to_string(),
            message: "Target application started.".to_string(),
            last_check_at: String::new(),
            last_check_ok: true,
            last_check_message: String::new(),
            last_health_at: String::new(),
            last_health_ok: true,
            last_health_message: String::new(),
            last_error: String::new(),
            last_operation_id: operation.id.clone(),
            last_operation: Some(operation),
            process_id: Some(process.id),
            pid: process.pid,
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn stop(&self) -> RefineResult<TargetAppSnapshot> {
        self.mark_target_processes_stopped()?;
        let settings = self.settings()?;
        let instructions = setting(&settings, "target_app_stop_instructions");
        let command = setting(&settings, "target_app_stop_command");
        let operation = if !instructions.trim().is_empty() {
            self.run_agent_lifecycle("stop", &instructions, &settings, Default::default())
        } else if command.trim().is_empty() {
            TargetAppOperation {
                id: new_operation_id("target-stop"),
                kind: "stop".to_string(),
                state: "complete".to_string(),
                started_at: now_timestamp(),
                finished_at: now_timestamp(),
                exit_code: Some(0),
                stdout: String::new(),
                stderr: "No target-app stop command is configured.".to_string(),
            }
        } else {
            self.run_command("stop", &command, &settings, Default::default())?
        };
        let ok = operation.exit_code == Some(0);
        let snapshot = TargetAppSnapshot {
            ok,
            state: if ok { "stopped" } else { "failed" }.to_string(),
            message: operation_message(&operation),
            last_check_at: String::new(),
            last_check_ok: ok,
            last_check_message: operation_message(&operation),
            last_health_at: String::new(),
            last_health_ok: ok,
            last_health_message: operation_message(&operation),
            last_error: if ok {
                String::new()
            } else {
                operation_message(&operation)
            },
            last_operation_id: operation.id.clone(),
            last_operation: Some(operation),
            process_id: None,
            pid: None,
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn build(&self) -> RefineResult<TargetAppSnapshot> {
        self.build_with_metadata(Default::default())
    }

    pub fn build_with_metadata(
        &self,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let instructions = first_nonempty(&[
            setting(&settings, "target_app_build_instructions"),
            setting(&settings, "target_app_rebuild_instructions"),
        ]);
        let command = setting(&settings, "target_app_build_command");
        if !instructions.trim().is_empty() {
            let operation =
                self.run_agent_lifecycle("build", &instructions, &settings, process_metadata);
            let ok = operation.exit_code == Some(0);
            let snapshot = TargetAppSnapshot {
                ok,
                state: if ok { "stopped" } else { "failed" }.to_string(),
                message: operation_message(&operation),
                last_check_at: String::new(),
                last_check_ok: ok,
                last_check_message: operation_message(&operation),
                last_health_at: String::new(),
                last_health_ok: ok,
                last_health_message: operation_message(&operation),
                last_error: if ok {
                    String::new()
                } else {
                    operation_message(&operation)
                },
                last_operation_id: operation.id.clone(),
                last_operation: Some(operation),
                process_id: None,
                pid: None,
            };
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        if command.trim().is_empty() {
            let mut snapshot = self.load_snapshot()?;
            snapshot.ok = true;
            snapshot.state = "stopped".to_string();
            snapshot.message = "No target-app build instructions are configured.".to_string();
            snapshot.last_check_ok = true;
            snapshot.last_check_message = snapshot.message.clone();
            snapshot.last_health_ok = true;
            snapshot.last_health_message = snapshot.message.clone();
            snapshot.last_error = String::new();
            snapshot.last_operation_id = String::new();
            snapshot.last_operation = None;
            snapshot.process_id = None;
            snapshot.pid = None;
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        let operation = self.run_command("build", &command, &settings, process_metadata)?;
        let ok = operation.exit_code == Some(0);
        let snapshot = TargetAppSnapshot {
            ok,
            state: if ok { "stopped" } else { "failed" }.to_string(),
            message: operation_message(&operation),
            last_check_at: String::new(),
            last_check_ok: ok,
            last_check_message: operation_message(&operation),
            last_health_at: String::new(),
            last_health_ok: ok,
            last_health_message: operation_message(&operation),
            last_error: if ok {
                String::new()
            } else {
                operation_message(&operation)
            },
            last_operation_id: operation.id.clone(),
            last_operation: Some(operation),
            process_id: None,
            pid: None,
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn test(&self) -> RefineResult<TargetAppSnapshot> {
        self.test_with_metadata(Default::default())
    }

    pub fn test_with_metadata(
        &self,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let commands = target_app_test_commands(&settings);
        if commands.is_empty() {
            let mut snapshot = self.load_snapshot()?;
            snapshot.ok = false;
            snapshot.state = "failed".to_string();
            snapshot.message = "No enabled target-app test command is configured.".to_string();
            snapshot.last_error = snapshot.message.clone();
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }

        let mut last_operation = None;
        let mut messages = Vec::new();
        let mut ok = true;
        for command in commands {
            let operation =
                self.run_quality_command("test", &command, &settings, process_metadata.clone())?;
            let operation_ok = operation.exit_code == Some(0);
            let message = operation_message(&operation);
            messages.push(format!("{command}: {message}"));
            if !operation_ok {
                ok = false;
                last_operation = Some(operation);
                break;
            }
            last_operation = Some(operation);
        }
        let operation = last_operation.expect("non-empty commands must produce an operation");
        let message = messages.join("\n");
        let snapshot = TargetAppSnapshot {
            ok,
            state: if ok { "stopped" } else { "failed" }.to_string(),
            message: message.clone(),
            last_check_at: String::new(),
            last_check_ok: ok,
            last_check_message: message.clone(),
            last_health_at: String::new(),
            last_health_ok: ok,
            last_health_message: message.clone(),
            last_error: if ok { String::new() } else { message.clone() },
            last_operation_id: operation.id.clone(),
            last_operation: Some(operation),
            process_id: None,
            pid: None,
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub(super) fn mark_target_processes_stopped(&self) -> RefineResult<()> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        for process in supervisor
            .recover_owner(ProcessOwner::TargetApp)?
            .into_iter()
            .filter(|process| {
                process.owner == ProcessOwner::TargetApp && process.state != "stopped"
            })
        {
            let _ = supervisor.signal(&process.id, "kill");
        }
        Ok(())
    }
}

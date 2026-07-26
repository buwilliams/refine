use super::*;

impl FileTargetAppService {
    pub(super) fn run_command(
        &self,
        kind: &str,
        command: &str,
        settings: &JsonObject,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<TargetAppOperation> {
        self.run_owned_command(
            kind,
            command,
            settings,
            process_metadata,
            ProcessOwner::TargetApp,
            "target_app",
        )
    }

    pub(super) fn run_quality_command(
        &self,
        kind: &str,
        command: &str,
        settings: &JsonObject,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<TargetAppOperation> {
        self.run_owned_command(
            kind,
            command,
            settings,
            process_metadata,
            ProcessOwner::Quality,
            "quality",
        )
    }

    pub(super) fn run_agent_lifecycle(
        &self,
        kind: &str,
        instructions: &str,
        settings: &JsonObject,
        mut process_metadata: Map<String, Value>,
    ) -> TargetAppOperation {
        let started_at = now_timestamp();
        process_metadata.insert(
            "target_app_action".to_string(),
            Value::String(kind.to_string()),
        );
        process_metadata.insert(
            "target_root".to_string(),
            Value::String(self.target_root.display().to_string()),
        );
        let provider = setting(settings, "agent_cli")
            .trim()
            .to_string()
            .if_empty("claude");
        let cwd = self.command_cwd(settings);
        let prompt =
            target_app_lifecycle_prompt(kind, instructions, settings, &self.target_root, &cwd);
        let result = HostAgentProviderService::with_runtime_root(self.runtime_root.join("agents"))
            .invoke(ProviderInvocation {
                provider,
                prompt,
                session_id: None,
                cwd: Some(cwd.display().to_string()),
                process_metadata,
            });
        match result {
            Ok(output) => TargetAppOperation {
                id: new_operation_id(&format!("target-{kind}")),
                kind: kind.to_string(),
                state: "complete".to_string(),
                started_at,
                finished_at: now_timestamp(),
                exit_code: Some(0),
                stdout: output.trim().to_string(),
                stderr: String::new(),
            },
            Err(error) => TargetAppOperation {
                id: new_operation_id(&format!("target-{kind}")),
                kind: kind.to_string(),
                state: "failed".to_string(),
                started_at,
                finished_at: now_timestamp(),
                exit_code: Some(1),
                stdout: String::new(),
                stderr: error.to_string(),
            },
        }
    }

    pub(super) fn run_owned_command(
        &self,
        kind: &str,
        command: &str,
        settings: &JsonObject,
        process_metadata: Map<String, Value>,
        owner: ProcessOwner,
        authorization_category: &str,
    ) -> RefineResult<TargetAppOperation> {
        let started_at = now_timestamp();
        FileSecurityService::from_project_settings(&self.runtime_root, &self.refine_dir)?
            .authorize_host_command(authorization_category, command)?;
        let (shell, args) = shell_program_args(command);
        let output = FileProcessSupervisor::new(&self.runtime_root).run_to_completion(
            ManagedProcessSpec {
                owner,
                command: shell,
                args,
                cwd: Some(self.command_cwd(settings).display().to_string()),
                env: command_env(settings)?,
                stdin: None,
                limits: None,
                authorization_command: Some(command.to_string()),
                sensitive: false,
                metadata: process_metadata,
            },
        )?;
        Ok(TargetAppOperation {
            id: new_operation_id(&format!("target-{kind}")),
            kind: kind.to_string(),
            state: if output.success() {
                "complete".to_string()
            } else {
                "failed".to_string()
            },
            started_at,
            finished_at: now_timestamp(),
            exit_code: output.process.exit_code,
            stdout: output.stdout.trim().to_string(),
            stderr: output.stderr.trim().to_string(),
        })
    }

    pub(super) fn run_configured_checks(
        &self,
        settings: &JsonObject,
    ) -> RefineResult<TargetCheckResult> {
        let mut checks = Vec::new();
        let status_command = setting(settings, "target_app_status_command");
        if !status_command.trim().is_empty() {
            let operation =
                self.run_command("status", &status_command, settings, Default::default())?;
            checks.push(TargetCheckResult {
                ok: operation.exit_code == Some(0),
                message: operation_message(&operation),
            });
        }
        let process_command = setting(settings, "target_app_process_check_command");
        if !process_command.trim().is_empty() {
            let operation = self.run_command(
                "process-check",
                &process_command,
                settings,
                Default::default(),
            )?;
            checks.push(TargetCheckResult {
                ok: operation.exit_code == Some(0),
                message: operation_message(&operation),
            });
        }
        let tcp_host = setting(settings, "target_app_tcp_check_host");
        let tcp_port = setting(settings, "target_app_tcp_check_port");
        if !tcp_host.trim().is_empty() && !tcp_port.trim().is_empty() {
            let port = tcp_port.parse::<u16>().map_err(|_| {
                RefineError::InvalidInput("target_app_tcp_check_port must be a port".to_string())
            })?;
            let ok = tcp_reachable(&tcp_host, port);
            checks.push(TargetCheckResult {
                ok,
                message: if ok {
                    format!("TCP check {tcp_host}:{port} succeeded")
                } else {
                    format!("TCP check {tcp_host}:{port} failed")
                },
            });
        }
        if checks.is_empty() {
            return Ok(TargetCheckResult {
                ok: true,
                message: "No target-app status checks are configured.".to_string(),
            });
        }
        let failed: Vec<_> = checks.iter().filter(|check| !check.ok).collect();
        if failed.is_empty() {
            Ok(TargetCheckResult {
                ok: true,
                message: checks
                    .into_iter()
                    .map(|check| check.message)
                    .collect::<Vec<_>>()
                    .join("; "),
            })
        } else {
            Ok(TargetCheckResult {
                ok: false,
                message: failed
                    .into_iter()
                    .map(|check| check.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            })
        }
    }

    pub(super) fn command_cwd(&self, settings: &JsonObject) -> PathBuf {
        let cwd = setting(settings, "target_app_cwd");
        if cwd.trim().is_empty() {
            return self.target_root.clone();
        }
        let path = PathBuf::from(cwd);
        if path.is_absolute() {
            path
        } else {
            self.target_root.join(path)
        }
    }
}

use super::*;

impl FileProcessSupervisor {
    /// Apply the same pause and host-command authorization gates used by ordinary managed
    /// processes before an interactive PTY process is spawned by a surface adapter.
    pub fn validate_interactive_launch(&self, spec: &ManagedProcessSpec) -> RefineResult<()> {
        self.validate_launch(spec)
    }

    pub fn run_to_completion(
        &self,
        spec: ManagedProcessSpec,
    ) -> RefineResult<ManagedProcessOutput> {
        self.run_to_completion_with_output(spec, |_, _| {})
    }

    pub fn run_to_completion_with_output<F>(
        &self,
        spec: ManagedProcessSpec,
        on_output: F,
    ) -> RefineResult<ManagedProcessOutput>
    where
        F: FnMut(ManagedProcessOutputStream, &[u8]),
    {
        self.run_to_completion_with_output_and_environment(spec, None, on_output)
    }

    pub(crate) fn run_to_completion_with_prepared_environment<F>(
        &self,
        spec: ManagedProcessSpec,
        environment: &crate::infrastructure::process::launch_environment::EffectiveLaunchEnvironment,
        on_output: F,
    ) -> RefineResult<ManagedProcessOutput>
    where
        F: FnMut(ManagedProcessOutputStream, &[u8]),
    {
        self.run_to_completion_with_output_and_environment(spec, Some(environment), on_output)
    }

    fn run_to_completion_with_output_and_environment<F>(
        &self,
        spec: ManagedProcessSpec,
        environment: Option<
            &crate::infrastructure::process::launch_environment::EffectiveLaunchEnvironment,
        >,
        mut on_output: F,
    ) -> RefineResult<ManagedProcessOutput>
    where
        F: FnMut(ManagedProcessOutputStream, &[u8]),
    {
        self.validate_launch(&spec)?;
        let launch_guard = self.operation_launch_guard(&spec)?;
        fs::create_dir_all(self.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process registry {}: {error}",
                self.processes_dir().display()
            ))
        })?;
        let process_id = new_process_id();
        let stdout_path = self
            .processes_dir()
            .join(format!("{process_id}.stdout.log"));
        let stderr_path = self
            .processes_dir()
            .join(format!("{process_id}.stderr.log"));

        let mut command = match environment {
            Some(environment) => process_command_with_environment(&spec, environment)?,
            None => process_command(&spec)?,
        };
        #[cfg(target_os = "linux")]
        let mut scope_launch = owned_groups::launch_scope::ScopeLaunch::prepare(
            self,
            &process_id,
            &spec,
            &mut command,
        )?;
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if spec.stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }

        let child = command.spawn().map_err(|error| {
            RefineError::Io(format!(
                "failed to launch managed process {}: {error}",
                spec.command
            ))
        })?;
        let mut child = supervision::ReapedChild(Some(child));
        let mut details = process_details(&spec);
        #[cfg(target_os = "linux")]
        let workload_pid = match scope_launch.as_mut() {
            Some(scope) => match scope.attach(&child, &mut details) {
                Ok(pid) => pid,
                Err(error) => {
                    // Closing the launch gate lets the guardian reap its gated
                    // workload; killing the guardian would destroy that proof.
                    return Err(error);
                }
            },
            None => child.id(),
        };
        #[cfg(not(target_os = "linux"))]
        let workload_pid = child.id();
        let stdin_path = if let Some(stdin) = spec.stdin.as_deref() {
            let path = self.processes_dir().join(format!("{process_id}.stdin.txt"));
            if !spec.sensitive && !spec.metadata.contains_key("prompt_transport") {
                fs::write(&path, stdin).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to write process stdin {}: {error}",
                        path.display()
                    ))
                })?;
            }
            if spec.sensitive || spec.metadata.contains_key("prompt_transport") {
                None
            } else {
                Some(path.display().to_string())
            }
        } else {
            None
        };
        // Read before the limits move into the managed record.
        let stall_timeout = spec
            .limits
            .as_ref()
            .and_then(|limits| limits.stall_timeout_seconds)
            .map(Duration::from_secs);
        let mut process = ManagedProcess {
            id: process_id,
            owner: spec.owner.clone(),
            pid: Some(workload_pid),
            state: "running".to_string(),
            label: Some(spec.command.clone()),
            details: Some(details),
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: Some(stderr_path.display().to_string()),
            stdin_path,
            limits: spec.limits,
            started_at: now_millis_string(),
            exit_code: None,
        };
        if let Err(error) = self.write_process_transient(&process) {
            if child.id() == workload_pid {
                let _ = child.kill();
            }
            return Err(error);
        }
        if let Err(error) = self.create_process_identity_transient(&process) {
            if child.id() == workload_pid {
                let _ = child.kill();
            }
            let _ = self.remove_process_artifacts(&process);
            return Err(error);
        }
        #[cfg(target_os = "linux")]
        if let Some(scope) = scope_launch.as_mut() {
            if let Err(error) = scope.release() {
                let settlement = self.stop_capture_scope(&process);
                return Err(RefineError::Degraded(format!(
                    "{error}; launch settlement: {settlement:?}"
                )));
            }
        }
        if let Some(stdin) = spec.stdin.as_deref() {
            if let Some(mut child_stdin) = child.stdin.take() {
                if let Err(error) = child_stdin.write_all(stdin.as_bytes()) {
                    let settlement = self.stop_capture_scope(&process);
                    return Err(RefineError::Degraded(format!(
                        "failed to send managed process stdin: {error}; launch settlement: {settlement:?}"
                    )));
                }
            }
        }
        drop(launch_guard);

        let stdout = child.stdout.take().ok_or_else(|| {
            RefineError::Io(format!(
                "managed process {} did not expose stdout",
                process.id
            ))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            RefineError::Io(format!(
                "managed process {} did not expose stderr",
                process.id
            ))
        })?;
        #[cfg(test)]
        run_capture_hook(&self.runtime_root, "before_capture", &process);
        let mut stdout = capture::Capture::new(stdout, &stdout_path);
        let mut stderr = capture::Capture::new(stderr, &stderr_path);
        let mut status = None;
        #[cfg(target_os = "linux")]
        let launch_scope: Option<owned_groups::launch_scope::LaunchScope> = process
            .details
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| serde_json::from_value(v["launch_scope"].clone()).ok());
        let started = Instant::now();
        let hard_cap = spec
            .metadata
            .get("completion_timeout_seconds")
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs);
        let mut last_progress = started;
        let mut drain_deadline = None;
        let result = (|| -> RefineResult<()> {
            let stdout = stdout
                .as_mut()
                .map_err(|e| RefineError::Io(e.to_string()))?;
            let stderr = stderr
                .as_mut()
                .map_err(|e| RefineError::Io(e.to_string()))?;
            loop {
                if hard_cap.is_some_and(|limit| started.elapsed() >= limit) {
                    return Err(RefineError::Degraded(format!(
                        "managed process {} exceeded its completion timeout",
                        process.id
                    )));
                }
                if stall_timeout.is_some_and(|limit| last_progress.elapsed() >= limit) {
                    return Err(RefineError::Degraded(format!(
                        "{} produced no output for {}s and was stopped",
                        spec.command,
                        stall_timeout.unwrap().as_secs()
                    )));
                }
                if status.is_some() && stdout.eof && stderr.eof {
                    break;
                }
                if drain_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    break;
                }
                let out =
                    stdout.poll(|bytes| on_output(ManagedProcessOutputStream::Stdout, bytes))?;
                let err =
                    stderr.poll(|bytes| on_output(ManagedProcessOutputStream::Stderr, bytes))?;
                if out || err {
                    last_progress = Instant::now();
                }
                #[cfg(target_os = "linux")]
                if status.is_none()
                    && let Some(scope) = &launch_scope
                {
                    status = scope.workload_status(&process, &self.runtime_root)?;
                    if status.is_none()
                        && child
                            .try_wait()
                            .map_err(|e| RefineError::Io(e.to_string()))?
                            .is_some()
                    {
                        // The guardian is not the workload. Never substitute its status
                        // for a missing or invalid workload receipt.
                        status = scope.workload_status(&process, &self.runtime_root)?;
                        if status.is_none() {
                            return Err(RefineError::Degraded(
                                "ownership guardian exited without workload status".into(),
                            ));
                        }
                    }
                }
                #[cfg(target_os = "linux")]
                let direct = launch_scope.is_none();
                #[cfg(not(target_os = "linux"))]
                let direct = true;
                if direct && status.is_none() {
                    status = child
                        .try_wait()
                        .map_err(|e| RefineError::Io(e.to_string()))?;
                }
                if status.is_some() {
                    #[cfg(test)]
                    if drain_deadline.is_none() {
                        run_capture_hook(&self.runtime_root, "workload_exit", &process);
                    }
                    drain_deadline.get_or_insert_with(|| Instant::now() + capture::FINAL_DRAIN);
                }
                if !out && !err {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Ok(())
        })();
        let reason = result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_else(|| "workload final drain expired".into());
        let stream_evidence = |capture: Result<Value, &RefineError>| {
            capture.unwrap_or_else(|e| json!({"complete":false,"failure":e.to_string(),"reason":"capture setup failed"}))
        };
        let out_evidence = stream_evidence(stdout.as_ref().map(|s| s.evidence(&reason)));
        let err_evidence = stream_evidence(stderr.as_ref().map(|s| s.evidence(&reason)));
        let stdout_text = stdout.as_ref().map(|s| s.text()).unwrap_or_default();
        let stderr_text = stderr.as_ref().map(|s| s.text()).unwrap_or_default();
        // Close both read ends immediately, even on quiet or continuously ready pipes.
        drop(stdout);
        drop(stderr);
        let mut failure = result.err().map(|e| e.to_string());
        if failure.is_some() {
            let stop = self.stop_capture_scope(&process);
            if let Err(error) = stop {
                failure = Some(format!(
                    "{}; ownership settlement: {error}",
                    failure.unwrap()
                ));
            }
            // A termination receipt may now be available; preserve actual workload status.
            #[cfg(target_os = "linux")]
            if status.is_none()
                && let Some(scope) = &launch_scope
            {
                status = scope
                    .workload_status(&process, &self.runtime_root)
                    .ok()
                    .flatten();
            }
            if status.is_none() && child.id() == workload_pid {
                status = child.try_wait().ok().flatten();
            }
        }
        process.state = if status.is_some_and(|s| s.success()) {
            "exited"
        } else {
            "failed"
        }
        .into();
        process.exit_code = status.and_then(|s| s.code());
        let complete = out_evidence["complete"] == true && err_evidence["complete"] == true;
        let existing_details = process
            .details
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .filter(Value::is_object);
        let structured_details = existing_details.is_some();
        let mut details = existing_details.unwrap_or_else(|| json!({"command":process.details}));
        details["output_capture"] = json!({"complete":complete, "stdout":out_evidence, "stderr":err_evidence,
            "workload_status":status.map(|s| s.to_string()), "exit_code":process.exit_code,
            "stdout_path":process.stdout_path, "stderr_path":process.stderr_path, "failure":failure});
        // Legacy plain command/redaction details stay compatible for complete output.
        if structured_details || !complete || failure.is_some() {
            process.details = Some(details.to_string());
        }
        if !complete || failure.is_some() {
            // Retain a separate immutable result receipt even if primary registration
            // disappears. Scope records remain exclusively owned by ownership supervision.
            let evidence = self.runtime_root.join("output-captures");
            let mut receipt = serde_json::to_value(&process)
                .map_err(|e| RefineError::Serialization(e.to_string()))?;
            if failure.is_some() {
                // A failed artifact write must not also discard bytes already
                // captured in memory. Keep the bounded prefixes with the fault.
                receipt["captured_stdout"] = json!(stdout_text);
                receipt["captured_stderr"] = json!(stderr_text);
            }
            let saved = fs::create_dir_all(&evidence)
                .map_err(|e| RefineError::Io(e.to_string()))
                .and_then(|_| {
                    write_json_atomically(
                        &evidence.join(format!("{}.json", process.id)),
                        &serde_json::to_vec(&receipt)
                            .map_err(|e| RefineError::Serialization(e.to_string()))?,
                        "output capture",
                    )
                });
            if let Err(error) = saved {
                return Err(RefineError::Degraded(format!(
                    "{}; capture evidence write failed: {error}; {}",
                    failure.as_deref().unwrap_or("incomplete capture"),
                    process.details.as_deref().unwrap()
                )));
            }
        } else {
            self.remove_process_artifacts(&process)?;
        }
        if let Some(error) = failure {
            return Err(RefineError::Degraded(format!(
                "{error}; process {}; {}",
                process.id,
                process.details.as_deref().unwrap()
            )));
        }
        Ok(ManagedProcessOutput {
            process,
            stdout: stdout_text,
            stderr: stderr_text,
        })
    }

    fn stop_capture_scope(&self, process: &ManagedProcess) -> RefineResult<()> {
        if Self::requires_group_ownership(process) {
            let group = self
                .owned_groups()?
                .into_iter()
                .find(|g| g.process.id == process.id)
                .ok_or_else(|| {
                    RefineError::Degraded(
                        "ownership evidence unavailable; retained registration".into(),
                    )
                })?;
            Self::ensure_same_registration(process, &group.process)?;
            let outcome = self.stop_owned_group(&group, Duration::from_secs(2))?;
            if !outcome.confirmed_exit {
                return Err(RefineError::Degraded(
                    "scope exit remains unverified; capacity and artifacts retained".into(),
                ));
            }
        } else {
            // The workload may already have been reaped during final drain.
            // Reuse registration/start-identity fencing instead of signalling a
            // cached PID that could now belong to a replacement.
            self.terminate_owned_and_confirm_exit(process, "kill", Duration::from_secs(2))?;
        }
        Ok(())
    }

    pub(super) fn validate_launch(&self, spec: &ManagedProcessSpec) -> RefineResult<()> {
        if spec.command.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "managed process command is required".to_string(),
            ));
        }
        let authorization_command = spec
            .authorization_command
            .clone()
            .unwrap_or_else(|| process_command_line(spec));
        FileSecurityService::with_allowed_commands(
            &self.runtime_root,
            self.allowed_commands.iter().cloned(),
        )
        .authorize_host_command("process_supervisor", &authorization_command)
    }

    pub(super) fn operation_launch_guard(
        &self,
        spec: &ManagedProcessSpec,
    ) -> RefineResult<Option<OperationLaunchGuard>> {
        let ids = ["operation_id", "event_operation_id"]
            .iter()
            .filter_map(|key| spec.metadata.get(*key).and_then(Value::as_str))
            .filter(|id| !id.is_empty())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(None);
        }
        // Agent processes have a separate process registry within the same port runtime.
        // Their operation authority remains owned by that port's registry.
        let operation_root =
            if self.runtime_root.file_name().and_then(|s| s.to_str()) == Some("agents") {
                self.runtime_root.parent().unwrap_or(&self.runtime_root)
            } else {
                &self.runtime_root
            };
        FileOperationRegistry::new(operation_root)
            .active_launch_guards(&ids)
            .map(Some)
    }
}

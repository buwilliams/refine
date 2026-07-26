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
        mut on_output: F,
    ) -> RefineResult<ManagedProcessOutput>
    where
        F: FnMut(ManagedProcessOutputStream, &[u8]),
    {
        self.validate_launch(&spec)?;
        let launch_guard = self.operation_launch_guard(&spec)?;
        let workflow_registration_guard = self.workflow_process_registration_guard(&spec)?;
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

        let mut command = process_command(&spec);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if spec.stdin.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }

        let mut child = command.spawn().map_err(|error| {
            RefineError::Io(format!(
                "failed to launch managed process {}: {error}",
                spec.command
            ))
        })?;
        let stdin_path = if let Some(stdin) = spec.stdin.as_deref() {
            let path = self.processes_dir().join(format!("{process_id}.stdin.txt"));
            if !spec.sensitive {
                fs::write(&path, stdin).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to write process stdin {}: {error}",
                        path.display()
                    ))
                })?;
            }
            if let Some(mut child_stdin) = child.stdin.take() {
                child_stdin.write_all(stdin.as_bytes()).map_err(|error| {
                    RefineError::Io(format!("failed to send managed process stdin: {error}"))
                })?;
            }
            if spec.sensitive {
                None
            } else {
                Some(path.display().to_string())
            }
        } else {
            None
        };
        let mut process = ManagedProcess {
            id: process_id,
            owner: spec.owner.clone(),
            pid: Some(child.id()),
            state: "running".to_string(),
            label: Some(spec.command.clone()),
            details: Some(process_details(&spec)),
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: Some(stderr_path.display().to_string()),
            stdin_path,
            limits: spec.limits,
            started_at: now_millis_string(),
            exit_code: None,
        };
        if let Err(error) = self.write_process(&process) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        if let Err(error) = self.create_process_identity(&process) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = self.remove_process_artifacts(&process);
            return Err(error);
        }
        drop(workflow_registration_guard);
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
        let (tx, rx) = mpsc::channel();
        let stdout_thread =
            spawn_output_reader(stdout, ManagedProcessOutputStream::Stdout, tx.clone());
        let stderr_thread = spawn_output_reader(stderr, ManagedProcessOutputStream::Stderr, tx);
        let mut stdout_file = fs::File::create(&stdout_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process stdout log {}: {error}",
                stdout_path.display()
            ))
        })?;
        let mut stderr_file = fs::File::create(&stderr_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process stderr log {}: {error}",
                stderr_path.display()
            ))
        })?;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let mut reader_done = 0usize;
        let mut reader_error = None;
        let mut status = None;
        while reader_done < 2 || status.is_none() {
            match rx.recv_timeout(Duration::from_millis(25)) {
                Ok(ProcessOutputEvent::Chunk { stream, bytes }) => {
                    match stream {
                        ManagedProcessOutputStream::Stdout => {
                            stdout_file.write_all(&bytes).map_err(|error| {
                                RefineError::Io(format!(
                                    "failed to write process stdout log {}: {error}",
                                    stdout_path.display()
                                ))
                            })?;
                            stdout_bytes.extend_from_slice(&bytes);
                        }
                        ManagedProcessOutputStream::Stderr => {
                            stderr_file.write_all(&bytes).map_err(|error| {
                                RefineError::Io(format!(
                                    "failed to write process stderr log {}: {error}",
                                    stderr_path.display()
                                ))
                            })?;
                            stderr_bytes.extend_from_slice(&bytes);
                        }
                    }
                    on_output(stream, &bytes);
                }
                Ok(ProcessOutputEvent::Done) => reader_done += 1,
                Ok(ProcessOutputEvent::Error(error)) => {
                    reader_done += 1;
                    if reader_error.is_none() {
                        reader_error = Some(error);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if status.is_none() {
                status = child.try_wait().map_err(|error| {
                    RefineError::Io(format!(
                        "failed to inspect managed process {}: {error}",
                        process.id
                    ))
                })?;
            }
        }
        let status = match status {
            Some(status) => status,
            None => child.wait().map_err(|error| {
                RefineError::Io(format!(
                    "failed to wait for managed process {}: {error}",
                    process.id
                ))
            })?,
        };
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        stdout_file.flush().map_err(|error| {
            RefineError::Io(format!(
                "failed to flush process stdout log {}: {error}",
                stdout_path.display()
            ))
        })?;
        stderr_file.flush().map_err(|error| {
            RefineError::Io(format!(
                "failed to flush process stderr log {}: {error}",
                stderr_path.display()
            ))
        })?;
        process.state = if status.success() {
            "exited".to_string()
        } else {
            "failed".to_string()
        };
        process.exit_code = status.code();
        self.remove_process_artifacts(&process)?;
        if let Some(error) = reader_error {
            return Err(error);
        }
        Ok(ManagedProcessOutput {
            process,
            stdout: String::from_utf8_lossy(&stdout_bytes).to_string(),
            stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
        })
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
        let Some(operation_id) = spec
            .metadata
            .get("operation_id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        FileOperationRegistry::new(&self.runtime_root)
            .active_launch_guard(operation_id)
            .map(Some)
    }
}

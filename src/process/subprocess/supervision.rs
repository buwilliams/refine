use super::*;

impl FileProcessSupervisor {
    fn wait_for_reaper_terminal(&self, process_id: &str) -> RefineResult<Option<ManagedProcess>> {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.inspect_terminal(process_id) {
                Ok(terminal) => return Ok(Some(terminal)),
                Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
            let reaper_owned = self
                .reaper_owned
                .lock()
                .map_err(|_| {
                    RefineError::Io("managed process reaper lock was poisoned".to_string())
                })?
                .contains(process_id);
            if !reaper_owned {
                return Ok(None);
            }
            if Instant::now() >= deadline {
                return Err(RefineError::Conflict(format!(
                    "managed process reaper did not settle {process_id}"
                )));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl ProcessSupervisor for FileProcessSupervisor {
    fn launch(&self, spec: ManagedProcessSpec) -> RefineResult<ManagedProcess> {
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
        let stdout = fs::File::create(&stdout_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process stdout log {}: {error}",
                stdout_path.display()
            ))
        })?;
        let stderr = fs::File::create(&stderr_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process stderr log {}: {error}",
                stderr_path.display()
            ))
        })?;

        let mut command = process_command(&spec);
        command.stdout(Stdio::from(stdout));
        command.stderr(Stdio::from(stderr));
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

        let process = ManagedProcess {
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
        self.reaper_owned
            .lock()
            .map_err(|_| RefineError::Io("managed process reaper lock was poisoned".to_string()))?
            .insert(process.id.clone());
        let reaper = self.clone();
        let reaped_process = process.clone();
        std::thread::spawn(move || {
            let mut reaped_process = reaped_process;
            match child.wait() {
                Ok(status) => {
                    reaped_process.state = if status.success() {
                        "exited".to_string()
                    } else {
                        "failed".to_string()
                    };
                    reaped_process.exit_code = status.code();
                    let _ = reaper.archive_terminal_process(&reaped_process);
                }
                Err(error) => {
                    reaped_process.state = "failed".to_string();
                    reaped_process.details = Some(append_detail(
                        reaped_process.details.take(),
                        &format!("failed to reap managed process: {error}"),
                    ));
                    let _ = reaper.archive_terminal_process(&reaped_process);
                }
            }
            if let Ok(mut reaper_owned) = reaper.reaper_owned.lock() {
                reaper_owned.remove(&reaped_process.id);
            }
        });
        Ok(process)
    }

    fn signal(&self, process_id: &str, signal: &str) -> RefineResult<ManagedProcess> {
        let mut process = self.inspect(process_id)?;
        if matches!(signal, "stop" | "terminate" | "kill") {
            if let Some(pid) = process.pid
                && let Some(message) = signal_os_process(pid, signal, process_owns_group(&process))?
            {
                process.details = Some(match process.details {
                    Some(details) if !details.trim().is_empty() => {
                        format!("{details}; {message}")
                    }
                    _ => message,
                });
            }
            process.state = "stopped".to_string();
            self.write_process(&process)?;
            self.remove_process_artifacts(&process)?;
        } else {
            process.state = format!("signalled:{signal}");
            self.write_process(&process)?;
        }
        Ok(process)
    }

    fn wait(&self, process_id: &str) -> RefineResult<ManagedProcess> {
        let mut process = match self.inspect(process_id) {
            Ok(process) => process,
            Err(RefineError::NotFound(error)) => {
                return self
                    .wait_for_reaper_terminal(process_id)?
                    .ok_or_else(|| RefineError::NotFound(error));
            }
            Err(error) => return Err(error),
        };
        if process.state == "running"
            && let Some(pid) = process.pid
            && !pid_alive(pid)?
        {
            if let Some(terminal) = self.wait_for_reaper_terminal(process_id)? {
                return Ok(terminal);
            }
            process.state = "exited".to_string();
            self.write_process(&process)?;
            process = self.archive_terminal_process(&process)?;
        }
        Ok(process)
    }

    fn stream(&self, process_id: &str) -> RefineResult<String> {
        let process = match self.inspect(process_id) {
            Ok(process) => process,
            Err(RefineError::NotFound(_)) => self.inspect_terminal(process_id)?,
            Err(error) => return Err(error),
        };
        let mut output = String::new();
        if let Some(path) = process.stdout_path.as_deref() {
            append_stream_file(&mut output, "stdout", path)?;
        }
        if let Some(path) = process.stderr_path.as_deref() {
            append_stream_file(&mut output, "stderr", path)?;
        }
        if output.trim().is_empty() {
            Ok(format!("No captured output for process {process_id}."))
        } else {
            Ok(output)
        }
    }

    fn observe_output(
        &self,
        enumerated: &ManagedProcess,
    ) -> RefineResult<ProcessOutputObservation> {
        let current = match self.inspect(&enumerated.id) {
            Ok(current) => current,
            Err(RefineError::NotFound(_)) => match self.inspect_terminal(&enumerated.id) {
                Ok(terminal) => terminal,
                Err(RefineError::NotFound(_)) => {
                    return Ok(ProcessOutputObservation::Terminal {
                        process_id: enumerated.id.clone(),
                    });
                }
                Err(error) => return Err(error),
            },
            Err(error) => return Err(error),
        };
        if current.id != enumerated.id
            || current.owner != enumerated.owner
            || current.pid != enumerated.pid
            || current.started_at != enumerated.started_at
            || current.stdout_path != enumerated.stdout_path
            || current.stderr_path != enumerated.stderr_path
        {
            return Err(RefineError::Conflict(format!(
                "managed process {} identity changed after enumeration; output observation was skipped and both observations were retained",
                enumerated.id
            )));
        }

        let mut output = String::new();
        for (label, path) in [
            ("stdout", current.stdout_path.as_deref()),
            ("stderr", current.stderr_path.as_deref()),
        ] {
            let Some(path) = path else {
                continue;
            };
            if !append_observed_stream_file(&mut output, label, path)? {
                return Ok(ProcessOutputObservation::Terminal {
                    process_id: current.id,
                });
            }
        }
        if output.trim().is_empty()
            && (current.stdout_path.is_some() || current.stderr_path.is_some())
        {
            output = format!("No captured output for process {}.", current.id);
        }
        Ok(ProcessOutputObservation::Observed {
            process: current,
            output,
        })
    }

    fn inspect(&self, process_id: &str) -> RefineResult<ManagedProcess> {
        let path = self.processes_dir().join(format!("{process_id}.json"));
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                return RefineError::NotFound(format!("Process {process_id} was not found"));
            }
            RefineError::Io(format!(
                "failed to read process {}: {error}",
                path.display()
            ))
        })?;
        let process = serde_json::from_slice::<ManagedProcess>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse process {}: {error}",
                path.display()
            ))
        })?;
        if process.state != "running" {
            self.archive_terminal_process(&process)?;
            return Err(RefineError::NotFound(format!(
                "Process {process_id} was not found"
            )));
        }
        Ok(process)
    }

    fn cleanup(&self, process_id: &str) -> RefineResult<()> {
        if let Ok(process) = self.inspect(process_id) {
            let _ = self.signal(process_id, "terminate");
            self.remove_process_artifacts(&process)?;
        } else if let Ok(process) = self.inspect_terminal(process_id) {
            self.remove_process_artifacts(&process)?;
        }
        Ok(())
    }

    fn recover(&self) -> RefineResult<Vec<ManagedProcess>> {
        let mut recovered = Vec::new();
        for mut process in self.list()? {
            if process.state == "running" && !self.recover_running_process(&mut process)? {
                continue;
            }
            recovered.push(process);
        }
        Ok(recovered)
    }
}

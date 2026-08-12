use super::*;

impl FileProcessSupervisor {
    /// Requests termination without removing the managed-process record. The process runner owns
    /// final reaping and artifact cleanup, so callers can keep capacity reserved until the real
    /// child has exited.
    pub fn request_termination(
        &self,
        process_id: &str,
        signal: &str,
    ) -> RefineResult<ManagedProcess> {
        if !matches!(signal, "stop" | "terminate" | "kill") {
            return Err(RefineError::InvalidInput(format!(
                "unsupported termination signal {signal}"
            )));
        }
        let process = self.inspect(process_id)?;
        if let Some(pid) = process.pid {
            let _ = signal_os_process(pid, signal, process_owns_group(&process))?;
        }
        // The existing durable record remains authoritative while the runner reaps the child.
        // Rewriting it here can resurrect a stale running record if reaping removes the file
        // between signal delivery and this call.
        Ok(process)
    }

    pub fn process_is_alive(process: &ManagedProcess) -> RefineResult<bool> {
        process
            .pid
            .map(pid_alive)
            .transpose()
            .map(|alive| alive.unwrap_or(false))
    }

    /// Terminates a managed process and does not return until its recorded PID is confirmed dead.
    ///
    /// Recovery callers must retain the process record until exit is known: removing registration
    /// before that point would make an orphaned worker invisible and could allow overlapping work.
    pub fn terminate_and_confirm_exit(
        &self,
        process: &ManagedProcess,
        timeout: Duration,
    ) -> RefineResult<()> {
        if !Self::process_is_alive(process)? {
            return Ok(());
        }
        if let Err(error) = self.request_termination(&process.id, "terminate") {
            if !Self::process_is_alive(process)? {
                return Ok(());
            }
            return Err(error);
        }

        let deadline = Instant::now() + timeout;
        while Self::process_is_alive(process)? {
            if Instant::now() >= deadline {
                return Err(RefineError::Degraded(format!(
                    "managed process {} did not exit within {} ms after termination",
                    process.id,
                    timeout.as_millis()
                )));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Signals the exact process registration supplied by the caller and waits until that owned
    /// OS process exits. PID reuse, registry replacement, and delayed termination are causal
    /// failures: the process record and its identity evidence remain available for recovery.
    ///
    /// This method deliberately does not clean up registry or identity evidence. The product
    /// capability must durably record the confirmed-exit outcome before cleanup can erase either
    /// source of supervision evidence.
    pub fn terminate_owned_and_confirm_exit(
        &self,
        expected: &ManagedProcess,
        signal: &str,
        timeout: Duration,
    ) -> RefineResult<ConfirmedProcessExit> {
        if !matches!(signal, "stop" | "terminate" | "kill") {
            return Err(RefineError::InvalidInput(format!(
                "unsupported termination signal {signal}"
            )));
        }
        let started = Instant::now();
        let identity = self.load_process_identity(expected)?;
        self.ensure_expected_registration(expected, &identity)?;
        match self.owned_process_state(expected, &identity)? {
            OwnedProcessState::Exited => {
                return Ok(confirmed_process_exit(expected, signal, &identity, started));
            }
            OwnedProcessState::IdentityMismatch(actual) => {
                return Err(process_identity_mismatch(
                    expected,
                    &identity,
                    actual.as_deref(),
                ));
            }
            OwnedProcessState::Alive => {}
        }

        if let Some(pid) = expected.pid
            && let Some(message) = signal_os_process(pid, signal, process_owns_group(expected))?
        {
            match self.owned_process_state(expected, &identity)? {
                OwnedProcessState::Exited => {}
                OwnedProcessState::IdentityMismatch(actual) => {
                    return Err(process_identity_mismatch(
                        expected,
                        &identity,
                        actual.as_deref(),
                    ));
                }
                OwnedProcessState::Alive => {
                    return Err(RefineError::Degraded(format!(
                        "failed to signal managed process {}: {message}; its process record and identity evidence were retained for recovery",
                        expected.id
                    )));
                }
            }
        }

        let deadline = Instant::now() + timeout;
        loop {
            match self.owned_process_state(expected, &identity)? {
                OwnedProcessState::Exited => {
                    return Ok(confirmed_process_exit(expected, signal, &identity, started));
                }
                OwnedProcessState::IdentityMismatch(actual) => {
                    return Err(process_identity_mismatch(
                        expected,
                        &identity,
                        actual.as_deref(),
                    ));
                }
                OwnedProcessState::Alive if Instant::now() >= deadline => {
                    return Err(RefineError::Degraded(format!(
                        "managed process {} did not exit within {} ms after {signal}; its process record and identity evidence were retained for recovery",
                        expected.id,
                        timeout.as_millis()
                    )));
                }
                OwnedProcessState::Alive => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    #[cfg_attr(test, allow(dead_code))]
    // Cleanup failure deliberately carries the confirmed exit evidence inline
    // so callers cannot lose it while settling a process.
    #[allow(clippy::result_large_err)]
    pub(crate) fn cleanup_confirmed_exit(
        &self,
        expected: &ManagedProcess,
        outcome: ConfirmedProcessExit,
    ) -> Result<ConfirmedProcessExit, ConfirmedProcessCleanupFailure> {
        self.cleanup_confirmed_exit_with(expected, outcome, |_| Ok(()))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn cleanup_confirmed_exit_with<F>(
        &self,
        expected: &ManagedProcess,
        mut outcome: ConfirmedProcessExit,
        mut before_stage: F,
    ) -> Result<ConfirmedProcessExit, ConfirmedProcessCleanupFailure>
    where
        F: FnMut(ProcessCleanupStage) -> RefineResult<()>,
    {
        let handoff_path = self.artifact_handoff_path(&expected.id);
        let handoff = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&handoff_path)
        {
            Ok(file) => match file.try_lock_exclusive() {
                Ok(()) => Some(file),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => None,
                Err(error) => {
                    return Err(ConfirmedProcessCleanupFailure {
                        outcome,
                        error: RefineError::Io(format!(
                            "failed to inspect process artifact handoff {}: {error}",
                            handoff_path.display()
                        )),
                    });
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ConfirmedProcessCleanupFailure {
                    outcome,
                    error: RefineError::Io(format!(
                        "failed to open process artifact handoff {}: {error}",
                        handoff_path.display()
                    )),
                });
            }
        };
        let artifacts_deferred = handoff.is_none() && handoff_path.exists();
        if !artifacts_deferred {
            for path in [
                expected.stdout_path.as_deref(),
                expected.stderr_path.as_deref(),
                expected.stdin_path.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if let Err(error) = remove_file_if_present(Path::new(path), "process artifact") {
                    return Err(ConfirmedProcessCleanupFailure { outcome, error });
                }
            }
        }

        if let Err(error) = before_stage(ProcessCleanupStage::Registry).and_then(|()| {
            remove_file_if_present(
                &self.processes_dir().join(format!("{}.json", expected.id)),
                "process registry",
            )
        }) {
            return Err(ConfirmedProcessCleanupFailure { outcome, error });
        }
        outcome.registry_cleanup_completed = true;

        if let Err(error) = before_stage(ProcessCleanupStage::Identity).and_then(|()| {
            remove_file_if_present(
                &self.process_identity_path(&expected.id),
                "process identity",
            )
        }) {
            return Err(ConfirmedProcessCleanupFailure { outcome, error });
        }
        outcome.identity_cleanup_completed = true;
        drop(handoff);
        if !artifacts_deferred
            && let Err(error) = remove_file_if_present(&handoff_path, "process artifact handoff")
        {
            return Err(ConfirmedProcessCleanupFailure { outcome, error });
        }
        Ok(outcome)
    }
}

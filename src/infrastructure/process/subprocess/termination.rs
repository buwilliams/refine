use super::*;

impl FileProcessSupervisor {
    pub fn owned_process_is_alive(&self, process: &ManagedProcess) -> RefineResult<bool> {
        let identity = self.load_process_identity(process)?;
        self.ensure_expected_registration(process, &identity)?;
        match self.owned_process_state(process, &identity)? {
            OwnedProcessState::Alive => Ok(true),
            OwnedProcessState::Exited => Ok(false),
            OwnedProcessState::IdentityMismatch(actual) => Err(process_identity_mismatch(
                process,
                &identity,
                actual.as_deref(),
            )),
        }
    }

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
        if Self::requires_group_ownership(process) || self.group_path(&process.id).exists() {
            let group = self.owned_group_for_process(process)?;
            self.stop_owned_group(&group, timeout)?;
            return Ok(());
        }
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
        if Self::requires_group_ownership(expected) || self.group_path(&expected.id).exists() {
            let group = self.owned_group_for_process(expected)?;
            self.stop_owned_group(&group, timeout)?;
            return Ok(confirmed_process_exit(expected, signal, &identity, started));
        }
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
}

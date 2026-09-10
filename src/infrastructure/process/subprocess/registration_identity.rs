//! Registration-time PID identity persistence and validation.
use super::*;
impl FileProcessSupervisor {
    pub(super) fn write_process_identity(
        &self,
        identity: &ManagedProcessIdentity,
    ) -> RefineResult<()> {
        fs::create_dir_all(self.process_identities_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process identity registry {}: {error}",
                self.process_identities_dir().display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(identity).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process identity: {error}"))
        })?;
        write_json_atomically(
            &self.process_identity_path(&identity.process_id),
            &encoded,
            "process identity",
        )
    }

    pub(super) fn create_process_identity(
        &self,
        process: &ManagedProcess,
    ) -> RefineResult<ManagedProcessIdentity> {
        self.register_owned_group(process)?;
        let identity = self.build_process_identity(process)?;
        self.write_process_identity(&identity)?;
        Ok(identity)
    }

    /// Identity for a run-to-completion process; see [`Self::write_process_transient`].
    pub(super) fn create_process_identity_transient(
        &self,
        process: &ManagedProcess,
    ) -> RefineResult<ManagedProcessIdentity> {
        self.register_owned_group(process)?;
        let identity = self.build_process_identity(process)?;
        fs::create_dir_all(self.process_identities_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process identity registry {}: {error}",
                self.process_identities_dir().display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(&identity).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process identity: {error}"))
        })?;
        write_json_atomically_transient(
            &self.process_identity_path(&identity.process_id),
            &encoded,
            "process identity",
        )?;
        Ok(identity)
    }

    fn build_process_identity(
        &self,
        process: &ManagedProcess,
    ) -> RefineResult<ManagedProcessIdentity> {
        Ok(ManagedProcessIdentity {
            process_id: process.id.clone(),
            owner: process.owner.clone(),
            pid: process.pid,
            os_identity: process.pid.map(os_process_identity).transpose()?.flatten(),
            registered_at: now_millis_string(),
        })
    }

    pub(super) fn load_process_identity(
        &self,
        process: &ManagedProcess,
    ) -> RefineResult<ManagedProcessIdentity> {
        let path = self.process_identity_path(&process.id);
        match fs::read(&path) {
            Ok(bytes) => {
                let identity =
                    serde_json::from_slice::<ManagedProcessIdentity>(&bytes).map_err(|error| {
                        RefineError::Serialization(format!(
                            "failed to parse process identity {}: {error}",
                            path.display()
                        ))
                    })?;
                Ok(identity)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Err(RefineError::Conflict(format!(
                    "managed process {} has no registration-time PID identity evidence; termination was not requested because the recorded PID may have been reused, and its legacy process record was retained for recovery",
                    process.id
                )))
            }
            Err(error) => Err(RefineError::Io(format!(
                "failed to read process identity {}: {error}",
                path.display()
            ))),
        }
    }

    pub(super) fn ensure_expected_registration(
        &self,
        expected: &ManagedProcess,
        identity: &ManagedProcessIdentity,
    ) -> RefineResult<()> {
        self.ensure_identity_matches_process(expected, identity)?;
        match self.inspect(&expected.id) {
            Ok(current)
                if current.id == expected.id
                    && current.owner == expected.owner
                    && current.pid == expected.pid
                    && current.started_at == expected.started_at =>
            {
                Ok(())
            }
            Ok(_) => Err(RefineError::Conflict(format!(
                "managed process {} registry identity changed before termination; termination was not requested and current evidence was retained for recovery",
                expected.id
            ))),
            Err(RefineError::NotFound(_)) => match self.owned_process_state(expected, identity)? {
                OwnedProcessState::Exited => Ok(()),
                OwnedProcessState::Alive | OwnedProcessState::IdentityMismatch(_) => {
                    Err(RefineError::Conflict(format!(
                        "managed process {} is alive without its expected registry record; termination was not requested and identity evidence was retained for recovery",
                        expected.id
                    )))
                }
            },
            Err(error) => Err(error),
        }
    }

    pub(super) fn ensure_identity_matches_process(
        &self,
        process: &ManagedProcess,
        identity: &ManagedProcessIdentity,
    ) -> RefineResult<()> {
        if identity.process_id != process.id
            || identity.owner != process.owner
            || identity.pid != process.pid
        {
            return Err(RefineError::Conflict(format!(
                "managed process {} identity evidence does not match its registry record; termination was not requested and both records were retained for recovery",
                process.id
            )));
        }
        Ok(())
    }

    pub(super) fn owned_process_state(
        &self,
        process: &ManagedProcess,
        identity: &ManagedProcessIdentity,
    ) -> RefineResult<OwnedProcessState> {
        let Some(pid) = process.pid else {
            return Ok(OwnedProcessState::Exited);
        };
        if !pid_alive(pid)? {
            return Ok(OwnedProcessState::Exited);
        }
        let actual = os_process_identity(pid)?;
        // The process can exit between the liveness probe and identity read,
        // especially immediately after a termination signal. Confirm that
        // exact transition before treating an unavailable observation as an
        // identity mismatch. If the PID is still alive, fail closed below.
        if actual.is_none() && !pid_alive(pid)? {
            return Ok(OwnedProcessState::Exited);
        }
        match (&identity.os_identity, &actual) {
            (Some(expected), Some(actual)) if expected != actual => {
                Ok(OwnedProcessState::IdentityMismatch(Some(actual.clone())))
            }
            (Some(_), None) => Ok(OwnedProcessState::IdentityMismatch(None)),
            _ => Ok(OwnedProcessState::Alive),
        }
    }
}

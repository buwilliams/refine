use super::*;

impl FileProcessSupervisor {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            allowed_commands: BTreeSet::new(),
            reaper_owned: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    pub fn with_allowed_commands(
        runtime_root: impl Into<PathBuf>,
        allowed_commands: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            allowed_commands: allowed_commands
                .into_iter()
                .map(|command| command.into())
                .collect(),
            reaper_owned: Arc::new(Mutex::new(BTreeSet::new())),
        }
    }

    pub fn processes_dir(&self) -> PathBuf {
        self.runtime_root.join("processes")
    }

    pub fn process_history_dir(&self) -> PathBuf {
        self.runtime_root.join(PROCESS_HISTORY_DIR)
    }

    pub(super) fn process_history_path(&self, process_id: &str) -> PathBuf {
        self.process_history_dir()
            .join(format!("{process_id}.json"))
    }

    /// Hold transient process artifacts while a workflow-owned consumer finishes reading them.
    ///
    /// The filesystem lock is released automatically if the consumer exits, so later recovery can
    /// still remove abandoned artifacts.
    pub(crate) fn begin_artifact_handoff(&self, process_id: &str) -> RefineResult<fs::File> {
        fs::create_dir_all(self.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process registry {}: {error}",
                self.processes_dir().display()
            ))
        })?;
        let path = self.artifact_handoff_path(process_id);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open process artifact handoff {}: {error}",
                    path.display()
                ))
            })?;
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to lock process artifact handoff {}: {error}",
                path.display()
            ))
        })?;
        Ok(file)
    }

    pub(crate) fn finish_artifact_handoff(&self, handoff: fs::File) -> RefineResult<()> {
        FileExt::unlock(&handoff).map_err(|error| {
            RefineError::Io(format!(
                "failed to unlock process artifact handoff: {error}"
            ))
        })?;
        drop(handoff);
        Ok(())
    }

    pub(crate) fn artifact_handoff_path(&self, process_id: &str) -> PathBuf {
        self.processes_dir()
            .join(format!("{process_id}.artifact-handoff.lock"))
    }

    pub(super) fn process_identities_dir(&self) -> PathBuf {
        self.runtime_root.join(PROCESS_IDENTITIES_DIR)
    }

    pub(super) fn process_identity_path(&self, process_id: &str) -> PathBuf {
        self.process_identities_dir()
            .join(format!("{process_id}.json"))
    }

    pub fn pause_state_path(&self) -> PathBuf {
        self.runtime_root.join("process-control.json")
    }

    pub fn list(&self) -> RefineResult<Vec<ManagedProcess>> {
        let dir = self.processes_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut processes = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to read process registry {}: {error}",
                dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!("failed to inspect process registry entry: {error}"))
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                if is_stale_process_temp(&path) {
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(RefineError::Io(format!(
                                "failed to remove stale process temp {}: {error}",
                                path.display()
                            )));
                        }
                    }
                }
                continue;
            }
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to read process {}: {error}",
                        path.display()
                    )));
                }
            };
            match serde_json::from_slice::<ManagedProcess>(&bytes) {
                Ok(process) if process.state == "running" => processes.push(process),
                Ok(process) => {
                    // Terminal records do not belong in active enumeration, but
                    // their exit status and output remain available for explicit
                    // inspection and cleanup.
                    let _ = self.archive_terminal_process(&process);
                }
                Err(_) if bytes.is_empty() => continue,
                Err(_) => continue,
            }
        }
        processes.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(processes)
    }

    pub fn recover_owner(&self, owner: ProcessOwner) -> RefineResult<Vec<ManagedProcess>> {
        let mut recovered = Vec::new();
        for mut process in self.list()? {
            if process.owner == owner
                && process.state == "running"
                && !self.recover_running_process(&mut process)?
            {
                continue;
            }
            recovered.push(process);
        }
        Ok(recovered)
    }

    pub fn pause_state(&self) -> RefineResult<ProcessPauseState> {
        let path = self.pause_state_path();
        if !path.exists() {
            return Ok(ProcessPauseState::default());
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read process control {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse process control {}: {error}",
                path.display()
            ))
        })
    }

    pub fn set_workflow_paused(&self, paused: bool) -> RefineResult<ProcessPauseState> {
        let mut state = self.pause_state()?;
        state.workflow_paused = paused;
        self.write_pause_state(&state)?;
        Ok(state)
    }

    pub fn set_background_worker_enabled(
        &self,
        worker_kind: &str,
        enabled: bool,
    ) -> RefineResult<ProcessPauseState> {
        let mut state = self.pause_state()?;
        if enabled {
            state.disabled_background_workers.remove(worker_kind);
        } else {
            state
                .disabled_background_workers
                .insert(worker_kind.to_string());
        }
        self.write_pause_state(&state)?;
        Ok(state)
    }

    pub(super) fn write_pause_state(&self, state: &ProcessPauseState) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process control: {error}"))
        })?;
        let path = self.pause_state_path();
        write_json_atomically(&path, &encoded, "process control")
    }

    pub(super) fn write_process(&self, process: &ManagedProcess) -> RefineResult<()> {
        fs::create_dir_all(self.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process registry {}: {error}",
                self.processes_dir().display()
            ))
        })?;
        let path = self.processes_dir().join(format!("{}.json", process.id));
        let encoded = serde_json::to_vec_pretty(process).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process: {error}"))
        })?;
        write_json_atomically(&path, &encoded, "process")
    }

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
        let identity = ManagedProcessIdentity {
            process_id: process.id.clone(),
            owner: process.owner.clone(),
            pid: process.pid,
            os_identity: process.pid.map(os_process_identity).transpose()?.flatten(),
            registered_at: now_millis_string(),
        };
        self.write_process_identity(&identity)?;
        Ok(identity)
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

    pub(super) fn remove_process_artifacts(&self, process: &ManagedProcess) -> RefineResult<()> {
        let handoff_path = self.artifact_handoff_path(&process.id);
        // A live workflow consumer owns the transcript through this lease. Reconciliation may
        // already persist a truthful terminal state, but deletion waits until consumption ends.
        let handoff = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&handoff_path)
        {
            Ok(file) => match file.try_lock_exclusive() {
                Ok(()) => Some(file),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to lock process artifact handoff {} for cleanup: {error}",
                        handoff_path.display()
                    )));
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to open process artifact handoff {} for cleanup: {error}",
                    handoff_path.display()
                )));
            }
        };
        for path in [
            process.stdout_path.as_deref(),
            process.stderr_path.as_deref(),
            process.stdin_path.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to remove process artifact {path}: {error}"
                    )));
                }
            }
        }
        if let Some(handoff) = handoff {
            drop(handoff);
            match fs::remove_file(&handoff_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to remove process artifact handoff {}: {error}",
                        handoff_path.display()
                    )));
                }
            }
        }
        let path = self.processes_dir().join(format!("{}.json", process.id));
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to remove process {}: {error}",
                    path.display()
                )));
            }
        }
        let history_path = self.process_history_path(&process.id);
        match fs::remove_file(&history_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to remove process history {}: {error}",
                    history_path.display()
                )));
            }
        }
        let identity_path = self.process_identity_path(&process.id);
        match fs::remove_file(&identity_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to remove process identity {}: {error}",
                    identity_path.display()
                )));
            }
        }
        Ok(())
    }

    pub(super) fn inspect_terminal(&self, process_id: &str) -> RefineResult<ManagedProcess> {
        let path = self.process_history_path(process_id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                return RefineError::NotFound(format!("Process {process_id} was not found"));
            }
            RefineError::Io(format!(
                "failed to read process history {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<ManagedProcess>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse process history {}: {error}",
                path.display()
            ))
        })
    }

    pub(super) fn archive_terminal_process(
        &self,
        process: &ManagedProcess,
    ) -> RefineResult<ManagedProcess> {
        if process.state == "running" {
            return Err(RefineError::Conflict(format!(
                "running process {} cannot be archived as terminal",
                process.id
            )));
        }
        fs::create_dir_all(self.process_history_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process history {}: {error}",
                self.process_history_dir().display()
            ))
        })?;
        let mut archived = process.clone();
        if let Some(stdin_path) = archived.stdin_path.take() {
            remove_file_if_present(Path::new(&stdin_path), "process stdin")?;
        }
        let encoded = serde_json::to_vec_pretty(&archived).map_err(|error| {
            RefineError::Serialization(format!("failed to encode process history: {error}"))
        })?;
        write_json_atomically(
            &self.process_history_path(&archived.id),
            &encoded,
            "process history",
        )?;
        remove_file_if_present(
            &self.processes_dir().join(format!("{}.json", archived.id)),
            "active process registry",
        )?;
        remove_file_if_present(
            &self.process_identity_path(&archived.id),
            "process identity",
        )?;
        Ok(archived)
    }

    pub fn register(&self, process: ManagedProcess) -> RefineResult<ManagedProcess> {
        if process.state != "running" {
            self.write_process(&process)?;
            return self.archive_terminal_process(&process);
        }
        let existing = match self.inspect(&process.id) {
            Ok(existing) => Some(existing),
            Err(RefineError::NotFound(_)) => None,
            Err(error) => return Err(error),
        };
        if let Some(existing) = existing {
            if existing.owner != process.owner
                || existing.pid != process.pid
                || existing.started_at != process.started_at
            {
                return Err(RefineError::Conflict(format!(
                    "managed process {} registration changed ownership identity; its existing process record and identity evidence were retained",
                    process.id
                )));
            }
            let identity = self.load_process_identity(&existing)?;
            self.ensure_identity_matches_process(&existing, &identity)?;
            self.write_process(&process)?;
            return Ok(process);
        }
        self.write_process(&process)?;
        if let Err(error) = self.create_process_identity(&process) {
            let _ = self.remove_process_artifacts(&process);
            return Err(error);
        }
        Ok(process)
    }
}

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

    /// Register a run-to-completion process whose record is unlinked before the
    /// launch call returns. The record only serves live observability, so it is
    /// written without the durability fsync a long-lived registration needs.
    pub(super) fn write_process_transient(&self, process: &ManagedProcess) -> RefineResult<()> {
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
        write_json_atomically_transient(&path, &encoded, "process")
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

    /// Serializes terminal archiving against explicit-stop cleanup.
    ///
    /// The launching process's reaper and a stopper are usually different
    /// supervisor instances — often different OS processes — so in-memory
    /// coordination cannot order them. Without this lock the two interleaved
    /// freely: archive-then-remove left nothing (fine), but remove-then-archive
    /// resurrected a history record for a process an explicit stop had already
    /// cleaned. The flock is advisory, per process id, and released on drop.
    pub(super) fn cleanup_lock(&self, process_id: &str) -> RefineResult<fs::File> {
        self.prepare_artifact_coordination()?;
        fs::create_dir_all(self.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process registry {}: {error}",
                self.processes_dir().display()
            ))
        })?;
        let path = self.cleanup_lock_path(process_id);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open process cleanup lock {}: {error}",
                    path.display()
                ))
            })?;
        crate::infrastructure::process::supervisor::coordination::lock_exclusive_before(
            &file,
            &path,
            Duration::from_millis(200),
        )
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to lock process cleanup lock {}: {error}",
                path.display()
            ))
        })?;
        Ok(file)
    }

    pub(super) fn cleanup_lock_path(&self, process_id: &str) -> PathBuf {
        self.processes_dir()
            .join(format!(".{process_id}.cleanup.lock"))
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
        let _registration = self.artifact_registration_fence(&process.id)?;
        let lock = self.cleanup_lock(&process.id)?;
        let archived = self.archive_terminal_process_locked(process);
        FileExt::unlock(&lock).ok();
        archived
    }

    fn archive_terminal_process_locked(
        &self,
        process: &ManagedProcess,
    ) -> RefineResult<ManagedProcess> {
        let registration = self.processes_dir().join(format!("{}.json", process.id));
        // A missing registration means an explicit stop already removed this
        // process and every artifact it owned; writing history now would
        // resurrect a record the stop deliberately deleted.
        if !registration.exists() {
            return Ok(process.clone());
        }
        fs::create_dir_all(self.process_history_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create process history {}: {error}",
                self.process_history_dir().display()
            ))
        })?;
        // A leader exit is not a group exit. Preserve registration, identities and command
        // evidence while maintenance still observes descendants or cannot prove their exit.
        if (Self::requires_group_ownership(process) || self.group_path(&process.id).exists())
            && self.group_pending(process).unwrap_or(true)
        {
            return Ok(process.clone());
        }
        let mut archived = process.clone();
        let handoff_path = self.artifact_handoff_path(&archived.id);
        let mut handoff = None;
        // A reaper can observe the child exit before its workflow consumer has
        // finished settlement. Keep the command queue and its history pointer
        // while that consumer still owns the artifact-handoff lease.
        let artifacts_deferred = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(&handoff_path)
        {
            Ok(file) => match file.try_lock_exclusive() {
                Ok(()) => {
                    handoff = Some(file);
                    false
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => true,
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to lock process artifact handoff {} for archiving: {error}",
                        handoff_path.display()
                    )));
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to open process artifact handoff {} for archiving: {error}",
                    handoff_path.display()
                )));
            }
        };
        if !artifacts_deferred && let Some(stdin_path) = archived.stdin_path.take() {
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
        remove_file_if_present(&registration, "active process registry")?;
        remove_file_if_present(
            &self.process_identity_path(&archived.id),
            "process identity",
        )?;
        drop(handoff);
        Ok(archived)
    }

    pub fn register(&self, process: ManagedProcess) -> RefineResult<ManagedProcess> {
        let _retirement = self.artifact_registration_fence(&process.id)?;
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

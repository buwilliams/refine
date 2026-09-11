//! Shared coordinated retirement of process artifacts. Missing evidence retains files.
use super::*;

impl FileProcessSupervisor {
    /// Hold transient process artifacts while a workflow-owned consumer finishes reading them.
    ///
    /// The filesystem lock is released automatically if the consumer exits, so later recovery can
    /// still remove abandoned artifacts.
    pub(crate) fn begin_artifact_handoff(&self, process_id: &str) -> RefineResult<fs::File> {
        self.prepare_artifact_coordination()?;
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
        crate::infrastructure::process::supervisor::coordination::lock_exclusive_before(
            &file,
            &path,
            Duration::from_millis(200),
        )
        .map_err(|error| {
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

    pub(super) fn remove_process_artifacts(&self, process: &ManagedProcess) -> RefineResult<()> {
        if (Self::requires_group_ownership(process) || self.group_path(&process.id).exists())
            && self.group_pending(process).unwrap_or(true)
        {
            return Ok(());
        }

        let _registration = self.artifact_registration_fence(&process.id)?;
        let lock = self.cleanup_lock(&process.id)?;
        if !self.artifacts_may_retire(process)? {
            return Ok(());
        }
        let removed = self.remove_process_artifacts_locked(process);
        FileExt::unlock(&lock).ok();
        removed
    }

    fn remove_process_artifacts_locked(&self, process: &ManagedProcess) -> RefineResult<()> {
        let handoff_path = self.artifact_handoff_path(&process.id);
        // A live workflow consumer owns the transcript through this lease. Reconciliation may
        // already persist a truthful terminal state, but deletion waits until consumption ends.
        let handoff = match OpenOptions::new()
            .create(true)
            .truncate(false)
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
        if let Some(signal) = process
            .details
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v["signal_path"].as_str().map(PathBuf::from))
        {
            if signal.parent() == Some(self.processes_dir().as_path()) {
                remove_file_if_present(&signal, "settled process signal")?;
            }
        }
        let _handoff = handoff; // Keep the lease held through every removal. Never unlink its inode.
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
        // A deferred handoff returned before any removal. Retire ownership only
        // after the process and its artifacts have actually been retired.
        if process.owner == ProcessOwner::Maintenance {
            remove_file_if_present(&self.group_path(&process.id), "completed transient group")?;
        }
        Ok(())
    }
}

impl FileProcessSupervisor {
    /// Persistent coordination belongs to this runtime, never to the app's
    /// source inventory. Ignore only supervisor lock files and this marker;
    /// preserve any existing runtime ignore policy unchanged.
    pub(super) fn prepare_artifact_coordination(&self) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|e| RefineError::Io(e.to_string()))?;
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.runtime_root.join(".gitignore"))
        {
            Ok(mut file) => file
                .write_all(b"/processes/*.lock\n/runtime/record-locks/*.lock\n/.gitignore\n")
                .map_err(|e| RefineError::Io(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(RefineError::Io(e.to_string())),
        }
    }

    pub(super) fn artifact_registration_fence(
        &self,
        id: &str,
    ) -> RefineResult<crate::infrastructure::process::supervisor::coordination::RecordLease> {
        self.prepare_artifact_coordination()?;
        use crate::infrastructure::process::supervisor::coordination::{
            acquire_record_lock, with_lock_timeout,
        };
        with_lock_timeout(Duration::from_millis(200), || {
            acquire_record_lock(&self.runtime_root, &format!("artifact-registration:{id}"))
        })
    }

    fn artifacts_may_retire(&self, process: &ManagedProcess) -> RefineResult<bool> {
        // Reread registration without inspect()'s archiving side effects.
        match fs::read(self.processes_dir().join(format!("{}.json", process.id))) {
            Ok(bytes) => {
                let current: ManagedProcess = serde_json::from_slice(&bytes)
                    .map_err(|e| RefineError::Serialization(e.to_string()))?;
                if current.pid != process.pid
                    || current.owner != process.owner
                    || current.started_at != process.started_at
                {
                    return Ok(false);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(RefineError::Io(e.to_string())),
        }
        Ok(!self.group_pending(process)?)
    }

    /// Best-effort retention sweep; uncertainty for one artifact cannot suppress
    /// another registration's independently proven retirement.
    pub fn retire_aged_process_logs(&self, retention: Duration) {
        let Ok(entries) = fs::read_dir(self.processes_dir()) else {
            return;
        };
        let entries = entries.flatten().collect::<Vec<_>>();
        #[cfg(test)]
        run_after_process_enumeration_hook(&self.runtime_root);
        for entry in entries {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(id) = name
                .strip_suffix(".stdout.log")
                .or_else(|| name.strip_suffix(".stderr.log"))
                .or_else(|| name.strip_suffix(".stdin.txt"))
            else {
                continue;
            };
            let _ = self.retire_aged_log(id, &path, retention);
        }
    }
    fn retire_aged_log(&self, id: &str, path: &Path, retention: Duration) -> RefineResult<()> {
        let _registration = self.artifact_registration_fence(id)?;
        let _cleanup = self.cleanup_lock(id)?;
        let handoff_path = self.artifact_handoff_path(id);
        let handoff = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&handoff_path)
            .map_err(|e| RefineError::Io(e.to_string()))?;
        if handoff.try_lock_exclusive().is_err() {
            return Ok(());
        }
        // These observations occur under the same registration/cleanup fences
        // as publication and retirement, and while holding the stable handoff inode.
        match fs::symlink_metadata(self.processes_dir().join(format!("{id}.json"))) {
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(RefineError::Io(e.to_string())),
        }
        let bytes = fs::read(self.group_path(id)).map_err(|e| RefineError::Io(e.to_string()))?;
        let group: owned_groups::OwnedGroup = serde_json::from_slice(&bytes)
            .map_err(|e| RefineError::Serialization(e.to_string()))?;
        if group.process.id != id || !self.artifacts_may_retire(&group.process)? {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(path).map_err(|e| RefineError::Io(e.to_string()))?;
        if metadata.is_file()
            && metadata
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > retention)
        {
            fs::remove_file(path).map_err(|e| RefineError::Io(e.to_string()))?;
        }
        Ok(())
    }
}

impl FileProcessSupervisor {
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
        // Serialize against the launcher's reaper (see `cleanup_lock`): without
        // this, an archive racing the removal below could resurrect records
        // for a process this cleanup deliberately retired. Held until return;
        // the flock releases when the file handle drops.
        let _registration = match self.artifact_registration_fence(&expected.id) {
            Ok(lock) => lock,
            Err(error) => return Err(ConfirmedProcessCleanupFailure { outcome, error }),
        };
        let _cleanup_lock = match self.cleanup_lock(&expected.id) {
            Ok(lock) => lock,
            Err(error) => return Err(ConfirmedProcessCleanupFailure { outcome, error }),
        };
        match self.artifacts_may_retire(expected) {
            Ok(true) => {}
            Ok(false) => {
                return Err(ConfirmedProcessCleanupFailure {
                    outcome,
                    error: RefineError::Degraded(
                        "owned execution exit is unverified; retain registration and artifacts"
                            .into(),
                    ),
                });
            }
            Err(error) => return Err(ConfirmedProcessCleanupFailure { outcome, error }),
        }
        let handoff_path = self.artifact_handoff_path(&expected.id);
        let handoff = match OpenOptions::new()
            .create(true)
            .truncate(false)
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
        // Stable lock inodes also cover callers already waiting for these leases.
        Ok(outcome)
    }
}

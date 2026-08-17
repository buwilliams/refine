use super::*;

#[derive(Clone, Debug)]
pub(super) struct FileSourcePromotionHost {
    service: FileSourcePromotionService,
    previous_executable: PathBuf,
    previous_binary_backup: PathBuf,
    candidate_artifact: Option<PathBuf>,
    pub(crate) candidate_builder: PathBuf,
}

impl FileSourcePromotionHost {
    pub(super) fn new(service: FileSourcePromotionService) -> Self {
        let previous_executable = service.checkout_path.join("bin/refine");
        let previous_binary_backup = service
            .port_runtime_root
            .join("source-promotion/prior-bin-refine");
        Self {
            service,
            previous_executable,
            previous_binary_backup,
            candidate_artifact: None,
            candidate_builder: PathBuf::from("cargo"),
        }
    }

    fn runtime_root(&self) -> RefineResult<PathBuf> {
        self.service
            .port_runtime_root
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| RefineError::InvalidInput("port runtime root has no parent".to_string()))
    }

    fn launch(&self, executable: &Path) -> RefineResult<()> {
        let runtime_root = self.runtime_root()?;
        let output = Command::new(executable)
            .args([
                "system",
                "start",
                "--port",
                &self.service.port.to_string(),
                "--runtime-root",
                &runtime_root.display().to_string(),
            ])
            .current_dir(&self.service.checkout_path)
            .output()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to launch Refine from {}: {error}",
                    executable.display()
                ))
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RefineError::Degraded(format!(
                "Refine restart failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}

impl SourcePromotionHost for FileSourcePromotionHost {
    fn build_candidate(&mut self, commit: &str) -> RefineResult<PathBuf> {
        let root = self.service.port_runtime_root.join("source-promotion");
        let artifact_id = format!(
            "{}-{}",
            &commit[..commit.len().min(12)],
            Uuid::new_v4().simple()
        );
        let worktree = root.join(format!("candidate-{artifact_id}"));
        let binary = root.join(format!("refine-{artifact_id}"));
        fs::create_dir_all(&root).map_err(|error| {
            RefineError::Io(format!("failed to create {}: {error}", root.display()))
        })?;
        if let Err(error) = git_ok(
            &self.service.checkout_path,
            &[
                "worktree",
                "add",
                "--detach",
                &worktree.display().to_string(),
                commit,
            ],
        ) {
            let cleanup_errors =
                cleanup_candidate_worktree(&self.service.checkout_path, &worktree, false);
            return Err(with_candidate_cleanup(error, &worktree, cleanup_errors));
        }
        let candidate_result = Command::new(&self.candidate_builder)
            .args(["build", "--release", "--locked"])
            .env("REFINE_BUILD_SOURCE_COMMIT", commit)
            .current_dir(&worktree)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to launch candidate build: {error}")))
            .and_then(|build| {
                let built = worktree.join("target/release/refine");
                if build.status.success() {
                    fs::copy(&built, &binary).map(|_| ()).map_err(|error| {
                        RefineError::Io(format!(
                            "failed to preserve candidate binary {} as {}: {error}",
                            built.display(),
                            binary.display()
                        ))
                    })
                } else {
                    Err(RefineError::Degraded(format!(
                        "candidate build failed with status {}: {}",
                        build.status,
                        String::from_utf8_lossy(&build.stderr).trim()
                    )))
                }
            });
        let mut cleanup_errors =
            cleanup_candidate_worktree(&self.service.checkout_path, &worktree, true);
        match candidate_result {
            Ok(()) if cleanup_errors.is_empty() => Ok(binary),
            Ok(()) => {
                cleanup_errors.extend(remove_candidate_binary(&binary));
                Err(with_candidate_cleanup(
                    RefineError::Io(
                        "candidate build succeeded but artifact cleanup failed".to_string(),
                    ),
                    &worktree,
                    cleanup_errors,
                ))
            }
            Err(error) => {
                cleanup_errors.extend(remove_candidate_binary(&binary));
                Err(with_candidate_cleanup(error, &worktree, cleanup_errors))
            }
        }
    }

    fn verify_preconditions(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()> {
        let mut snapshot = self.service.inspect(false)?;
        snapshot
            .active_work
            .retain(|item| !item.starts_with("source promotion "));
        if snapshot.current_commit != from_commit || snapshot.available_commit != to_commit {
            return Err(RefineError::Conflict(
                "source commits changed while the candidate was building; activation was aborted"
                    .to_string(),
            ));
        }
        validate_promotion(&snapshot)
    }

    fn prepare_restart(
        &mut self,
        _executable: &Path,
    ) -> RefineResult<Option<ServiceRegistrationUpdate>> {
        if self.previous_binary_backup.exists() {
            return Err(RefineError::Conflict(format!(
                "a prior deployed-binary backup remains at {}; reconcile it before another promotion",
                self.previous_binary_backup.display()
            )));
        }
        if !self.previous_executable.is_file() {
            return Err(RefineError::NotFound(format!(
                "checkout-local deployed binary is missing: {}; run `./r system build` to restore it, then retry the update",
                self.previous_executable.display()
            )));
        }
        if let Some(parent) = self.previous_binary_backup.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create binary backup directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::copy(&self.previous_executable, &self.previous_binary_backup).map_err(|error| {
            RefineError::Io(format!(
                "failed to preserve deployed binary {} as {}: {error}",
                self.previous_executable.display(),
                self.previous_binary_backup.display()
            ))
        })?;
        fs::File::open(&self.previous_binary_backup)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to synchronize deployed-binary backup {}: {error}",
                    self.previous_binary_backup.display()
                ))
            })?;
        let registration = FileInstallationService::for_port(
            self.runtime_root()?,
            env!("CARGO_PKG_VERSION"),
            self.service.port,
        )
        .prepare_service_executable(&self.previous_executable);
        if registration.is_err() {
            let _ = fs::remove_file(&self.previous_binary_backup);
        }
        registration
    }

    fn stop_daemon(&mut self) -> RefineResult<()> {
        let runtime_root = self.runtime_root()?;
        crate::tools::host::daemon_lifecycle::execute_daemon_lifecycle(
            &crate::tools::host::daemon_lifecycle::FileHostDaemonLifecycleService::new(
                RuntimeRoot { root: runtime_root },
                env!("CARGO_PKG_VERSION"),
            ),
            crate::tools::host::daemon_lifecycle::DaemonLifecycleAction::Stop,
            crate::process::supervisor::lifecycle::BackgroundDaemonConfig {
                port: self.service.port,
                ..Default::default()
            },
        )?;
        for _ in 0..50 {
            if http_probe(self.service.port).is_err() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(RefineError::Degraded(format!(
            "Refine daemon on port {} did not stop",
            self.service.port
        )))
    }

    fn activate_candidate_binary(&mut self, candidate: &Path) -> RefineResult<PathBuf> {
        let parent = self.previous_executable.parent().ok_or_else(|| {
            RefineError::InvalidInput("deployed binary has no parent directory".to_string())
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!("failed to create {}: {error}", parent.display()))
        })?;
        let pending = parent.join("refine.source-promotion-pending");
        fs::copy(candidate, &pending).map_err(|error| {
            RefineError::Io(format!(
                "failed to stage source-promotion binary {} as {}: {error}",
                candidate.display(),
                pending.display()
            ))
        })?;
        fs::File::open(&pending)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to synchronize pending source-promotion binary {}: {error}",
                    pending.display()
                ))
            })?;
        replace_binary(&pending, &self.previous_executable).map_err(|error| {
            RefineError::Io(format!(
                "failed to activate checkout-local binary {}: {error}",
                self.previous_executable.display()
            ))
        })?;
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to synchronize deployed binary directory {}: {error}",
                    parent.display()
                ))
            })?;
        self.candidate_artifact = Some(candidate.to_path_buf());
        Ok(self.previous_executable.clone())
    }

    fn activate(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()> {
        let snapshot = self.service.inspect(false)?;
        if !snapshot.clean || snapshot.current_commit != from_commit {
            return Err(RefineError::Conflict(
                "controller checkout changed after candidate build; source activation was aborted"
                    .to_string(),
            ));
        }
        if !git_status(
            &self.service.checkout_path,
            &["merge-base", "--is-ancestor", from_commit, to_commit],
        )? {
            return Err(RefineError::Conflict(
                "fetched source is no longer a fast-forward of the controller checkout".to_string(),
            ));
        }
        let reference = format!("refs/heads/{}", snapshot.local_branch);
        update_checked_out_branch(
            &self.service.checkout_path,
            &reference,
            from_commit,
            to_commit,
        )
    }

    fn restart_daemon(&mut self, executable: &Path) -> RefineResult<()> {
        self.launch(executable)
    }

    fn verify_daemon(
        &mut self,
        expected_commit: &str,
        expected_executable: &Path,
    ) -> RefineResult<PathBuf> {
        http_probe(self.service.port)?;
        let observed_executable = live_daemon_executable(self.service.port)?;
        let expected_identity = fs::canonicalize(expected_executable).map_err(|error| {
            RefineError::Io(format!(
                "failed to canonicalize candidate executable {}: {error}",
                expected_executable.display()
            ))
        })?;
        let observed_identity = fs::canonicalize(&observed_executable).map_err(|error| {
            RefineError::Io(format!(
                "failed to canonicalize live daemon executable {}: {error}",
                observed_executable.display()
            ))
        })?;
        if observed_identity != expected_identity {
            return Err(RefineError::Degraded(format!(
                "daemon is reachable from {}, but the candidate executable is {}",
                observed_identity.display(),
                expected_identity.display()
            )));
        }
        let actual = git_text(&self.service.checkout_path, &["rev-parse", "HEAD"])?;
        if actual == expected_commit {
            Ok(observed_identity)
        } else {
            Err(RefineError::Degraded(format!(
                "daemon restarted but checkout commit is {actual}, expected {expected_commit}"
            )))
        }
    }

    fn complete_restart_registration(&mut self) -> RefineResult<()> {
        FileInstallationService::for_port(
            self.runtime_root()?,
            env!("CARGO_PKG_VERSION"),
            self.service.port,
        )
        .complete_service_executable_update()?;
        match fs::remove_file(&self.previous_binary_backup) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to remove deployed-binary backup {}: {error}",
                    self.previous_binary_backup.display()
                )));
            }
        }
        if let Some(candidate) = self.candidate_artifact.take() {
            match fs::remove_file(&candidate) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to remove source-promotion candidate {}: {error}",
                        candidate.display()
                    )));
                }
            }
        }
        Ok(())
    }

    fn restore_restart_registration(&mut self) -> RefineResult<bool> {
        FileInstallationService::for_port(
            self.runtime_root()?,
            env!("CARGO_PKG_VERSION"),
            self.service.port,
        )
        .restore_service_executable()
    }

    fn restore_previous_binary(&mut self) -> RefineResult<bool> {
        if !self.previous_binary_backup.exists() {
            return Ok(false);
        }
        replace_binary(&self.previous_binary_backup, &self.previous_executable).map_err(
            |error| {
                RefineError::Io(format!(
                    "failed to restore checkout-local binary {} from {}: {error}",
                    self.previous_executable.display(),
                    self.previous_binary_backup.display()
                ))
            },
        )?;
        Ok(true)
    }

    fn verify_previous_registration(&mut self) -> RefineResult<PathBuf> {
        FileInstallationService::for_port(
            self.runtime_root()?,
            env!("CARGO_PKG_VERSION"),
            self.service.port,
        )
        .verify_restored_service_executable(&self.previous_executable)
    }

    fn verify_previous_source(&mut self, expected_commit: &str) -> RefineResult<()> {
        let actual_commit = git_text(&self.service.checkout_path, &["rev-parse", "HEAD"])?;
        let status = git_text(&self.service.checkout_path, &["status", "--porcelain"])?;
        if actual_commit == expected_commit && status.is_empty() {
            Ok(())
        } else {
            Err(RefineError::Degraded(format!(
                "prior source is not authoritatively restored: checkout commit is {actual_commit}, expected {expected_commit}, clean={}",
                status.is_empty()
            )))
        }
    }

    fn rollback(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()> {
        let branch = git_text(
            &self.service.checkout_path,
            &["symbolic-ref", "--short", "HEAD"],
        )?;
        update_checked_out_branch(
            &self.service.checkout_path,
            &format!("refs/heads/{branch}"),
            to_commit,
            from_commit,
        )
    }

    fn restart_previous_daemon(&mut self) -> RefineResult<()> {
        crate::tools::host::daemon_lifecycle::execute_daemon_lifecycle(
            &crate::tools::host::daemon_lifecycle::FileHostDaemonLifecycleService::new(
                RuntimeRoot {
                    root: self.runtime_root()?,
                },
                env!("CARGO_PKG_VERSION"),
            ),
            crate::tools::host::daemon_lifecycle::DaemonLifecycleAction::Restart,
            crate::process::supervisor::lifecycle::BackgroundDaemonConfig {
                port: self.service.port,
                ..Default::default()
            },
        )
        .map(|_| ())
    }

    fn observe_previous_daemon(&mut self) -> SourcePromotionDaemonObservation {
        let expected_executable = self.previous_executable.display().to_string();
        match crate::process::supervisor::lifecycle::http_reachability_probe(self.service.port) {
            crate::process::supervisor::lifecycle::DaemonReachability::Reachable => {
                match live_daemon_executable(self.service.port) {
                    Ok(observed) => {
                        let observed_executable = Some(observed.display().to_string());
                        match (
                            fs::canonicalize(&self.previous_executable),
                            fs::canonicalize(&observed),
                        ) {
                            (Ok(expected), Ok(actual)) => SourcePromotionDaemonObservation {
                                reachability: "reachable".to_string(),
                                expected_executable,
                                observed_executable,
                                identity_matches: Some(expected == actual),
                                error: (expected != actual).then(|| {
                                    format!(
                                        "live daemon executable {} does not match restored prior executable {}",
                                        actual.display(),
                                        expected.display()
                                    )
                                }),
                            },
                            (expected, actual) => SourcePromotionDaemonObservation {
                                reachability: "reachable".to_string(),
                                expected_executable,
                                observed_executable,
                                identity_matches: None,
                                error: Some(format!(
                                    "daemon is reachable but executable identity is indeterminate: expected canonicalization={}; observed canonicalization={}",
                                    canonicalization_result(expected),
                                    canonicalization_result(actual)
                                )),
                            },
                        }
                    }
                    Err(error) => SourcePromotionDaemonObservation {
                        reachability: "reachable".to_string(),
                        expected_executable,
                        observed_executable: None,
                        identity_matches: None,
                        error: Some(format!(
                            "daemon is reachable but live executable identity is indeterminate: {error}"
                        )),
                    },
                }
            }
            crate::process::supervisor::lifecycle::DaemonReachability::Unreachable(error) => {
                SourcePromotionDaemonObservation {
                    reachability: "unreachable".to_string(),
                    expected_executable,
                    observed_executable: None,
                    identity_matches: None,
                    error: Some(format!(
                        "restored prior daemon is not reachable after replacement: {error}"
                    )),
                }
            }
            crate::process::supervisor::lifecycle::DaemonReachability::Unknown(error) => {
                SourcePromotionDaemonObservation {
                    reachability: "unknown".to_string(),
                    expected_executable,
                    observed_executable: None,
                    identity_matches: None,
                    error: Some(format!(
                        "restored prior daemon reachability is indeterminate after replacement: {error}"
                    )),
                }
            }
        }
    }
}

fn replace_binary(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(source, destination)
}

fn canonicalization_result(result: std::io::Result<PathBuf>) -> String {
    match result {
        Ok(path) => path.display().to_string(),
        Err(error) => format!("error ({error})"),
    }
}

use super::*;

#[derive(Clone, Debug)]
pub(super) struct FileSourcePromotionHost {
    service: FileSourcePromotionService,
    previous_executable: PathBuf,
    pub(crate) candidate_builder: PathBuf,
}

impl FileSourcePromotionHost {
    pub(super) fn new(service: FileSourcePromotionService) -> Self {
        let previous_executable =
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("refine"));
        Self {
            service,
            previous_executable,
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

    fn stop_daemon(&mut self) -> RefineResult<()> {
        let runtime_root = self.runtime_root()?;
        FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root })
            .stop(self.service.port)?;
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

    fn verify_daemon(&mut self, expected_commit: &str) -> RefineResult<()> {
        http_probe(self.service.port)?;
        let actual = git_text(&self.service.checkout_path, &["rev-parse", "HEAD"])?;
        if actual == expected_commit {
            Ok(())
        } else {
            Err(RefineError::Degraded(format!(
                "daemon restarted but checkout commit is {actual}, expected {expected_commit}"
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
        let executable = self.previous_executable.clone();
        self.launch(&executable)
    }
}

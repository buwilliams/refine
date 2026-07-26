use super::*;

#[derive(Clone, Debug)]
pub struct FileSourcePromotionService {
    pub checkout_path: PathBuf,
    pub port_runtime_root: PathBuf,
    pub port: u16,
}

impl FileSourcePromotionService {
    pub fn new(
        checkout_path: impl Into<PathBuf>,
        port_runtime_root: impl Into<PathBuf>,
        port: u16,
    ) -> Self {
        Self {
            checkout_path: checkout_path.into(),
            port_runtime_root: port_runtime_root.into(),
            port,
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.port_runtime_root.join(SOURCE_PROMOTION_STATE_FILE)
    }

    pub fn inspect(&self, fetch: bool) -> RefineResult<SourcePromotionSnapshot> {
        ensure_checkout(&self.checkout_path)?;
        let local_branch = git_text(&self.checkout_path, &["symbolic-ref", "--short", "HEAD"])?;
        let current_commit = git_text(&self.checkout_path, &["rev-parse", "HEAD"])?;
        let remote = git_optional_text(
            &self.checkout_path,
            &["config", "--get", &format!("branch.{local_branch}.remote")],
        )?
        .filter(|value| value != ".")
        .unwrap_or_else(|| "origin".to_string());
        let merge_ref = git_optional_text(
            &self.checkout_path,
            &["config", "--get", &format!("branch.{local_branch}.merge")],
        )?
        .unwrap_or_else(|| format!("refs/heads/{local_branch}"));
        let remote_branch = merge_ref
            .strip_prefix("refs/heads/")
            .unwrap_or(&local_branch)
            .to_string();
        if fetch {
            git_ok(&self.checkout_path, &["fetch", "--prune", &remote])?;
        }
        let available_ref = format!("{remote}/{remote_branch}");
        let available_commit = git_text(&self.checkout_path, &["rev-parse", &available_ref])?;
        let clean = git_text(&self.checkout_path, &["status", "--porcelain"])?.is_empty();
        let fast_forward = git_status(
            &self.checkout_path,
            &[
                "merge-base",
                "--is-ancestor",
                &current_commit,
                &available_commit,
            ],
        )?;
        let active_work = self.active_work()?;
        Ok(SourcePromotionSnapshot {
            checkout_path: self.checkout_path.display().to_string(),
            update_available: current_commit != available_commit,
            current_commit,
            remote,
            local_branch,
            branch: remote_branch,
            available_commit,
            clean,
            fast_forward,
            active_work,
            operation: self.load_operation()?,
        })
    }

    pub fn check(&self) -> RefineResult<SourcePromotionSnapshot> {
        self.inspect(true)
    }

    pub fn queue(&self) -> RefineResult<SourcePromotionOperation> {
        http_probe(self.port).map_err(|_| {
            RefineError::Conflict(format!(
                "source promotion requires a healthy running Refine daemon on port {}",
                self.port
            ))
        })?;
        let snapshot = self.check()?;
        validate_promotion(&snapshot)?;
        let executable = std::env::current_exe().map_err(|error| {
            RefineError::Io(format!(
                "failed to locate source-promotion helper executable: {error}"
            ))
        })?;
        self.queue_validated(
            &snapshot,
            &executable,
            &ProcessSourcePromotionHelperLauncher,
        )
    }

    pub(crate) fn queue_validated(
        &self,
        snapshot: &SourcePromotionSnapshot,
        executable: &Path,
        launcher: &dyn SourcePromotionHelperLauncher,
    ) -> RefineResult<SourcePromotionOperation> {
        let operation = SourcePromotionOperation::queued(snapshot);
        self.save_operation(&operation)?;
        let mut command = Command::new(executable);
        command
            .args([
                "system",
                "source-promote-helper",
                "--checkout",
                &snapshot.checkout_path,
                "--port-runtime-root",
                &self.port_runtime_root.display().to_string(),
                "--port",
                &self.port.to_string(),
                "--operation-id",
                &operation.id,
            ])
            .current_dir(&self.checkout_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Err(error) = launcher.launch(&mut command) {
            let launch_error = RefineError::Io(format!(
                "failed to launch restart-safe source-promotion helper {}: {error}",
                executable.display()
            ));
            let mut failed = operation.clone();
            failed.status = "failed".to_string();
            failed.stage = "launch_helper".to_string();
            failed.message = "Source promotion helper could not start".to_string();
            failed.error = Some(launch_error.to_string());
            failed.recovery = Some(
                "No checkout or daemon changes were made; resolve the launch failure and retry"
                    .to_string(),
            );
            failed.updated_at = now_timestamp();
            if let Err(persist_error) = self.save_operation(&failed) {
                return Err(append_error_context(
                    launch_error,
                    &format!(
                        "the terminal failure state also could not be persisted: {persist_error}"
                    ),
                ));
            }
            return Err(launch_error);
        }
        Ok(operation)
    }

    pub fn run_helper(&self, operation_id: &str) -> RefineResult<SourcePromotionOperation> {
        let mut operation = self.load_operation()?.ok_or_else(|| {
            RefineError::NotFound("source-promotion operation state was not found".to_string())
        })?;
        if operation.id != operation_id {
            return Err(RefineError::Conflict(format!(
                "source-promotion operation {} is no longer current",
                operation_id
            )));
        }
        // Allow the initiating HTTP response to leave the daemon before the
        // helper marks it unhealthy and waits for shutdown.
        thread::sleep(Duration::from_millis(750));
        let mut snapshot = match self.check() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                operation.status = "failed".to_string();
                operation.stage = "preflight".to_string();
                operation.message = "Source promotion failed during preflight".to_string();
                operation.error = Some(error.to_string());
                operation.recovery = Some(
                    "Check remote connectivity and source state, then check again; no checkout or daemon changes were made"
                        .to_string(),
                );
                operation.updated_at = now_timestamp();
                self.save_operation(&operation)?;
                return Err(error);
            }
        };
        snapshot.active_work.retain(|item| {
            item != &format!("source promotion {} is {}", operation.id, operation.status)
        });
        if snapshot.current_commit != operation.from_commit
            || snapshot.available_commit != operation.to_commit
        {
            let error = RefineError::Conflict(
                "source commits changed after promotion was queued; check again before retrying"
                    .to_string(),
            );
            operation.status = "failed".to_string();
            operation.stage = "preflight".to_string();
            operation.message = "Source promotion failed during preflight".to_string();
            operation.error = Some(error.to_string());
            operation.recovery = Some(
                "Check for source updates again; no checkout or daemon changes were made"
                    .to_string(),
            );
            operation.updated_at = now_timestamp();
            self.save_operation(&operation)?;
            return Err(error);
        }
        if let Err(error) = validate_promotion(&snapshot) {
            operation.status = "failed".to_string();
            operation.stage = "preflight".to_string();
            operation.message = "Source promotion failed during preflight".to_string();
            operation.error = Some(error.to_string());
            operation.recovery = Some(
                "Resolve the preflight condition and check for source updates again; no checkout or daemon changes were made"
                    .to_string(),
            );
            operation.updated_at = now_timestamp();
            self.save_operation(&operation)?;
            return Err(error);
        }
        let mut host = FileSourcePromotionHost::new(self.clone());
        run_source_promotion(&mut host, &mut operation, |operation| {
            self.save_operation(operation)
        })?;
        Ok(operation)
    }

    pub fn load_operation(&self) -> RefineResult<Option<SourcePromotionOperation>> {
        match fs::read(self.state_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse source-promotion state {}: {error}",
                    self.state_path().display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RefineError::Io(format!(
                "failed to read source-promotion state {}: {error}",
                self.state_path().display()
            ))),
        }
    }

    fn save_operation(&self, operation: &SourcePromotionOperation) -> RefineResult<()> {
        fs::create_dir_all(&self.port_runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create port runtime root {}: {error}",
                self.port_runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(operation).map_err(|error| {
            RefineError::Serialization(format!("failed to encode source-promotion state: {error}"))
        })?;
        let pending = self.state_path().with_extension("json.pending");
        fs::write(&pending, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write source-promotion state {}: {error}",
                pending.display()
            ))
        })?;
        fs::rename(&pending, self.state_path()).map_err(|error| {
            RefineError::Io(format!(
                "failed to publish source-promotion state {}: {error}",
                self.state_path().display()
            ))
        })
    }

    pub(crate) fn active_work(&self) -> RefineResult<Vec<String>> {
        let mut active = Vec::new();
        let workflow_path = self
            .port_runtime_root
            .join("workflow-automation-state.json");
        if workflow_path.is_file() {
            let value: Value =
                serde_json::from_slice(&fs::read(&workflow_path).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to read {}: {error}",
                        workflow_path.display()
                    ))
                })?)
                .map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to parse {}: {error}",
                        workflow_path.display()
                    ))
                })?;
            if let Some(claims) = value.get("claims").and_then(Value::as_array) {
                for claim in claims {
                    let state = claim.get("state").and_then(Value::as_str).unwrap_or("");
                    if matches!(state, "claimed" | "running") {
                        let goal = claim
                            .get("goal_id")
                            .or_else(|| claim.get("gap_id"))
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        active.push(format!("active Goal claim {goal}"));
                    }
                }
            }
        }
        let supervisor = FileProcessSupervisor::new(&self.port_runtime_root);
        let pause_state = supervisor.pause_state()?;
        if !pause_state.workflow_paused {
            active.push("workflow automation is not paused".to_string());
        }
        for process in supervisor.list()? {
            if process.state == "running"
                && !matches!(process.owner, ProcessOwner::Daemon | ProcessOwner::Runner)
            {
                active.push(format!(
                    "running {} process {}",
                    process.owner.as_kind(),
                    process.id
                ));
            }
        }
        if let Some(operation) = self.load_operation()?
            && matches!(operation.status.as_str(), "queued" | "running")
        {
            active.push(format!(
                "source promotion {} is {}",
                operation.id, operation.status
            ));
        }
        active.sort();
        active.dedup();
        Ok(active)
    }
}

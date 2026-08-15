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

    pub(crate) fn operation_registry(&self) -> FileOperationRegistry {
        FileOperationRegistry::new(&self.port_runtime_root)
    }

    pub(crate) fn register_exclusive_operation(
        &self,
        owner: &str,
        request: Value,
    ) -> RefineResult<(OperationHandle, bool)> {
        let registry = self.operation_registry();
        match registry.register_exclusive_with_request(owner, request) {
            Ok(operation) => Ok((operation, true)),
            Err(RefineError::Conflict(_)) => registry
                .recover()?
                .into_iter()
                .find(|operation| {
                    operation.owner == owner
                        && matches!(
                            operation.state,
                            OperationState::Pending
                                | OperationState::Running
                                | OperationState::Cancelling
                        )
                })
                .map(|operation| (operation, false))
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "an exclusive {owner} operation changed while it was being recovered"
                    ))
                }),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn correlated_processes(
        &self,
        operation_id: &str,
    ) -> RefineResult<Vec<crate::process::subprocess::ManagedProcess>> {
        Ok(FileProcessSupervisor::new(&self.port_runtime_root)
            .recover()?
            .into_iter()
            .filter(|process| {
                process.details.as_deref().is_some_and(|details| {
                    serde_json::from_str::<Value>(details)
                        .ok()
                        .and_then(|value| {
                            value
                                .get("operation_id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .as_deref()
                        == Some(operation_id)
                })
            })
            .collect())
    }

    pub(crate) fn settle_registry_from_source_operation(
        &self,
        operation: &SourcePromotionOperation,
    ) -> RefineResult<()> {
        let registry = self.operation_registry();
        let Ok(handle) = registry.status(&operation.id) else {
            return Ok(());
        };
        if matches!(
            handle.state,
            OperationState::Succeeded
                | OperationState::Failed
                | OperationState::Cancelled
                | OperationState::Interrupted
        ) {
            return Ok(());
        }
        if matches!(handle.state, OperationState::Cancelling) {
            return Ok(());
        }
        match operation.status.as_str() {
            "succeeded" => {
                registry.finish_with_result(
                    &operation.id,
                    OperationState::Succeeded,
                    serde_json::to_value(operation).map_err(|error| {
                        RefineError::Serialization(format!(
                            "failed to encode source-upgrade result: {error}"
                        ))
                    })?,
                )?;
            }
            "failed" | "interrupted" => {
                registry.fail_with_partial_result(
                    &operation.id,
                    json!({
                        "code": format!("source_upgrade_{}", operation.status),
                        "message": operation.error.as_deref().unwrap_or(&operation.message),
                        "retryable": true
                    }),
                    serde_json::to_value(operation).map_err(|error| {
                        RefineError::Serialization(format!(
                            "failed to encode source-upgrade failure: {error}"
                        ))
                    })?,
                )?;
            }
            _ => {}
        }
        Ok(())
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
            operation: self.reconcile_interrupted_agent()?,
        })
    }

    pub fn check(&self) -> RefineResult<SourcePromotionSnapshot> {
        self.inspect(true)
    }

    /// Preserve any uncommitted work in a named git stash so an update can
    /// proceed on a clean tree. Returns the stash reference when work was
    /// stashed, or `None` when the tree was already clean. The stash is never
    /// reapplied automatically; it is reported to the operator instead.
    pub(crate) fn stash_dirty_tree(&self) -> RefineResult<Option<String>> {
        ensure_checkout(&self.checkout_path)?;
        if git_text(&self.checkout_path, &["status", "--porcelain"])?.is_empty() {
            return Ok(None);
        }
        let label = format!("refine-update-{}", now_timestamp());
        git_ok(
            &self.checkout_path,
            &["stash", "push", "--include-untracked", "-m", &label],
        )
        .map_err(|error| {
            RefineError::Conflict(format!(
                "the checkout has uncommitted changes and they could not be preserved in a stash: {error}"
            ))
        })?;
        let commit = git_text(&self.checkout_path, &["rev-parse", "stash@{0}"])?;
        Ok(Some(format!("{commit} ({label})")))
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
        let executable = self.checkout_path.join("bin/refine");
        require_checkout_binary(&executable, "source-promotion helper")?;
        self.queue_validated(&snapshot, &executable, &HostRestartSafeHandoffLauncher)
    }

    pub(crate) fn queue_validated(
        &self,
        snapshot: &SourcePromotionSnapshot,
        executable: &Path,
        launcher: &dyn RestartSafeHandoffLauncher,
    ) -> RefineResult<SourcePromotionOperation> {
        let mut operation = SourcePromotionOperation::queued(snapshot);
        self.operation_registry().register_with_id(
            &operation.id,
            "maintenance:source-upgrade",
            json!({
                "kind": "source_upgrade",
                "restart_recovery": "capability",
                "defer_cancellation_terminal": true
            }),
        )?;
        self.save_operation(&operation)?;
        self.launch_operation(&mut operation, snapshot, executable, launcher)?;
        Ok(operation)
    }

    pub(super) fn launch_operation(
        &self,
        operation: &mut SourcePromotionOperation,
        snapshot: &SourcePromotionSnapshot,
        executable: &Path,
        launcher: &dyn RestartSafeHandoffLauncher,
    ) -> RefineResult<()> {
        if let Err(launch_error) =
            self.launch_operation_two_phase(operation, snapshot, executable, launcher)
        {
            let mut failed = operation.clone();
            let injected_crash = launch_error.to_string().contains("injected crash");
            if !injected_crash {
                failed.status = "failed".to_string();
                failed.stage = "launch_helper".to_string();
                failed.message = "Source promotion helper could not start".to_string();
                failed.error = Some(launch_error.to_string());
                failed.recovery = Some(
                    "No checkout or daemon changes were made; resolve the launch failure and retry"
                        .to_string(),
                );
                failed.updated_at = now_timestamp();
                if let Ok(handle) = self.operation_registry().status(&failed.id) {
                    let _ = self.operation_registry().compare_and_set(
                        &failed.id,
                        handle.revision,
                        |candidate| {
                            candidate.state = OperationState::Failed;
                            candidate.error = Some(json!({
                                "code": "source_upgrade_helper_launch_failed",
                                "message": launch_error.to_string(),
                                "retryable": true
                            }));
                            if let Some(attempt) = candidate.external_attempt.as_mut() {
                                attempt.state = ExternalAttemptState::Failed;
                                attempt.terminal_evidence = candidate.error.clone();
                            }
                            Ok(())
                        },
                    );
                }
            }
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
        Ok(())
    }

    /// Block until the durable operation reaches a terminal status, invoking
    /// `on_transition` whenever its (status, stage, message) triple changes.
    ///
    /// Polling is file-based and reconciles interrupted agents each tick, so
    /// it survives the daemon restart that a successful promotion performs.
    /// Brief read misses are tolerated: the projection is written by atomic
    /// rename and the registry by compare-and-set, so a reader can catch a
    /// gap between the two.
    pub fn wait_for_terminal(
        &self,
        operation_id: &str,
        timeout: Duration,
        on_transition: &mut dyn FnMut(&SourcePromotionOperation),
    ) -> RefineResult<SourcePromotionOperation> {
        const MAX_CONSECUTIVE_MISSES: usize = 10;
        let deadline = std::time::Instant::now() + timeout;
        let mut last_transition: Option<(String, String, String)> = None;
        let mut consecutive_misses = 0usize;
        loop {
            match self.reconcile_interrupted_agent() {
                Ok(Some(operation)) => {
                    consecutive_misses = 0;
                    if operation.id != operation_id {
                        return Err(RefineError::Conflict(format!(
                            "source-update operation {operation_id} was superseded by {}",
                            operation.id
                        )));
                    }
                    let transition = (
                        operation.status.clone(),
                        operation.stage.clone(),
                        operation.message.clone(),
                    );
                    if last_transition.as_ref() != Some(&transition) {
                        last_transition = Some(transition);
                        on_transition(&operation);
                    }
                    if matches!(
                        operation.status.as_str(),
                        "succeeded" | "failed" | "interrupted" | "cancelled"
                    ) {
                        return Ok(operation);
                    }
                }
                Ok(None) => {
                    consecutive_misses += 1;
                    if consecutive_misses > MAX_CONSECUTIVE_MISSES {
                        return Err(RefineError::NotFound(format!(
                            "source-update operation {operation_id} disappeared while waiting for it"
                        )));
                    }
                }
                Err(error) => {
                    consecutive_misses += 1;
                    if consecutive_misses > MAX_CONSECUTIVE_MISSES {
                        return Err(error);
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(RefineError::Degraded(format!(
                    "the update did not reach a terminal state within {}s; it continues in the background — watch `./r system source-status` or the web UI",
                    timeout.as_secs()
                )));
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    pub fn run_helper(
        &self,
        operation_id: &str,
        attempt_id: Option<&str>,
        claim_nonce: Option<&str>,
    ) -> RefineResult<SourcePromotionOperation> {
        match (attempt_id, claim_nonce) {
            (Some(attempt_id), Some(claim_nonce)) => {
                self.claim_handoff(operation_id, attempt_id, claim_nonce)?;
            }
            (None, None) => {}
            _ => {
                return Err(RefineError::InvalidInput(
                    "restart-safe helper requires both attempt id and claim nonce".to_string(),
                ));
            }
        }
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
                let _ = self.restore_workflow_admission(&mut operation);
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
            let _ = self.restore_workflow_admission(&mut operation);
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
            let _ = self.restore_workflow_admission(&mut operation);
            self.save_operation(&operation)?;
            return Err(error);
        }
        let mut host = FileSourcePromotionHost::new(self.clone());
        let result = run_source_promotion(&mut host, &mut operation, |candidate| {
            if candidate.status == "succeeded" {
                let mut reconciling = candidate.clone();
                reconciling.status = "running".to_string();
                reconciling.stage = "post_restart_reconciliation".to_string();
                reconciling.message =
                    "Candidate identity verified; reconciling runtime services".to_string();
                self.save_operation(&reconciling)
            } else {
                self.save_operation(candidate)
            }
        });
        if result.is_ok() {
            operation.status = "running".to_string();
            operation.stage = "post_restart_reconciliation".to_string();
            operation.message =
                "Candidate identity verified; reconciling runtime services".to_string();
        }
        operation.primary_outcome = Some(if result.is_ok() {
            "succeeded".to_string()
        } else {
            "failed".to_string()
        });
        let restore_result = self.restore_workflow_admission(&mut operation);
        let reconciliation_result = if result.is_ok() && restore_result.is_ok() {
            self.verify_post_restart_reconciliation(&mut operation)
        } else {
            Ok(())
        };
        if result.is_ok() && restore_result.is_ok() && reconciliation_result.is_ok() {
            let _ = self.refresh_update_check_after_promotion();
        }
        self.save_operation(&operation)?;
        result?;
        restore_result?;
        reconciliation_result?;
        Ok(operation)
    }
}

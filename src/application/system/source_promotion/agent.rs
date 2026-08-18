use std::fs;

use fs2::FileExt;
use serde_json::{Value, json};

use crate::infrastructure::agents::invocation::{HostAgentProviderService, ProviderInvocation};

use super::*;

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentLaunchFailpoint {
    None,
    AfterReservation,
    AfterLaunchBeforeProjectionReceipt,
}

trait AgentFailpoint: Copy {
    fn after_reservation(self) -> bool;
    fn after_launch(self) -> bool;
}

#[cfg(test)]
impl AgentFailpoint for AgentLaunchFailpoint {
    fn after_reservation(self) -> bool {
        self == Self::AfterReservation
    }

    fn after_launch(self) -> bool {
        self == Self::AfterLaunchBeforeProjectionReceipt
    }
}

#[cfg(not(test))]
impl AgentFailpoint for () {
    fn after_reservation(self) -> bool {
        false
    }

    fn after_launch(self) -> bool {
        false
    }
}

impl FileSourcePromotionService {
    /// Queue the one-click maintenance path as one exclusive shared operation
    /// and launch the configured installed provider directly as its Agent.
    pub fn queue_agent(&self, provider: &str) -> RefineResult<SourcePromotionOperation> {
        let executable = self.checkout_path.join("bin/refine");
        require_checkout_binary(&executable, "typed source-upgrade capabilities")?;
        self.queue_agent_with(provider, &executable)
    }

    pub(crate) fn queue_agent_with(
        &self,
        provider: &str,
        executable: &std::path::Path,
    ) -> RefineResult<SourcePromotionOperation> {
        #[cfg(test)]
        return self.queue_agent_with_failpoint(provider, executable, AgentLaunchFailpoint::None);
        #[cfg(not(test))]
        self.queue_agent_inner(provider, executable, ())
    }

    #[cfg(test)]
    pub(crate) fn queue_agent_with_failpoint(
        &self,
        provider: &str,
        executable: &std::path::Path,
        failpoint: AgentLaunchFailpoint,
    ) -> RefineResult<SourcePromotionOperation> {
        self.queue_agent_inner(provider, executable, failpoint)
    }

    fn queue_agent_inner<F: AgentFailpoint>(
        &self,
        provider: &str,
        executable: &std::path::Path,
        failpoint: F,
    ) -> RefineResult<SourcePromotionOperation> {
        let _queue_guard = self.acquire_agent_queue_lock()?;
        if let Some(operation) = self.reconcile_interrupted_agent_locked()?
            && matches!(operation.status.as_str(), "queued" | "running")
        {
            return Ok(operation);
        }
        http_probe(self.port).map_err(|_| {
            RefineError::Conflict(format!(
                "source upgrade requires a healthy running Refine daemon on port {}",
                self.port
            ))
        })?;
        let mut snapshot = self.inspect_cached()?.source;
        // Intent is validated before normalization: a checkout with no real,
        // fast-forwardable update is never touched, so a dirty diverged tree
        // reaches the operator (or a rescue Agent) exactly as it was found.
        validate_update_intent(&snapshot)?;
        let stashed_changes = self.stash_dirty_tree()?;
        let with_stash_context = |error: RefineError| match &stashed_changes {
            Some(reference) => append_error_context(
                error,
                &format!(
                    "local changes were preserved in stash {reference} and are not reapplied automatically"
                ),
            ),
            None => error,
        };
        if stashed_changes.is_some() {
            self.refresh_cached_snapshot().map_err(with_stash_context)?;
            snapshot = self.inspect_cached().map_err(with_stash_context)?.source;
        }
        validate_source_update(&snapshot).map_err(with_stash_context)?;
        let supervisor = FileProcessSupervisor::new(&self.port_runtime_root);
        let prior_paused = supervisor
            .pause_state()
            .map_err(with_stash_context)?
            .workflow_paused;
        let provider = provider.trim();
        if provider.is_empty() {
            return Err(with_stash_context(RefineError::InvalidInput(
                "a configured installed Agent provider is required".to_string(),
            )));
        }
        let (registry_operation, created) = self
            .register_exclusive_operation(
                "maintenance:source-upgrade",
                json!({
                    "kind": "source_upgrade_agent",
                    "checkout": snapshot.checkout_path,
                    "provider": provider,
                    "restart_recovery": "capability"
                }),
            )
            .map_err(with_stash_context)?;
        if !created {
            return self.reconcile_interrupted_agent_locked()?.ok_or_else(|| {
                RefineError::Conflict(format!(
                    "source-upgrade operation {} has no durable source projection",
                    registry_operation.id
                ))
            });
        }
        let mut operation = SourcePromotionOperation::queued(&snapshot);
        operation.id = registry_operation.id;
        operation.stage = "agent_queued".to_string();
        operation.message = match &stashed_changes {
            Some(reference) => format!(
                "Refine update Agent queued; local changes preserved in stash {reference} (not reapplied automatically)"
            ),
            None => "Refine update Agent queued".to_string(),
        };
        operation.stashed_changes = stashed_changes.clone();
        operation.pre_upgrade_workflow_paused = Some(prior_paused);
        operation.agent_provider = Some(provider.to_string());
        let context_path = self
            .write_agent_context(&operation)
            .map_err(with_stash_context)?;
        operation.agent_context_path = Some(context_path.display().to_string());
        self.save_operation(&operation)
            .map_err(with_stash_context)?;

        if failpoint.after_reservation() {
            return Err(RefineError::Degraded(
                "injected daemon termination after source-upgrade reservation".to_string(),
            ));
        }

        let prompt = self.agent_prompt(executable, &operation)?;
        let mut metadata = serde_json::Map::new();
        metadata.insert("kind".to_string(), json!("source_upgrade_agent"));
        metadata.insert("profile".to_string(), json!("maintenance"));
        metadata.insert(
            "source_upgrade_operation".to_string(),
            Value::String(operation.id.clone()),
        );
        metadata.insert("operation_id".to_string(), json!(&operation.id));
        metadata.insert("provider".to_string(), json!(provider));
        let process = match HostAgentProviderService::with_runtime_root(&self.port_runtime_root)
            .launch_managed(ProviderInvocation {
                stall_timeout_seconds: None,
                provider: provider.to_string(),
                prompt,
                session_id: None,
                cwd: Some(snapshot.checkout_path.clone()),
                process_metadata: metadata,
            }) {
            Ok(process) => process,
            Err(error) => {
                self.fail_agent_operation(
                    &mut operation,
                    "agent_launch",
                    &error,
                    "No checkout, workflow, or daemon changes were made; resolve the Agent launch failure and retry",
                )?;
                return Err(error);
            }
        };
        if failpoint.after_launch() {
            return Err(RefineError::Degraded(
                "injected daemon termination after source-upgrade Agent launch".to_string(),
            ));
        }
        // The Agent may invoke its first capability before launch returns.
        // Merge its receipt into the latest record instead of overwriting a
        // newer decision with the pre-launch clone.
        let mut latest = self.current_operation(&operation.id)?;
        latest.agent_process_id = Some(process.id);
        if latest.stage == "agent_queued" {
            latest.status = "running".to_string();
            latest.stage = "agent_running".to_string();
            latest.message = format!("Installed {provider} update Agent is choosing its plan");
        }
        self.save_operation(&latest)?;
        Ok(latest)
    }

    fn agent_prompt(
        &self,
        executable: &std::path::Path,
        operation: &SourcePromotionOperation,
    ) -> RefineResult<String> {
        let context_path = operation.agent_context_path.as_deref().ok_or_else(|| {
            RefineError::NotFound("durable source-upgrade Agent context is missing".to_string())
        })?;
        let base = format!(
            "{} system source-upgrade-capability --checkout {} --port-runtime-root {} --port {} --operation-id {} --action",
            executable.display(),
            self.checkout_path.display(),
            self.port_runtime_root.display(),
            self.port,
            operation.id
        );
        Ok(format!(
            "You are the dedicated Refine source-upgrade Agent for operation {}. Read the complete durable context at {context_path}. You are outside Goal claims and workflow capacity. Inspect evidence and choose the safe sequence yourself. Never run Git, process, installation, service-manager, or lifecycle mutations directly. Invoke the granular typed Refine capabilities below separately, inspect each JSON result, retry observations when safe, and record an explicit recovery choice when evidence blocks promotion. Do not use or look for an aggregate execute command.\n\nAvailable commands:\n{base} inspect\n{base} pause-admission\n{base} observe-work\n{base} refresh-source\n{base} prepare-candidate\n{base} handoff-promotion\n{base} verify-post-restart\n{base} restore-admission\n{base} recover\n\nA safe normal plan inspects, pauses admission, observes until preserved managed work is settled, refreshes and validates source, prepares the candidate handoff, then starts promotion. Dirty or diverged source, restoration errors, and partial identity evidence require using recovery evidence rather than forcing progress. Report only outcomes proven by capability JSON.",
            operation.id
        ))
    }

    pub(crate) fn current_operation(
        &self,
        operation_id: &str,
    ) -> RefineResult<SourcePromotionOperation> {
        let operation = self.load_operation()?.ok_or_else(|| {
            RefineError::NotFound("source-upgrade operation state was not found".to_string())
        })?;
        if operation.id != operation_id {
            return Err(RefineError::Conflict(format!(
                "source-upgrade operation {operation_id} is no longer current"
            )));
        }
        Ok(operation)
    }

    fn write_agent_context(
        &self,
        operation: &SourcePromotionOperation,
    ) -> RefineResult<std::path::PathBuf> {
        fs::create_dir_all(&self.port_runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create source-upgrade context root {}: {error}",
                self.port_runtime_root.display()
            ))
        })?;
        let path = self
            .port_runtime_root
            .join(format!("source-upgrade-context-{}.json", operation.id));
        let context = json!({
            "operation_id": operation.id,
            "checkout": operation.checkout_path,
            "from_commit": operation.from_commit,
            "to_commit": operation.to_commit,
            "pre_upgrade_workflow_paused": operation.pre_upgrade_workflow_paused,
            "capabilities": [
                "workflow_admission",
                "managed_process_inspection",
                "source_promotion",
                "installation_registration",
                "daemon_lifecycle",
                "live_executable_identity",
                "rollback_and_recovery"
            ],
            "constraints": {
                "preserve_goal_claims_and_worktrees": true,
                "fast_forward_only": true,
                "no_direct_shell_mutation": true,
                "restore_exact_admission_intent": true
            }
        });
        fs::write(
            &path,
            serde_json::to_vec_pretty(&context).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to encode source-upgrade Agent context: {error}"
                ))
            })?,
        )
        .map_err(|error| RefineError::Io(format!("failed to write {}: {error}", path.display())))?;
        Ok(path)
    }

    pub(crate) fn restore_workflow_admission(
        &self,
        operation: &mut SourcePromotionOperation,
    ) -> RefineResult<()> {
        let Some(prior_paused) = operation.pre_upgrade_workflow_paused else {
            return Ok(());
        };
        match FileProcessSupervisor::new(&self.port_runtime_root).set_workflow_paused(prior_paused)
        {
            Ok(state) if state.workflow_paused == prior_paused => {
                operation.workflow_pause_restored = Some(true);
                operation.restoration_error = None;
                operation.updated_at = now_timestamp();
                self.save_operation(operation)
            }
            Ok(state) => {
                let error = RefineError::Degraded(format!(
                    "workflow admission restored to {} instead of the recorded prior state {}",
                    state.workflow_paused, prior_paused
                ));
                self.persist_restoration_failure(operation, &error)?;
                Err(error)
            }
            Err(error) => {
                self.persist_restoration_failure(operation, &error)?;
                Err(error)
            }
        }
    }

    fn persist_restoration_failure(
        &self,
        operation: &mut SourcePromotionOperation,
        error: &RefineError,
    ) -> RefineResult<()> {
        operation.workflow_pause_restored = Some(false);
        operation.restoration_error = Some(error.to_string());
        if operation.primary_outcome.is_none() {
            operation.primary_outcome = Some(match operation.error.as_deref() {
                Some(primary_error) => format!("{}: {primary_error}", operation.status),
                None => operation.status.clone(),
            });
        }
        operation.status = "failed".to_string();
        operation.stage = "restore_workflow_admission".to_string();
        operation.message = "Refine update requires workflow-admission recovery".to_string();
        operation.error = Some(format!(
            "Primary outcome: {}; workflow-admission restoration failed: {error}",
            operation.primary_outcome.as_deref().unwrap_or("unknown")
        ));
        operation.recovery = Some(format!(
            "Set workflow admission back to {} through the shared workflow control, verify preserved claims, then retry reconciliation",
            if operation.pre_upgrade_workflow_paused == Some(true) {
                "paused"
            } else {
                "running"
            }
        ));
        operation.updated_at = now_timestamp();
        self.save_operation(operation)
    }

    pub(crate) fn fail_agent_operation(
        &self,
        operation: &mut SourcePromotionOperation,
        stage: &str,
        error: &RefineError,
        recovery: &str,
    ) -> RefineResult<()> {
        operation.status = "failed".to_string();
        operation.stage = stage.to_string();
        operation.message = format!("Refine update failed during {stage}");
        operation.error = Some(error.to_string());
        operation.recovery = Some(recovery.to_string());
        operation.updated_at = now_timestamp();
        self.save_operation(operation)
    }

    fn acquire_agent_queue_lock(&self) -> RefineResult<fs::File> {
        let (file, path) = self.open_agent_queue_lock()?;
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!("failed to lock {}: {error}", path.display()))
        })?;
        Ok(file)
    }

    fn try_acquire_agent_queue_lock(&self) -> RefineResult<Option<fs::File>> {
        let (file, path) = self.open_agent_queue_lock()?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(file)),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(RefineError::Io(format!(
                "failed to lock {}: {error}",
                path.display()
            ))),
        }
    }

    fn open_agent_queue_lock(&self) -> RefineResult<(fs::File, std::path::PathBuf)> {
        fs::create_dir_all(&self.port_runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create source-upgrade runtime root {}: {error}",
                self.port_runtime_root.display()
            ))
        })?;
        let path = self.port_runtime_root.join(".source-upgrade-queue.lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!("failed to open {}: {error}", path.display()))
            })?;
        Ok((file, path))
    }

    pub(crate) fn reconcile_interrupted_agent(
        &self,
    ) -> RefineResult<Option<SourcePromotionOperation>> {
        let Some(observed) = self.load_operation()? else {
            return Ok(None);
        };
        let needs_launch_reconciliation = matches!(observed.status.as_str(), "queued" | "running")
            && observed.handoff_attempt.is_none()
            && !observed.stage.starts_with("restart_safe_handoff")
            && !matches!(
                observed.stage.as_str(),
                "build_candidate"
                    | "verify_idle"
                    | "prepare_service_registration"
                    | "stop_daemon"
                    | "activate_source"
                    | "restart_daemon"
                    | "verify_daemon"
                    | "post_restart_reconciliation"
                    | "complete"
            );
        if needs_launch_reconciliation {
            // `queue_agent_inner` holds this lock from durable reservation until
            // the supervised process receipt is persisted. A concurrent status
            // read must not reinterpret that intentional launch window as a
            // crashed launcher. If the launcher really dies, the OS releases the
            // file lock and the next observation performs normal recovery.
            let Some(_queue_guard) = self.try_acquire_agent_queue_lock()? else {
                return Ok(Some(observed));
            };
            return self.reconcile_interrupted_agent_locked();
        }
        self.reconcile_interrupted_agent_locked()
    }

    fn reconcile_interrupted_agent_locked(&self) -> RefineResult<Option<SourcePromotionOperation>> {
        let Some(mut operation) = self.load_operation()? else {
            return Ok(None);
        };
        if self
            .operation_registry()
            .status(&operation.id)
            .is_ok_and(|handle| matches!(handle.state, OperationState::Cancelling))
        {
            self.cancel_operation(&operation.id)?;
            return self.load_operation();
        }
        if operation.handoff_attempt.is_some()
            || operation.stage.starts_with("restart_safe_handoff")
        {
            self.reconcile_handoff_attempt(&mut operation, &HostRestartSafeHandoffLauncher)?;
            return Ok(Some(operation));
        }
        if !matches!(operation.status.as_str(), "queued" | "running")
            || matches!(
                operation.stage.as_str(),
                "build_candidate"
                    | "verify_idle"
                    | "prepare_service_registration"
                    | "stop_daemon"
                    | "activate_source"
                    | "restart_daemon"
                    | "verify_daemon"
                    | "post_restart_reconciliation"
                    | "complete"
            )
        {
            return Ok(Some(operation));
        }
        let registry = self.operation_registry();
        let registry_operation = registry.status(&operation.id).ok();
        let processes = self.correlated_processes(&operation.id)?;
        if registry_operation.as_ref().is_some_and(|registered| {
            matches!(
                registered.state,
                OperationState::Pending | OperationState::Running
            )
        }) && processes.len() == 1
        {
            if operation.agent_process_id.as_deref() != Some(processes[0].id.as_str()) {
                operation.agent_process_id = Some(processes[0].id.clone());
                operation.status = "running".to_string();
                operation.message = "Adopted the installed update Agent after restart".to_string();
                operation.updated_at = now_timestamp();
                self.save_operation(&operation)?;
            }
            return Ok(Some(operation));
        }
        let interruption = if processes.len() > 1 {
            format!(
                "Upgrade operation {} has {} correlated Agent processes; ambiguous ownership was rejected",
                operation.id,
                processes.len()
            )
        } else if let Some(error) = registry_operation
            .as_ref()
            .and_then(|registered| registered.error.as_ref())
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
        {
            error.to_string()
        } else {
            "No correlated supervised Agent survived durable reservation or launch".to_string()
        };
        if registry_operation.as_ref().is_some_and(|registered| {
            matches!(
                registered.state,
                OperationState::Pending | OperationState::Running
            )
        }) {
            let _ = registry.fail_with_error(
                &operation.id,
                json!({
                    "code": if processes.len() > 1 {
                        "source_upgrade_duplicate_agents"
                    } else {
                        "source_upgrade_interrupted_before_receipt"
                    },
                    "message": &interruption,
                    "retryable": true
                }),
            );
        }
        operation.primary_outcome = Some("interrupted".to_string());
        if let Err(error) = self.restore_workflow_admission(&mut operation) {
            return Ok(Some(self.load_operation()?.unwrap_or_else(|| {
                operation.error = Some(error.to_string());
                operation
            })));
        }
        operation.status = "interrupted".to_string();
        operation.stage = "agent_interrupted".to_string();
        operation.message =
            "Refine update Agent was interrupted before restart-safe handoff".to_string();
        operation.error = Some(format!(
            "{interruption}; no restart-safe promotion handoff is active"
        ));
        operation.recovery = Some(
            "Preserved work and workflow admission were reconciled; retry the update".to_string(),
        );
        operation.updated_at = now_timestamp();
        self.save_operation(&operation)?;
        Ok(Some(operation))
    }
}

fn validate_source_update(snapshot: &SourcePromotionSnapshot) -> RefineResult<()> {
    let mut source_only = snapshot.clone();
    source_only.active_work.clear();
    validate_promotion(&source_only)
}

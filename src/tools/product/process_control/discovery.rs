use super::*;

impl FileProcessControlService {
    pub(super) fn find_managed_process(
        &self,
        process_id: &str,
    ) -> RefineResult<Option<(FileProcessSupervisor, ManagedProcess)>> {
        for root in managed_process_roots(&self.runtime_root) {
            let supervisor = FileProcessSupervisor::new(root);
            match supervisor.inspect(process_id) {
                Ok(process) => return Ok(Some((supervisor, process))),
                Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    pub(super) fn managed_processes_for_session(
        &self,
        session_id: &str,
    ) -> RefineResult<Vec<(FileProcessSupervisor, ManagedProcess)>> {
        let mut matches = Vec::new();
        for root in managed_process_roots(&self.runtime_root) {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.list()? {
                if process_metadata(&process)
                    .get("session_id")
                    .and_then(Value::as_str)
                    == Some(session_id)
                {
                    matches.push((supervisor.clone(), process));
                }
            }
        }
        Ok(matches)
    }

    pub(super) fn managed_processes_for_execution(
        &self,
        execution_id: &str,
    ) -> RefineResult<Vec<(FileProcessSupervisor, ManagedProcess)>> {
        let mut matches = Vec::new();
        for root in managed_process_roots(&self.runtime_root) {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.list()? {
                if process_metadata(&process)
                    .get("execution_id")
                    .and_then(Value::as_str)
                    == Some(execution_id)
                {
                    matches.push((supervisor.clone(), process));
                }
            }
        }
        Ok(matches)
    }

    pub(super) fn managed_processes_for_goal(
        &self,
        goal_id: &str,
    ) -> RefineResult<Vec<(FileProcessSupervisor, ManagedProcess)>> {
        let mut matches = Vec::new();
        for root in managed_process_roots(&self.runtime_root) {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.list()? {
                if is_agent_process(&process)
                    && process_metadata(&process)
                        .get("goal_id")
                        .and_then(Value::as_str)
                        == Some(goal_id)
                {
                    matches.push((supervisor.clone(), process));
                }
            }
        }
        matches.sort_by(|a, b| a.1.id.cmp(&b.1.id));
        Ok(matches)
    }

    pub(super) fn recoverable_workflow_terminations(
        &self,
        goal_id: &str,
        claim_id: &str,
        execution_id: &str,
    ) -> RefineResult<Vec<RecoveredWorkflowTermination>> {
        let directory = self.runtime_root.join("process-stop-outcomes");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to inspect process-stop recovery evidence {}: {error}",
                    directory.display()
                )));
            }
        };
        let mut recovered = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect process-stop recovery entry: {error}"
                ))
            })?;
            if entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("json")
            {
                continue;
            }
            let bytes = fs::read(entry.path()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read process-stop recovery evidence {}: {error}",
                    entry.path().display()
                ))
            })?;
            let receipt: Value = serde_json::from_slice(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse process-stop recovery evidence {}: {error}",
                    entry.path().display()
                ))
            })?;
            if receipt.get("goal_id").and_then(Value::as_str) != Some(goal_id)
                || receipt.get("confirmed_exit").and_then(Value::as_bool) != Some(true)
                || receipt
                    .get("registry_cleanup_completed")
                    .and_then(Value::as_bool)
                    != Some(true)
                || receipt
                    .get("identity_cleanup_completed")
                    .and_then(Value::as_bool)
                    != Some(true)
                || receipt.get("goal_cancelled").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            let Some(workflow) = receipt.get("workflow") else {
                continue;
            };
            if workflow.get("claim_id").and_then(Value::as_str) != Some(claim_id)
                || workflow.get("execution_id").and_then(Value::as_str) != Some(execution_id)
            {
                continue;
            }
            let process_id = workflow
                .get("process_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    RefineError::Serialization(format!(
                        "process-stop recovery evidence {} has no workflow process id",
                        entry.path().display()
                    ))
                })?
                .to_string();
            let termination = serde_json::from_value::<ConfirmedProcessExit>(
                receipt.get("termination").cloned().ok_or_else(|| {
                    RefineError::Serialization(format!(
                        "process-stop recovery evidence {} has no termination outcome",
                        entry.path().display()
                    ))
                })?,
            )
            .map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse confirmed process exit {}: {error}",
                    entry.path().display()
                ))
            })?;
            recovered.push(RecoveredWorkflowTermination {
                ownership: WorkflowGoalOwnership {
                    process_id,
                    claim_id: claim_id.to_string(),
                    execution_id: Some(execution_id.to_string()),
                    round_idx: workflow
                        .get("round_idx")
                        .and_then(Value::as_u64)
                        .and_then(|value| usize::try_from(value).ok()),
                },
                termination,
            });
        }
        recovered.sort_by(|a, b| a.ownership.process_id.cmp(&b.ownership.process_id));
        Ok(recovered)
    }

    pub(super) fn resolve_refine_dir(&self) -> RefineResult<PathBuf> {
        if let Some(refine_dir) = &self.refine_dir {
            return Ok(refine_dir.clone());
        }
        let registry = FileProjectRegistryService::new(&self.runtime_root, None).load()?;
        let target_root = registry
            .active_app
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                RefineError::Degraded(
                    "cannot stop a Goal-linked agent because the runtime has no active app; process and Goal state were left unchanged"
                        .to_string(),
                )
            })?;
        refine_dir_for_target_root(Path::new(&target_root))
    }
}

use super::*;

#[derive(Clone, Debug)]
pub struct FileReleaseService {
    pub repo_root: PathBuf,
    pub runtime_root: PathBuf,
}

impl FileReleaseService {
    pub fn new(repo_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn plan(&self, bump: ReleaseBump) -> RefineResult<ReleasePlan> {
        ShellReleaseHost::new(&self.repo_root).plan(bump)
    }

    pub fn status(&self) -> RefineResult<Value> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let mut operations = Vec::new();
        for operation in registry
            .recover()?
            .into_iter()
            .filter(|operation| operation.owner.starts_with("release:"))
        {
            let operation_id = operation.id.clone();
            let (logs, _, _) = registry.page_logs(&operation.id, 100, 0)?;
            let mut value = operation_json(operation);
            value["logs"] = json!(logs);
            let preparation_id = value["result"]["preparation_id"]
                .as_str()
                .map(ToString::to_string)
                .or_else(|| match self.load_request(&operation_id).ok()? {
                    ReleaseRequest::Prepare { .. } => Some(operation_id.clone()),
                    ReleaseRequest::Publish { preparation_id } => Some(preparation_id),
                });
            if let Some(preparation_id) = preparation_id
                && let Ok(preparation) = self.preparation_status(&preparation_id)
            {
                value["preparation"] = preparation;
            }
            operations.push(value);
        }
        Ok(json!({"operations": operations}))
    }

    pub fn start_prepare(&self, bump: ReleaseBump) -> RefineResult<OperationHandle> {
        let plan = self.plan(bump)?;
        let operation = self.register_request(ReleaseRequest::Prepare {
            plan: Box::new(plan),
            goal_id: None,
        })?;
        self.spawn(operation.id.clone());
        Ok(operation)
    }

    pub fn prepare_blocking(&self, bump: ReleaseBump) -> RefineResult<OperationHandle> {
        let plan = self.plan(bump)?;
        let operation = self.register_request(ReleaseRequest::Prepare {
            plan: Box::new(plan),
            goal_id: None,
        })?;
        self.run_or_fail(&operation.id)?;
        FileOperationRegistry::new(&self.runtime_root).status(&operation.id)
    }

    pub fn start_publish(
        &self,
        preparation_id: &str,
        confirmed: bool,
    ) -> RefineResult<OperationHandle> {
        self.require_confirmation(confirmed)?;
        self.resolve_trusted_preparation(preparation_id)?;
        let operation = self.register_request(ReleaseRequest::Publish {
            preparation_id: preparation_id.to_string(),
        })?;
        self.spawn(operation.id.clone());
        Ok(operation)
    }

    pub fn publish_blocking(
        &self,
        preparation_id: &str,
        confirmed: bool,
    ) -> RefineResult<OperationHandle> {
        self.require_confirmation(confirmed)?;
        self.resolve_trusted_preparation(preparation_id)?;
        let operation = self.register_request(ReleaseRequest::Publish {
            preparation_id: preparation_id.to_string(),
        })?;
        self.run_or_fail(&operation.id)?;
        FileOperationRegistry::new(&self.runtime_root).status(&operation.id)
    }

    pub fn retry(&self, operation_id: &str, confirmed: bool) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let prior = registry.status(operation_id)?;
        if !prior.owner.starts_with("release:") {
            return Err(RefineError::InvalidInput(
                "only release operations can be retried here".to_string(),
            ));
        }
        if matches!(
            prior.state,
            OperationState::Running | OperationState::Pending
        ) {
            return Err(RefineError::Conflict(format!(
                "release operation {operation_id} is still active"
            )));
        }
        let request = self.load_request(operation_id)?;
        if matches!(request, ReleaseRequest::Publish { .. }) {
            self.require_confirmation(confirmed)?;
        }
        let operation = self.register_request(request)?;
        self.spawn(operation.id.clone());
        Ok(operation)
    }

    pub fn run_with_host(
        &self,
        operation_id: &str,
        host: &mut dyn ReleaseHost,
    ) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let request = self.load_request(operation_id)?;
        let result = match request {
            ReleaseRequest::Prepare { plan, goal_id } => {
                let goal_id = self.queue_preparation_goal(operation_id, &plan, goal_id)?;
                json!({
                    "preparation_id": operation_id,
                    "goal_id": goal_id,
                    "plan": plan,
                    "review_required": true
                })
            }
            ReleaseRequest::Publish { preparation_id } => {
                let preparation = self.resolve_trusted_preparation(&preparation_id)?;
                let published = run_publication(&registry, operation_id, host, &preparation)?;
                json!({
                    "preparation_id": preparation_id,
                    "goal_id": preparation.goal_id,
                    "published": published
                })
            }
        };
        registry.finish_with_result(operation_id, OperationState::Succeeded, result)
    }

    fn queue_preparation_goal(
        &self,
        operation_id: &str,
        plan: &ReleasePlan,
        existing_goal_id: Option<String>,
    ) -> RefineResult<String> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        let work_items = self.work_items()?;
        if let Some(goal_id) = existing_goal_id {
            let goal = work_items.show_goal_summary(&goal_id)?;
            match goal.goal.status {
                GoalStatus::Failed => {
                    stage(
                        &registry,
                        operation_id,
                        "queue_goal",
                        "Re-queueing the linked release preparation Goal",
                        Some(&goal_id),
                    )?;
                    work_items.transition_goal_status(&goal_id, GoalStatus::Todo)?;
                    return Ok(goal_id);
                }
                GoalStatus::Backlog | GoalStatus::Todo => {
                    work_items.start_goal_workflow(&goal_id)?;
                    return Ok(goal_id);
                }
                _ => {
                    return Err(RefineError::Conflict(format!(
                        "release preparation Goal {goal_id} is {}; retry it through its normal workflow",
                        goal.goal.status.as_str()
                    )));
                }
            }
        }

        stage(
            &registry,
            operation_id,
            "queue_goal",
            "Creating a normal Goal for agent-operated release preparation",
            None,
        )?;
        let name = format!("Prepare {}", plan.proposed_tag);
        let goal = work_items.create_goal_summary(&name, None)?;
        let goal_id = goal.goal.id.clone();
        let prompt = release_goal_prompt(plan);
        if let Err(error) = work_items
            .append_goal_round_summary(&goal_id, "Release workflow", &prompt)
            .and_then(|_| work_items.start_goal_workflow(&goal_id))
        {
            let _ = work_items.delete_goal_record(&goal_id);
            return Err(error);
        }
        self.write_request(
            operation_id,
            &ReleaseRequest::Prepare {
                plan: Box::new(plan.clone()),
                goal_id: Some(goal_id.clone()),
            },
        )?;
        stage(
            &registry,
            operation_id,
            "queued",
            "Release preparation Goal queued for the configured agent",
            Some(&goal_id),
        )?;
        Ok(goal_id)
    }

    fn preparation_status(&self, preparation_id: &str) -> RefineResult<Value> {
        let request = self.load_request(preparation_id)?;
        let ReleaseRequest::Prepare { plan, goal_id } = request else {
            return Err(RefineError::InvalidInput(format!(
                "operation {preparation_id} is not a release preparation"
            )));
        };
        let Some(goal_id) = goal_id else {
            return Ok(json!({"preparation_id": preparation_id, "plan": plan}));
        };
        let detail = self.work_items()?.show_goal_detail(&goal_id)?;
        let status = detail.get("status").cloned().unwrap_or(Value::Null);
        let branch = detail.get("branch_name").cloned().unwrap_or(Value::Null);
        let candidate_commit = detail
            .get("candidate_commit")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(json!({
            "preparation_id": preparation_id,
            "goal_id": goal_id,
            "plan": plan,
            "status": status,
            "branch": branch,
            "candidate_commit": candidate_commit,
            "rounds": detail.get("rounds").cloned().unwrap_or_else(|| json!([])),
            "review_url": format!("#/goals/{goal_id}"),
            "publishable": status == "done" && !candidate_commit.is_null()
        }))
    }

    fn resolve_trusted_preparation(
        &self,
        preparation_id: &str,
    ) -> RefineResult<TrustedPreparation> {
        let request = self.load_request(preparation_id)?;
        let ReleaseRequest::Prepare { plan, goal_id } = request else {
            return Err(RefineError::InvalidInput(
                "publication requires a persisted preparation operation id".to_string(),
            ));
        };
        let goal_id = goal_id.ok_or_else(|| {
            RefineError::Conflict("release preparation has not created its Goal yet".to_string())
        })?;
        let work_items = self.work_items()?;
        let goal = work_items.show_goal_summary(&goal_id)?;
        if goal.goal.status != GoalStatus::Done {
            return Err(RefineError::Conflict(format!(
                "release preparation Goal {goal_id} must be approved and done before publication; it is {}",
                goal.goal.status.as_str()
            )));
        }
        let detail = work_items.show_goal_detail(&goal_id)?;
        let required = |field: &str| {
            detail
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .ok_or_else(|| {
                    RefineError::Conflict(format!(
                        "release preparation Goal {goal_id} has no {field}"
                    ))
                })
        };
        Ok(TrustedPreparation {
            preparation_id: preparation_id.to_string(),
            goal_id: goal_id.clone(),
            version: plan.proposed_version,
            tag: plan.proposed_tag,
            branch: required("branch_name")?,
            target_branch: required("target_branch")?,
            candidate_commit: required("candidate_commit")?,
            release_notes: "RELEASE_NOTES.md".to_string(),
        })
    }

    pub(crate) fn work_items(&self) -> RefineResult<FileWorkItemService> {
        let refine_dir = prepare_refine_dir(&self.repo_root)?;
        Ok(FileWorkItemService::with_projection_cache(
            refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        ))
    }

    fn require_confirmation(&self, confirmed: bool) -> RefineResult<()> {
        if confirmed {
            Ok(())
        } else {
            Err(RefineError::InvalidInput(
                "publishing is externally mutating and requires confirmed=true".to_string(),
            ))
        }
    }

    pub(crate) fn register_request(
        &self,
        request: ReleaseRequest,
    ) -> RefineResult<OperationHandle> {
        let registry = FileOperationRegistry::new(&self.runtime_root);
        if registry.recover()?.iter().any(|operation| {
            operation.owner.starts_with("release:")
                && matches!(
                    operation.state,
                    OperationState::Running | OperationState::Pending
                )
        }) {
            return Err(RefineError::Conflict(
                "another release operation is already active".to_string(),
            ));
        }
        let owner = match request {
            ReleaseRequest::Prepare { .. } => "release:prepare",
            ReleaseRequest::Publish { .. } => "release:publish",
        };
        let operation = registry.register(owner)?;
        self.write_request(&operation.id, &request)?;
        Ok(operation)
    }

    fn request_path(&self, operation_id: &str) -> PathBuf {
        self.runtime_root
            .join(RELEASE_REQUESTS_DIR)
            .join(format!("{operation_id}.json"))
    }

    fn write_request(&self, operation_id: &str, request: &ReleaseRequest) -> RefineResult<()> {
        let path = self.request_path(operation_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_error("create release request directory"))?;
        }
        fs::write(
            &path,
            serde_json::to_vec_pretty(request).map_err(|error| {
                RefineError::Serialization(format!("failed to encode release request: {error}"))
            })?,
        )
        .map_err(io_error("write release request"))
    }

    fn load_request(&self, operation_id: &str) -> RefineResult<ReleaseRequest> {
        let path = self.request_path(operation_id);
        let bytes = fs::read(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RefineError::NotFound(format!("release request {operation_id} was not found"))
            } else {
                RefineError::Io(format!("failed to read {}: {error}", path.display()))
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
        })
    }

    fn run_or_fail(&self, operation_id: &str) -> RefineResult<()> {
        let mut host = ShellReleaseHost::new(&self.repo_root);
        if let Err(error) = self.run_with_host(operation_id, &mut host) {
            let registry = FileOperationRegistry::new(&self.runtime_root);
            let _ = registry.fail_with_error(
                operation_id,
                json!({"code": "release_operation_failed", "message": error.to_string()}),
            );
            return Err(error);
        }
        Ok(())
    }

    fn spawn(&self, operation_id: String) {
        let service = self.clone();
        std::thread::spawn(move || {
            let _ = service.run_or_fail(&operation_id);
        });
    }
}

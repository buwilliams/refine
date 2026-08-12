use super::*;

impl FileWorkItemService {
    pub fn update_goal_metadata_summary(
        &self,
        goal_id: &str,
        name: Option<&str>,
        priority: Option<&str>,
        reporter: Option<&str>,
        assignee: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditMetadata)?;
        let reporter = reporter.map(Self::validate_goal_reporter).transpose()?;

        let mutate = || {
            let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
            let object = value.as_object_mut().ok_or_else(|| {
                RefineError::Serialization(format!(
                    "Goal {} is not a JSON object",
                    goal_path.display()
                ))
            })?;
            if let Some(name) = name {
                let name = name.trim();
                if name.is_empty() {
                    return Err(RefineError::InvalidInput(
                        "Goal name cannot be empty".to_string(),
                    ));
                }
                object.insert("name".to_string(), Value::String(name.to_string()));
            }
            if let Some(priority) = priority {
                let Some(priority) = GoalPriority::parse_wire(priority) else {
                    return Err(RefineError::InvalidInput(
                        "priority must be one of low, medium, or high".to_string(),
                    ));
                };
                object.insert(
                    "priority".to_string(),
                    Value::String(priority.as_str().to_string()),
                );
            }
            if let Some(reporter) = reporter {
                object.insert(
                    "reporter".to_string(),
                    if reporter.is_empty() {
                        Value::Null
                    } else {
                        Value::String(reporter.to_string())
                    },
                );
            }
            object.insert("updated".to_string(), Value::String(now_timestamp()));
            write_json_atomically(&goal_path, &value)
        };
        if let Some(reporter) = reporter {
            self.with_goal_reporter_registered(reporter, mutate)?;
        } else {
            mutate()?;
        }
        if let Some(assignee) = assignee {
            self.set_latest_round_assignee(goal_id, assignee)?;
        }
        self.show_goal_summary(goal_id)
    }

    pub(super) fn validate_goal_assignee(assignee: &str) -> RefineResult<&str> {
        let assignee = assignee.trim();
        if !assignee.is_empty() && !valid_reporter_name(assignee) {
            return Err(RefineError::InvalidInput(
                "invalid assignee name".to_string(),
            ));
        }
        Ok(assignee)
    }

    pub(super) fn validate_goal_reporter(reporter: &str) -> RefineResult<&str> {
        let reporter = reporter.trim();
        if !reporter.is_empty() && !valid_reporter_name(reporter) {
            return Err(RefineError::InvalidInput(
                "invalid reporter name".to_string(),
            ));
        }
        Ok(reporter)
    }

    pub fn update_goal_assignee_summary(
        &self,
        goal_id: &str,
        assignee: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditMetadata)?;
        self.set_latest_round_assignee(goal_id, assignee)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_goal_reporter_summary(
        &self,
        goal_id: &str,
        reporter: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditMetadata)?;
        self.set_goal_reporter_unchecked(goal_id, reporter)?;
        self.show_goal_summary(goal_id)
    }

    pub fn add_goal_note_summary(
        &self,
        goal_id: &str,
        author: &str,
        body: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditNotes)?;
        let body = body.trim();
        if body.is_empty() {
            return Err(RefineError::InvalidInput(
                "note body cannot be empty".to_string(),
            ));
        }

        let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let now = now_timestamp();
        let mut note = Map::new();
        note.insert("id".to_string(), Value::String(new_ulid_like()));
        note.insert(
            "author".to_string(),
            Value::String(author.trim().to_string()),
        );
        note.insert("body".to_string(), Value::String(body.to_string()));
        note.insert("created".to_string(), Value::String(now.clone()));
        note.insert("updated".to_string(), Value::String(now.clone()));
        match object.get_mut("notes") {
            Some(Value::Array(notes)) => notes.push(Value::Object(note)),
            _ => {
                object.insert("notes".to_string(), Value::Array(vec![Value::Object(note)]));
            }
        }
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn replace_goal_notes_summary(
        &self,
        goal_id: &str,
        notes: &[Value],
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::EditNotes)?;

        let now = now_timestamp();
        let mut next_notes = Vec::new();
        for note in notes {
            let object = note.as_object().ok_or_else(|| {
                RefineError::InvalidInput("notes must be an array of objects".to_string())
            })?;
            let body = object
                .get("body")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim();
            if body.is_empty() {
                return Err(RefineError::InvalidInput(
                    "note body cannot be empty".to_string(),
                ));
            }
            let mut cleaned = Map::new();
            cleaned.insert(
                "id".to_string(),
                Value::String(
                    object
                        .get("id")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(new_ulid_like),
                ),
            );
            cleaned.insert(
                "author".to_string(),
                Value::String(
                    object
                        .get("author")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                ),
            );
            cleaned.insert("body".to_string(), Value::String(body.to_string()));
            cleaned.insert(
                "created".to_string(),
                Value::String(
                    object
                        .get("created")
                        .and_then(|value| value.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| now.clone()),
                ),
            );
            cleaned.insert("updated".to_string(), Value::String(now.clone()));
            next_notes.push(Value::Object(cleaned));
        }

        let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert("notes".to_string(), Value::Array(next_notes));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn append_goal_round_summary(
        &self,
        goal_id: &str,
        reporter: &str,
        prompt: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        self.append_goal_round_summary_with_assignee(goal_id, reporter, None, prompt)
    }

    pub fn append_goal_round_summary_with_assignee(
        &self,
        goal_id: &str,
        reporter: &str,
        assignee: Option<&str>,
        prompt: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        self.append_goal_round_summary_with_requeue(goal_id, reporter, assignee, prompt)
    }

    fn append_goal_round_summary_with_requeue(
        &self,
        goal_id: &str,
        reporter: &str,
        assignee: Option<&str>,
        prompt: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let reporter = Self::validate_goal_reporter(reporter)?;
        let assignee = assignee
            .map(Self::validate_goal_assignee)
            .transpose()?
            .filter(|value| !value.is_empty())
            .unwrap_or(reporter);
        let prompt = prompt.trim();
        if reporter.is_empty() || prompt.is_empty() {
            return Err(RefineError::InvalidInput(
                "round reporter and prompt are required".to_string(),
            ));
        }

        self.with_goal_reporter_registered(reporter, || {
            let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
            let current = self.show_goal_summary(goal_id)?;
            self.ensure_goal_owned(&current)?;
            validate_goal_operation(&current.goal.status, &GoalOperation::SubmitNewRound)?;
            let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
            let object = value.as_object_mut().ok_or_else(|| {
                RefineError::Serialization(format!(
                    "Goal {} is not a JSON object",
                    goal_path.display()
                ))
            })?;
            let round = new_round_value(reporter, assignee, prompt);
            match object.get_mut("rounds") {
                Some(Value::Array(rounds)) => rounds.push(round),
                _ => {
                    object.insert("rounds".to_string(), Value::Array(vec![round]));
                }
            }
            if matches!(current.goal.status, GoalStatus::Review | GoalStatus::Failed) {
                object.insert(
                    "status".to_string(),
                    Value::String(GoalStatus::Todo.as_str().to_string()),
                );
            }
            object.insert("updated".to_string(), Value::String(now_timestamp()));
            write_json_atomically(&goal_path, &value)
        })?;
        self.show_goal_summary(goal_id)
    }

    pub fn edit_latest_goal_round_summary(
        &self,
        goal_id: &str,
        reporter: Option<&str>,
        assignee: Option<&str>,
        prompt: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let reporter = reporter.map(Self::validate_goal_reporter).transpose()?;
        let assignee = assignee.map(Self::validate_goal_assignee).transpose()?;
        let mutate = || {
            let current = self.show_goal_summary(goal_id)?;
            validate_goal_operation(&current.goal.status, &GoalOperation::EditLatestRound)?;
            let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
            let object = value.as_object_mut().ok_or_else(|| {
                RefineError::Serialization(format!(
                    "Goal {} is not a JSON object",
                    goal_path.display()
                ))
            })?;
            let rounds = object
                .get_mut("rounds")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
            let latest = rounds
                .iter_mut()
                .rev()
                .find(|round| round.is_object())
                .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
            let latest = latest.as_object_mut().ok_or_else(|| {
                RefineError::Serialization(format!(
                    "latest round for Goal {goal_id} is not a JSON object"
                ))
            })?;
            if let Some(reporter) = reporter {
                latest.insert("reporter".to_string(), Value::String(reporter.to_string()));
            }
            if let Some(assignee) = assignee {
                latest.insert(
                    "assignee".to_string(),
                    if assignee.is_empty() {
                        Value::Null
                    } else {
                        Value::String(assignee.to_string())
                    },
                );
            }
            if let Some(prompt) = prompt {
                latest.insert(
                    "prompt".to_string(),
                    Value::String(prompt.trim().to_string()),
                );
            }
            let now = now_timestamp();
            latest.insert("updated".to_string(), Value::String(now.clone()));
            object.insert("updated".to_string(), Value::String(now));
            write_json_atomically(&goal_path, &value)
        };
        if let Some(reporter) = reporter {
            self.with_goal_reporter_registered(reporter, mutate)?;
        } else {
            mutate()?;
        }
        self.show_goal_summary(goal_id)
    }

    pub fn update_goal_branch_name(
        &self,
        goal_id: &str,
        branch_name: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (_goal_lock, goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        match branch_name.map(str::trim).filter(|value| !value.is_empty()) {
            Some(branch) => {
                object.insert("branch_name".to_string(), Value::String(branch.to_string()));
            }
            None => {
                object.insert("branch_name".to_string(), Value::Null);
            }
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_goal_git_refs(
        &self,
        goal_id: &str,
        branch_name: &str,
        target_branch: &str,
        base_commit: &str,
        candidate_commit: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (_goal_lock, goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        for (key, raw) in [
            ("branch_name", branch_name),
            ("target_branch", target_branch),
            ("base_commit", base_commit),
        ] {
            let raw = raw.trim();
            if raw.is_empty() {
                return Err(RefineError::InvalidInput(format!("{key} is required")));
            }
            object.insert(key.to_string(), Value::String(raw.to_string()));
        }
        object.insert(
            "candidate_commit".to_string(),
            candidate_commit
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| Value::String(value.to_string()))
                .unwrap_or(Value::Null),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_goal_candidate_commit(
        &self,
        goal_id: &str,
        candidate_commit: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let candidate_commit = candidate_commit.trim();
        if candidate_commit.is_empty() {
            return Err(RefineError::InvalidInput(
                "candidate commit is required".to_string(),
            ));
        }
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (_goal_lock, goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "candidate_commit".to_string(),
            Value::String(candidate_commit.to_string()),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_latest_goal_round_evaluation_summary(
        &self,
        goal_id: &str,
        evaluation: &Value,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        let round_idx = current
            .goal
            .round_count
            .checked_sub(1)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        self.update_goal_round_evaluation_summary(goal_id, round_idx, evaluation)
    }

    pub fn update_goal_round_evaluation_summary(
        &self,
        goal_id: &str,
        round_idx: usize,
        evaluation: &Value,
    ) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let fields = evaluation.as_object().ok_or_else(|| {
            RefineError::InvalidInput("round evaluation body must be a JSON object".to_string())
        })?;
        if fields.contains_key("implementation_plan") {
            return Err(RefineError::InvalidInput(
                "implementation_plan is Workflow-owned evidence and can only be changed through its fenced planning authority"
                    .to_string(),
            ));
        }

        let (_goal_lock, goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let round = rounds.get_mut(round_idx).ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} has no round {}", round_idx + 1))
        })?;
        let round = round.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "round {} for Goal {goal_id} is not a JSON object",
                round_idx + 1
            ))
        })?;
        for key in [
            "agent_context",
            "guidance_decision",
            "rule_state",
            "meta_rule_state",
            "product_state",
            "constitution_state",
            "governance_message",
            "governance_details",
            "governance_checked_at",
            "governance_rule_actions",
            "quality_state",
            "quality_message",
            "quality_details",
            "quality_checked_at",
            "workflow_quality_timing",
            "workflow_git_remote",
            "workflow_integration",
            "workflow_reconciliation",
            "workflow_recovery",
            "failure_category",
            "failure_message",
            "failure_at",
        ] {
            if let Some(value) = fields.get(key) {
                round.insert(key.to_string(), value.clone());
            }
        }
        let now = now_timestamp();
        round.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn update_latest_goal_round_implementation_report(
        &self,
        goal_id: &str,
        report: &str,
    ) -> RefineResult<GoalSummaryProjection> {
        let report = report.trim();
        if report.is_empty() {
            return Err(RefineError::InvalidInput(
                "implementation report cannot be empty".to_string(),
            ));
        }
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (_goal_lock, goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        let rounds = object
            .get_mut("rounds")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let latest = rounds
            .iter_mut()
            .rev()
            .find_map(Value::as_object_mut)
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let now = now_timestamp();
        latest.insert(
            "implementation_report".to_string(),
            Value::String(report.to_string()),
        );
        latest.insert(
            "implementation_reported_at".to_string(),
            Value::String(now.clone()),
        );
        latest.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)?;
        self.show_goal_summary(goal_id)
    }

    pub fn delete_goal_record(&self, goal_id: &str) -> RefineResult<()> {
        let _goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::Delete)?;
        let goal_path = self.refine_dir.join(&current.goal.json_path);
        fs::remove_file(&goal_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to delete Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        // A deleted record cannot be re-projected, so the removal is recorded
        // explicitly rather than derived from the file that no longer exists.
        if let Err(error) = ActiveGoalIndex::forget_goal(&self.refine_dir, &current.goal.id) {
            eprintln!(
                "refine: active Goal index still lists deleted Goal {}: {error}",
                current.goal.id
            );
        }
        if let Some(parent) = goal_path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }
}

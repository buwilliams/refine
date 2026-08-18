use super::*;

impl FileWorkItemService {
    pub fn create_feature_summary(
        &self,
        name: &str,
        id: Option<&str>,
        description: Option<&str>,
        reporter: Option<&str>,
        assignee: Option<&str>,
    ) -> RefineResult<FeatureSummaryProjection> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RefineError::InvalidInput(
                "Feature name is required".to_string(),
            ));
        }
        let feature_id = id
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_ulid_like);
        if feature_id.len() < 3 {
            return Err(RefineError::InvalidInput(
                "Feature id must be at least three characters".to_string(),
            ));
        }

        let feature_path = feature_json_path(&self.refine_dir, &feature_id);
        if feature_path.exists() {
            return Err(RefineError::Conflict(format!(
                "Feature {feature_id} already exists"
            )));
        }
        let node_id = self.active_node_id()?;
        let now = now_timestamp();
        let mut object = Map::new();
        object.insert("id".to_string(), Value::String(feature_id.clone()));
        object.insert("name".to_string(), Value::String(name.to_string()));
        object.insert(
            "description".to_string(),
            Value::String(description.unwrap_or("").trim().to_string()),
        );
        object.insert(
            "reporter".to_string(),
            Value::String(reporter.unwrap_or("").trim().to_string()),
        );
        object.insert(
            "assignee".to_string(),
            Value::String(assignee.or(reporter).unwrap_or("").trim().to_string()),
        );
        object.insert("node_id".to_string(), Value::String(node_id));
        object.insert("created".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&feature_path, &Value::Object(object))?;
        self.show_feature_summary(&feature_id)
    }

    pub fn show_feature_summary(&self, feature_id: &str) -> RefineResult<FeatureSummaryProjection> {
        let snapshot = self.projection_snapshot()?;
        snapshot.features.get(feature_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!(
                "Feature {feature_id} was not found in refine state"
            ))
        })
    }

    pub fn update_feature_metadata_summary(
        &self,
        feature_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        reporter: Option<&str>,
        assignee: Option<&str>,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let feature_path = feature_json_path(&self.refine_dir, feature_id);
        let bytes = fs::read(&feature_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Feature {}: {error}",
                feature_path.display()
            ))
        })?;
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Feature {}: {error}",
                feature_path.display()
            ))
        })?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "Feature {} is not a JSON object",
                feature_path.display()
            ))
        })?;
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() {
                return Err(RefineError::InvalidInput(
                    "Feature name cannot be empty".to_string(),
                ));
            }
            object.insert("name".to_string(), Value::String(name.to_string()));
        }
        if let Some(description) = description {
            object.insert(
                "description".to_string(),
                Value::String(description.trim().to_string()),
            );
        }
        if let Some(reporter) = reporter {
            object.insert(
                "reporter".to_string(),
                Value::String(reporter.trim().to_string()),
            );
        }
        if let Some(assignee) = assignee {
            let assignee = assignee.trim();
            if !assignee.is_empty() && !valid_reporter_name(assignee) {
                return Err(RefineError::InvalidInput(
                    "invalid assignee name".to_string(),
                ));
            }
            object.insert(
                "assignee".to_string(),
                if assignee.is_empty() {
                    Value::Null
                } else {
                    Value::String(assignee.to_string())
                },
            );
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&feature_path, &value)?;
        self.show_feature_summary(feature_id)
    }

    pub fn list_feature_summaries(&self) -> RefineResult<Vec<FeatureSummaryProjection>> {
        let snapshot = self.projection_snapshot()?;
        Ok(snapshot.features.values().cloned().collect())
    }

    pub fn assign_goal_to_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::AssignToFeature)?;
        let old_feature_id = current_goal.goal.feature_id.clone();
        self.set_goal_feature_membership(goal_id, Some(feature_id), None)?;
        if let Some(old_feature_id) = old_feature_id
            && old_feature_id != feature_id
        {
            let _ = self.compact_feature_orders(&old_feature_id);
        }
        self.show_feature_summary(feature_id)
    }

    pub fn remove_goal_from_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        if current_goal.goal.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::RemoveFromFeature)?;
        self.set_goal_feature_membership(goal_id, None, None)?;
        self.compact_feature_orders(feature_id)?;
        self.show_feature_summary(feature_id)
    }

    pub fn order_goal_in_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        if current_goal.goal.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::ReorderInFeature)?;
        if is_ordered_feature_goal(current_goal.goal.feature_order) {
            return self.show_feature_summary(feature_id);
        }
        let next_order = self.next_feature_order(feature_id)?;
        self.set_goal_feature_membership(goal_id, Some(feature_id), Some(next_order))?;
        self.show_feature_summary(feature_id)
    }

    pub fn unorder_goal_in_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        if current_goal.goal.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::ReorderInFeature)?;
        if !is_ordered_feature_goal(current_goal.goal.feature_order) {
            return self.show_feature_summary(feature_id);
        }
        self.set_goal_feature_membership(goal_id, Some(feature_id), None)?;
        self.compact_feature_orders(feature_id)?;
        self.show_feature_summary(feature_id)
    }

    pub fn reorder_goal_in_feature(
        &self,
        feature_id: &str,
        goal_id: &str,
        order: i64,
    ) -> RefineResult<FeatureSummaryProjection> {
        if order < 1 {
            return Err(RefineError::InvalidInput(
                "feature order must be at least 1".to_string(),
            ));
        }
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let current_goal = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current_goal)?;
        if current_goal.goal.feature_id.as_deref() != Some(feature_id) {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} is not assigned to Feature {feature_id}"
            )));
        }
        validate_goal_operation(&current_goal.goal.status, &GoalOperation::ReorderInFeature)?;
        let mut goals: Vec<_> = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .filter(|goal| is_ordered_feature_goal(goal.goal.feature_order))
            .collect();
        goals.sort_by(|a, b| {
            compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        let Some(current_index) = goals.iter().position(|goal| goal.goal.id == goal_id) else {
            return Err(RefineError::NotFound(format!(
                "Goal {goal_id} was not found in Feature {feature_id}"
            )));
        };
        let goal = goals.remove(current_index);
        let insert_index = usize::min(order as usize - 1, goals.len());
        goals.insert(insert_index, goal);
        for (idx, goal) in goals.iter().enumerate() {
            self.set_goal_feature_membership(
                &goal.goal.id,
                Some(feature_id),
                Some(idx as i64 + 1),
            )?;
        }
        self.show_feature_summary(feature_id)
    }

    pub fn order_goals_in_feature(
        &self,
        feature_id: &str,
        goal_ids: &[String],
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        for goal_id in goal_ids {
            self.order_goal_in_feature(feature_id, goal_id)?;
        }
        self.show_feature_summary(feature_id)
    }

    pub fn move_feature_workflow(
        &self,
        feature_id: &str,
        target: GoalStatus,
    ) -> RefineResult<FeatureSummaryProjection> {
        if !matches!(target, GoalStatus::Backlog | GoalStatus::Todo) {
            return Err(RefineError::InvalidInput(
                "Feature workflow target must be backlog or todo".to_string(),
            ));
        }
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let mut goals: Vec<_> = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .collect();
        goals.sort_by(|a, b| {
            compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        for goal in goals {
            if is_feature_protected_status(&goal.goal.status) {
                continue;
            }
            self.set_goal_status_unchecked(&goal.goal.id, &target)?;
        }
        self.show_feature_summary(feature_id)
    }

    pub fn cancel_feature_summary(
        &self,
        feature_id: &str,
    ) -> RefineResult<FeatureSummaryProjection> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let goals = self.feature_goal_summaries(feature_id)?;
        validate_feature_operation(
            &goals
                .iter()
                .map(|goal| goal.goal.status.clone())
                .collect::<Vec<_>>(),
            &FeatureOperation::CancelFeature,
        )?;
        for goal in goals {
            if is_feature_cancel_status(&goal.goal.status) {
                self.cancel_goal_summary(&goal.goal.id)?;
            }
        }
        self.show_feature_summary(feature_id)
    }

    pub fn delete_feature_record(&self, feature_id: &str) -> RefineResult<()> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let goals = self.feature_goal_summaries(feature_id)?;
        validate_feature_operation(
            &goals
                .iter()
                .map(|goal| goal.goal.status.clone())
                .collect::<Vec<_>>(),
            &FeatureOperation::DeleteFeature,
        )?;
        for goal in goals {
            self.delete_goal_record(&goal.goal.id)?;
        }
        let feature_path = feature_json_path(&self.refine_dir, feature_id);
        fs::remove_file(&feature_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to delete Feature {}: {error}",
                feature_path.display()
            ))
        })?;
        if let Some(parent) = feature_path.parent() {
            let _ = fs::remove_dir(parent);
        }
        Ok(())
    }
}

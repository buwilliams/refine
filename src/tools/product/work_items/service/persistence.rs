use super::*;

impl FileWorkItemService {
    pub(super) fn projection_snapshot(
        &self,
    ) -> RefineResult<crate::tools::product::project_state::ProjectionSnapshot> {
        if let Some(cache_dir) = &self.projection_cache_dir {
            // Same explicit runtime root as ownership resolution: inferring it from
            // the cache directory pointed the store at a nested cache path.
            let store = self
                .active_node_root
                .as_ref()
                .map(|runtime_root| {
                    FileProjectStateStore::with_runtime_root(&self.refine_dir, runtime_root)
                })
                .unwrap_or_else(|| FileProjectStateStore::new(&self.refine_dir));
            store.load_or_refresh_projection(cache_dir)
        } else {
            let store = FileProjectStateStore::new(&self.refine_dir);
            store.rebuild_projection()
        }
    }

    /// Serialize a mutation of one Goal against other writers of that Goal.
    ///
    /// Keyed by the Goal's own identifier so that locking it and then writing
    /// its record resolve to the same lock and nest re-entrantly. This replaces
    /// a single lock covering the whole target application, under which two
    /// unrelated Goals could not be mutated concurrently no matter how much
    /// capacity the host had.
    pub(super) fn acquire_goal_mutation_lock(
        &self,
        goal_id: &str,
    ) -> RefineResult<GoalMutationLock> {
        fs::create_dir_all(&self.refine_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create Refine state directory {}: {error}",
                self.refine_dir.display()
            ))
        })?;
        Ok(GoalMutationLock {
            _record: acquire_record_lock(&self.refine_dir, goal_id)?,
        })
    }

    pub(super) fn active_node_id(&self) -> RefineResult<String> {
        if let Some(node_id) = &self.active_node_id_override {
            return Ok(node_id.clone());
        }
        self.node_registry_service().active_node_id()
    }

    pub(super) fn node_registry_service(&self) -> FileNodeRegistryService {
        match &self.active_node_root {
            Some(active_root) => {
                FileNodeRegistryService::with_active_root(&self.refine_dir, active_root)
            }
            None => FileNodeRegistryService::new(&self.refine_dir),
        }
    }

    pub(super) fn ensure_goal_owned(&self, goal: &GoalSummaryProjection) -> RefineResult<()> {
        self.ensure_goal_index_owned(&goal.goal)
    }

    /// Ownership decided from index fields alone, so callers holding a Goal from
    /// the scheduler index need not materialize a full summary for it.
    pub(super) fn ensure_goal_index_owned(&self, goal: &GoalIndexProjection) -> RefineResult<()> {
        let owner = goal
            .node_id
            .as_deref()
            .filter(|node_id| !node_id.is_empty())
            .unwrap_or("default");
        let active = self.active_node_id()?;
        if owner == active {
            Ok(())
        } else {
            Err(RefineError::Conflict(format!(
                "Goal {} is owned by node {owner}, not active node {active}",
                goal.id
            )))
        }
    }

    pub(super) fn ensure_feature_owned(
        &self,
        feature: &FeatureSummaryProjection,
    ) -> RefineResult<()> {
        let owner = feature
            .feature
            .node_id
            .as_deref()
            .filter(|node_id| !node_id.is_empty())
            .unwrap_or("default");
        let active = self.active_node_id()?;
        if owner == active {
            Ok(())
        } else {
            Err(RefineError::Conflict(format!(
                "Feature {} is owned by node {owner}, not active node {active}",
                feature.feature.id
            )))
        }
    }

    pub(super) fn read_goal_value(
        &self,
        goal_id: &str,
    ) -> RefineResult<(GoalMutationLock, PathBuf, Value)> {
        let goal_lock = self.acquire_goal_mutation_lock(goal_id)?;
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, value) = self.read_goal_value_unchecked_locked(&current)?;
        Ok((goal_lock, goal_path, value))
    }

    pub(super) fn read_goal_value_unchecked(
        &self,
        current: &GoalSummaryProjection,
    ) -> RefineResult<(GoalMutationLock, PathBuf, Value)> {
        let goal_lock = self.acquire_goal_mutation_lock(&current.goal.id)?;
        let (goal_path, value) = self.read_goal_value_unchecked_locked(current)?;
        Ok((goal_lock, goal_path, value))
    }

    pub(super) fn read_goal_value_unchecked_locked(
        &self,
        current: &GoalSummaryProjection,
    ) -> RefineResult<(PathBuf, Value)> {
        let goal_path = self.refine_dir.join(&current.goal.json_path);
        let bytes = fs::read(&goal_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse Goal {}: {error}",
                goal_path.display()
            ))
        })?;
        Ok((goal_path, value))
    }

    pub(super) fn set_goal_feature_membership(
        &self,
        goal_id: &str,
        feature_id: Option<&str>,
        feature_order: Option<i64>,
    ) -> RefineResult<()> {
        let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "feature_id".to_string(),
            feature_id
                .map(|id| Value::String(id.to_string()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "feature_order".to_string(),
            feature_order
                .map(|order| Value::Number(order.into()))
                .unwrap_or(Value::Null),
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)
    }

    pub(crate) fn set_goal_status_unchecked(
        &self,
        goal_id: &str,
        status: &GoalStatus,
    ) -> RefineResult<()> {
        let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
        self.write_goal_status_value(&goal_path, &mut value, status)
    }

    pub(crate) fn set_goal_status_unchecked_locked(
        &self,
        goal_id: &str,
        status: &GoalStatus,
    ) -> RefineResult<()> {
        let current = self.show_goal_summary(goal_id)?;
        self.ensure_goal_owned(&current)?;
        let (goal_path, mut value) = self.read_goal_value_unchecked_locked(&current)?;
        self.write_goal_status_value(&goal_path, &mut value, status)
    }

    pub(super) fn write_goal_status_value(
        &self,
        goal_path: &std::path::Path,
        value: &mut Value,
        status: &GoalStatus,
    ) -> RefineResult<()> {
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "status".to_string(),
            Value::String(status.as_str().to_string()),
        );
        if !matches!(status, GoalStatus::Failed) {
            clear_latest_round_failure(object);
        }
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(goal_path, value)
    }

    pub(super) fn set_goal_priority_unchecked(
        &self,
        goal_id: &str,
        priority: &str,
    ) -> RefineResult<()> {
        let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert("priority".to_string(), Value::String(priority.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)
    }

    pub(super) fn set_goal_reporter_unchecked(
        &self,
        goal_id: &str,
        reporter: &str,
    ) -> RefineResult<()> {
        let reporter = Self::validate_goal_reporter(reporter)?;
        let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert(
            "reporter".to_string(),
            if reporter.is_empty() {
                Value::Null
            } else {
                Value::String(reporter.to_string())
            },
        );
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)
    }

    pub(super) fn set_goal_node_unchecked(&self, goal_id: &str, node_id: &str) -> RefineResult<()> {
        let current = self.show_goal_summary(goal_id)?;
        let (_goal_lock, goal_path, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {} is not a JSON object", goal_path.display()))
        })?;
        object.insert("node_id".to_string(), Value::String(node_id.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&goal_path, &value)
    }

    pub(super) fn set_feature_node_unchecked(
        &self,
        feature_id: &str,
        node_id: &str,
    ) -> RefineResult<()> {
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
        object.insert("node_id".to_string(), Value::String(node_id.to_string()));
        object.insert("updated".to_string(), Value::String(now_timestamp()));
        write_json_atomically(&feature_path, &value)
    }

    pub(super) fn validate_transfer_target_node(
        &self,
        target_node_id: &str,
    ) -> RefineResult<String> {
        let target_node_id = target_node_id.trim();
        if target_node_id.is_empty() {
            return Err(RefineError::InvalidInput(
                "target_node_id is required".to_string(),
            ));
        }
        self.node_registry_service()
            .ensure_transfer_target(target_node_id)?;
        Ok(target_node_id.to_string())
    }

    pub(super) fn set_latest_round_assignee(
        &self,
        goal_id: &str,
        assignee: &str,
    ) -> RefineResult<()> {
        let assignee = Self::validate_goal_assignee(assignee)?;
        let (_goal_lock, goal_path, mut value) = self.read_goal_value(goal_id)?;
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
            .find(|round| round.is_object())
            .ok_or_else(|| RefineError::NotFound(format!("Goal {goal_id} has no rounds")))?;
        let latest = latest.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!(
                "latest round for Goal {goal_id} is not a JSON object"
            ))
        })?;
        let now = now_timestamp();
        latest.insert(
            "assignee".to_string(),
            if assignee.is_empty() {
                Value::Null
            } else {
                Value::String(assignee.to_string())
            },
        );
        latest.insert("updated".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        write_json_atomically(&goal_path, &value)
    }

    pub(super) fn next_feature_order(&self, feature_id: &str) -> RefineResult<i64> {
        let max_order = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .filter_map(|goal| goal.goal.feature_order)
            .max()
            .unwrap_or(0);
        Ok(max_order + 1)
    }

    pub(super) fn compact_feature_orders(&self, feature_id: &str) -> RefineResult<()> {
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
        for (idx, goal) in goals.iter().enumerate() {
            self.set_goal_feature_membership(
                &goal.goal.id,
                Some(feature_id),
                Some(idx as i64 + 1),
            )?;
        }
        Ok(())
    }

    pub(super) fn feature_goal_summaries(
        &self,
        feature_id: &str,
    ) -> RefineResult<Vec<GoalSummaryProjection>> {
        let mut goals: Vec<_> = self
            .list_goal_summaries()?
            .into_iter()
            .filter(|goal| goal.goal.feature_id.as_deref() == Some(feature_id))
            .collect();
        goals.sort_by(|a, b| {
            compare_feature_goal_order(a.goal.feature_order, b.goal.feature_order)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        Ok(goals)
    }

    pub(super) fn select_bulk_goal_summaries(
        &self,
        selection: &BulkGoalSelection,
        status_protection: BulkGoalStatusProtection,
    ) -> RefineResult<(Vec<GoalSummaryProjection>, Vec<BulkSkippedDetail>)> {
        let excluded: BTreeSet<_> = selection
            .exclude_ids
            .iter()
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .collect();
        let mut goals = if let Some(selected_ids) = &selection.selected_ids {
            let mut selected = Vec::new();
            for id in selected_ids {
                let id = id.trim().to_uppercase();
                if id.is_empty() || excluded.contains(&id) {
                    continue;
                }
                selected.push(self.show_goal_summary(&id)?);
            }
            selected
        } else {
            self.list_goal_summaries()?
                .into_iter()
                .filter(|goal| !excluded.contains(&goal.goal.id))
                .filter(|goal| {
                    bulk_goal_matches_filter(Some(&self.refine_dir), goal, &selection.filter)
                })
                .collect()
        };
        goals.sort_by(|a, b| a.goal.id.cmp(&b.goal.id));
        let mut skipped_details = Vec::new();
        if !matches!(status_protection, BulkGoalStatusProtection::None) {
            let mut retained = Vec::new();
            for goal in goals {
                let protected = is_automated_status(&goal.goal.status)
                    || matches!(status_protection, BulkGoalStatusProtection::WorkflowOwned)
                        && matches!(goal.goal.status, GoalStatus::Review | GoalStatus::Done);
                if protected {
                    skipped_details.push(BulkSkippedDetail {
                        id: goal.goal.id,
                        reason: format!("status:{}", goal.goal.status.as_str()),
                    });
                } else {
                    retained.push(goal);
                }
            }
            goals = retained;
        }
        Ok((goals, skipped_details))
    }

    pub(super) fn select_bulk_feature_summaries(
        &self,
        selection: &BulkFeatureSelection,
    ) -> RefineResult<Vec<FeatureSummaryProjection>> {
        let excluded: BTreeSet<_> = selection
            .exclude_ids
            .iter()
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .collect();
        let mut features = if let Some(selected_ids) = &selection.selected_ids {
            let mut selected = Vec::new();
            for id in selected_ids {
                let id = id.trim().to_uppercase();
                if id.is_empty() || excluded.contains(&id) {
                    continue;
                }
                selected.push(self.show_feature_summary(&id)?);
            }
            selected
        } else {
            let active_node_id = self.active_node_id()?;
            self.list_feature_summaries()?
                .into_iter()
                .filter(|feature| !excluded.contains(&feature.feature.id))
                .filter(|feature| {
                    bulk_feature_matches_filter(feature, &selection.filter, &active_node_id)
                })
                .collect()
        };
        features.sort_by(|a, b| a.feature.id.cmp(&b.feature.id));
        Ok(features)
    }
}

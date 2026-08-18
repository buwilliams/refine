use super::*;

impl FileWorkItemService {
    pub fn bulk_update_goals(
        &self,
        selection: BulkGoalSelection,
        update: BulkGoalUpdate,
    ) -> RefineResult<BulkUpdateResult> {
        let (field, raw_value) = match update {
            BulkGoalUpdate::Priority(value) => ("priority".to_string(), value.trim().to_string()),
            BulkGoalUpdate::Status(value) => ("status".to_string(), value.trim().to_lowercase()),
            BulkGoalUpdate::Reporter(value) => ("reporter".to_string(), value.trim().to_string()),
            BulkGoalUpdate::Assignee(value) => ("assignee".to_string(), value.trim().to_string()),
        };
        if field == "priority" && GoalPriority::parse_wire(&raw_value).is_none() {
            return Err(RefineError::InvalidInput(
                "priority must be one of low, medium, or high".to_string(),
            ));
        }
        if field == "status" && raw_value != "__last_workflow_state" {
            let Some(status) = GoalStatus::parse_wire(&raw_value) else {
                return Err(RefineError::InvalidInput("invalid status".to_string()));
            };
            if !is_bulk_target_allowed(&status) {
                return Err(RefineError::Conflict(
                    "Bulk status updates cannot enter automated workflow states".to_string(),
                ));
            }
        }
        if field == "reporter" && !valid_reporter_name(&raw_value) {
            return Err(RefineError::InvalidInput(
                "invalid reporter name".to_string(),
            ));
        }
        if field == "assignee" && !valid_reporter_name(&raw_value) {
            return Err(RefineError::InvalidInput(
                "invalid assignee name".to_string(),
            ));
        }

        let status_protection = match (field.as_str(), raw_value.as_str()) {
            ("status", "review" | "done") => BulkGoalStatusProtection::Automated,
            ("status", _) => BulkGoalStatusProtection::WorkflowOwned,
            _ => BulkGoalStatusProtection::None,
        };
        let (goals, mut skipped_details) =
            self.select_bulk_goal_summaries(&selection, status_protection)?;
        #[cfg(test)]
        if let Some(hook) = &self.after_bulk_goal_selection_hook {
            hook();
        }
        let mut ids = Vec::new();
        for goal in goals {
            if field == "status" {
                match self.mutate_bulk_goal_status_if_eligible(
                    &goal.goal.id,
                    &raw_value,
                    status_protection,
                )? {
                    BulkGoalStatusMutation::Updated => ids.push(goal.goal.id),
                    BulkGoalStatusMutation::Skipped(reason) => {
                        skipped_details.push(BulkSkippedDetail {
                            id: goal.goal.id,
                            reason,
                        });
                    }
                }
                continue;
            }
            self.ensure_goal_owned(&goal)?;
            match field.as_str() {
                "priority" => self.set_goal_priority_unchecked(&goal.goal.id, &raw_value)?,
                "reporter" => self.set_goal_reporter_unchecked(&goal.goal.id, &raw_value)?,
                "assignee" => self.set_latest_round_assignee(&goal.goal.id, &raw_value)?,
                _ => unreachable!(),
            }
            ids.push(goal.goal.id);
        }
        Ok(BulkUpdateResult {
            updated: ids.len(),
            ids,
            field,
            value: raw_value,
            skipped: skipped_details.len(),
            skipped_details,
            failed: 0,
            failures: Vec::new(),
        })
    }

    pub fn select_bulk_goal_ids(&self, selection: &BulkGoalSelection) -> RefineResult<Vec<String>> {
        let (goals, _) =
            self.select_bulk_goal_summaries(selection, BulkGoalStatusProtection::None)?;
        Ok(goals.into_iter().map(|goal| goal.goal.id).collect())
    }

    pub fn bulk_delete_goals(
        &self,
        selection: BulkGoalSelection,
    ) -> RefineResult<BulkDeleteResult> {
        let (goals, _) =
            self.select_bulk_goal_summaries(&selection, BulkGoalStatusProtection::None)?;
        let mut ids = Vec::new();
        let mut feature_ids = BTreeSet::new();
        for goal in goals {
            self.ensure_goal_owned(&goal)?;
            if let Some(feature_id) = &goal.goal.feature_id {
                feature_ids.insert(feature_id.clone());
            }
            self.delete_goal_record(&goal.goal.id)?;
            ids.push(goal.goal.id);
        }
        for feature_id in feature_ids {
            let _ = self.compact_feature_orders(&feature_id);
        }
        Ok(BulkDeleteResult {
            deleted: ids.len(),
            ids,
            failures: Vec::new(),
            failed: 0,
        })
    }

    pub fn bulk_update_features(
        &self,
        selection: BulkFeatureSelection,
        update: BulkFeatureUpdate,
    ) -> RefineResult<BulkUpdateResult> {
        let (field, raw_value) = match update {
            BulkFeatureUpdate::Reporter(value) => {
                ("reporter".to_string(), value.trim().to_string())
            }
            BulkFeatureUpdate::Assignee(value) => {
                ("assignee".to_string(), value.trim().to_string())
            }
        };
        if field == "reporter" && !valid_reporter_name(&raw_value) {
            return Err(RefineError::InvalidInput(
                "invalid reporter name".to_string(),
            ));
        }
        if field == "assignee" && !valid_reporter_name(&raw_value) {
            return Err(RefineError::InvalidInput(
                "invalid assignee name".to_string(),
            ));
        }
        let features = self.select_bulk_feature_summaries(&selection)?;
        let mut ids = Vec::new();
        for feature in features {
            self.ensure_feature_owned(&feature)?;
            match field.as_str() {
                "reporter" => {
                    self.update_feature_metadata_summary(
                        &feature.feature.id,
                        None,
                        None,
                        Some(&raw_value),
                        None,
                    )?;
                }
                "assignee" => {
                    self.update_feature_metadata_summary(
                        &feature.feature.id,
                        None,
                        None,
                        None,
                        Some(&raw_value),
                    )?;
                }
                _ => unreachable!(),
            }
            ids.push(feature.feature.id);
        }
        Ok(BulkUpdateResult {
            updated: ids.len(),
            ids,
            field,
            value: raw_value,
            skipped: 0,
            skipped_details: Vec::new(),
            failed: 0,
            failures: Vec::new(),
        })
    }

    pub fn bulk_delete_features(
        &self,
        selection: BulkFeatureSelection,
    ) -> RefineResult<BulkDeleteResult> {
        let features = self.select_bulk_feature_summaries(&selection)?;
        let mut ids = Vec::new();
        for feature in features {
            self.delete_feature_record(&feature.feature.id)?;
            ids.push(feature.feature.id);
        }
        Ok(BulkDeleteResult {
            deleted: ids.len(),
            ids,
            failures: Vec::new(),
            failed: 0,
        })
    }

    pub fn bulk_assign_goals_to_feature(
        &self,
        feature_id: &str,
        selection: BulkGoalSelection,
    ) -> RefineResult<BulkAssignFeatureResult> {
        let feature = self.show_feature_summary(feature_id)?;
        self.ensure_feature_owned(&feature)?;
        let (goals, mut skipped_details) =
            self.select_bulk_goal_summaries(&selection, BulkGoalStatusProtection::None)?;
        let mut old_feature_ids = BTreeSet::new();
        let mut ids = Vec::new();
        for goal in goals {
            self.ensure_goal_owned(&goal)?;
            if goal.goal.feature_id.as_deref() == Some(feature_id) {
                skipped_details.push(BulkSkippedDetail {
                    id: goal.goal.id,
                    reason: "already-assigned".to_string(),
                });
                continue;
            }
            validate_goal_operation(&goal.goal.status, &GoalOperation::AssignToFeature)?;
            if let Some(old_feature_id) = &goal.goal.feature_id {
                old_feature_ids.insert(old_feature_id.clone());
            }
            self.set_goal_feature_membership(&goal.goal.id, Some(feature_id), None)?;
            ids.push(goal.goal.id);
        }
        for old_feature_id in old_feature_ids {
            let _ = self.compact_feature_orders(&old_feature_id);
        }
        Ok(BulkAssignFeatureResult {
            feature_id: feature_id.to_string(),
            updated: ids.len(),
            ids,
            skipped: skipped_details.len(),
            skipped_details,
        })
    }

    pub fn bulk_transfer_goals_to_node(
        &self,
        target_node_id: &str,
        selection: BulkGoalSelection,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = self.validate_transfer_target_node(target_node_id)?;
        let (goals, mut skipped_details) =
            self.select_bulk_goal_summaries(&selection, BulkGoalStatusProtection::None)?;
        let mut ids = Vec::new();
        for goal in goals {
            if let Some(reason) = goal_transfer_skip_reason(&goal) {
                skipped_details.push(BulkSkippedDetail {
                    id: goal.goal.id,
                    reason,
                });
                continue;
            }
            self.set_goal_node_unchecked(&goal.goal.id, &target_node_id)?;
            ids.push(goal.goal.id);
        }
        Ok(BulkTransferNodeResult {
            target_node_id,
            updated: ids.len(),
            ids,
            skipped: skipped_details.len(),
            skipped_details,
        })
    }

    pub fn transfer_goal_to_node(
        &self,
        target_node_id: &str,
        goal_id: &str,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = self.validate_transfer_target_node(target_node_id)?;
        let goal = self.show_goal_summary(goal_id)?;
        validate_goal_transfer_to_node(&goal)?;
        self.set_goal_node_unchecked(&goal.goal.id, &target_node_id)?;
        Ok(BulkTransferNodeResult {
            target_node_id,
            updated: 1,
            ids: vec![goal.goal.id],
            skipped: 0,
            skipped_details: Vec::new(),
        })
    }

    pub fn transfer_feature_to_node(
        &self,
        target_node_id: &str,
        feature_id: &str,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = self.validate_transfer_target_node(target_node_id)?;
        let feature = self.show_feature_summary(feature_id)?;
        let mut goals = Vec::new();
        for goal_id in &feature.goal_ids {
            let goal = self.show_goal_summary(goal_id)?;
            if let Some(reason) = goal_status_transfer_skip_reason(&goal) {
                return Err(RefineError::Conflict(format!(
                    "Feature {} cannot transfer while Goal {} is not transferable ({reason})",
                    feature.feature.id, goal.goal.id
                )));
            }
            goals.push(goal);
        }
        self.set_feature_node_unchecked(&feature.feature.id, &target_node_id)?;
        let mut ids = vec![feature.feature.id];
        for goal in goals {
            self.set_goal_node_unchecked(&goal.goal.id, &target_node_id)?;
            ids.push(goal.goal.id);
        }
        Ok(BulkTransferNodeResult {
            target_node_id,
            updated: ids.len(),
            ids,
            skipped: 0,
            skipped_details: Vec::new(),
        })
    }

    pub fn bulk_transfer_features_to_node(
        &self,
        target_node_id: &str,
        selection: BulkFeatureSelection,
    ) -> RefineResult<BulkTransferNodeResult> {
        let target_node_id = self.validate_transfer_target_node(target_node_id)?;
        let features = self.select_bulk_feature_summaries(&selection)?;
        let mut ids = Vec::new();
        let mut skipped_details = Vec::new();
        for feature in features {
            self.ensure_feature_owned(&feature)?;
            let mut goals = Vec::new();
            let mut skip_reason = None;
            for goal_id in &feature.goal_ids {
                let goal = self.show_goal_summary(goal_id)?;
                if let Some(reason) = goal_status_transfer_skip_reason(&goal) {
                    skip_reason = Some(format!("goal:{}:{reason}", goal.goal.id));
                    break;
                }
                goals.push(goal);
            }
            if let Some(reason) = skip_reason {
                skipped_details.push(BulkSkippedDetail {
                    id: feature.feature.id,
                    reason,
                });
                continue;
            }
            self.set_feature_node_unchecked(&feature.feature.id, &target_node_id)?;
            ids.push(feature.feature.id);
            for goal in goals {
                self.set_goal_node_unchecked(&goal.goal.id, &target_node_id)?;
                ids.push(goal.goal.id);
            }
        }
        Ok(BulkTransferNodeResult {
            target_node_id,
            updated: ids.len(),
            ids,
            skipped: skipped_details.len(),
            skipped_details,
        })
    }

    /// Reassigns node ownership of eligible Goals across the given nodes.
    /// Distribute is the one sanctioned exception to node ownership
    /// enforcement: queued work may move regardless of which node owns it,
    /// because reassignment is the transfer. Eligible means captured or
    /// actionable (backlog/todo); converge instead moves
    /// reviewable Goals home to a single review node. Feature-bound goals are
    /// skipped so Feature ordering stays intact — transfer the Feature to move
    /// them as a unit.
    pub fn distribute_goals_across_nodes(
        &self,
        target_node_ids: &[String],
        converge: bool,
        dry_run: bool,
    ) -> RefineResult<DistributeResult> {
        if target_node_ids.is_empty() {
            return Err(RefineError::InvalidInput(
                "distribute requires at least one enabled, healthy node".to_string(),
            ));
        }
        let mut node_ids = Vec::new();
        for node_id in target_node_ids {
            let node_id = self.validate_transfer_target_node(node_id)?;
            if !node_ids.contains(&node_id) {
                node_ids.push(node_id);
            }
        }
        if converge && node_ids.len() != 1 {
            return Err(RefineError::InvalidInput(
                "converge moves reviewable goals to exactly one review node".to_string(),
            ));
        }
        let mut summaries = self.list_goal_summaries()?;
        summaries.sort_by(|a, b| {
            a.goal
                .created
                .cmp(&b.goal.created)
                .then_with(|| a.goal.id.cmp(&b.goal.id))
        });
        let mut eligible = Vec::new();
        let mut skipped_details = Vec::new();
        let mut load: Vec<usize> = vec![0; node_ids.len()];
        for goal in &summaries {
            let owner = goal
                .goal
                .node_id
                .clone()
                .unwrap_or_else(|| "default".to_string());
            let matches = if converge {
                goal.goal.status == GoalStatus::Review
            } else {
                matches!(goal.goal.status, GoalStatus::Backlog | GoalStatus::Todo)
            };
            if !matches {
                if !is_terminal_status(&goal.goal.status)
                    && let Some(index) = node_ids.iter().position(|id| *id == owner)
                {
                    load[index] += 1;
                }
                continue;
            }
            if let Some(feature_id) = goal.goal.feature_id.as_deref() {
                skipped_details.push(BulkSkippedDetail {
                    id: goal.goal.id.clone(),
                    reason: format!("feature:{feature_id}"),
                });
                if let Some(index) = node_ids.iter().position(|id| *id == owner) {
                    load[index] += 1;
                }
                continue;
            }
            eligible.push((goal.goal.id.clone(), owner));
        }
        let mut moves = Vec::new();
        for (goal_id, owner) in &eligible {
            let target_index = load
                .iter()
                .enumerate()
                .min_by_key(|(index, count)| (**count, *index))
                .map(|(index, _)| index)
                .unwrap_or(0);
            let to_node_id = node_ids[target_index].clone();
            load[target_index] += 1;
            if to_node_id != *owner {
                moves.push(DistributeMove {
                    goal_id: goal_id.clone(),
                    from_node_id: owner.clone(),
                    to_node_id,
                });
            }
        }
        if !dry_run {
            for entry in &moves {
                self.set_goal_node_unchecked(&entry.goal_id, &entry.to_node_id)?;
            }
        }
        Ok(DistributeResult {
            strategy: if converge {
                "converge".to_string()
            } else if node_ids.len() == 1 {
                "fill".to_string()
            } else {
                "spread".to_string()
            },
            node_ids,
            eligible: eligible.len(),
            moved: moves.len(),
            moves,
            skipped: skipped_details.len(),
            skipped_details,
            dry_run,
        })
    }

    pub fn transfer_item_to_node(
        &self,
        target_node_id: &str,
        item_id: &str,
    ) -> RefineResult<BulkTransferNodeResult> {
        let item_id = item_id.trim();
        if item_id.is_empty() {
            return Err(RefineError::InvalidInput("item_id is required".to_string()));
        }
        match self.show_feature_summary(item_id) {
            Ok(_) => self.transfer_feature_to_node(target_node_id, item_id),
            Err(feature_error) => match self.transfer_goal_to_node(target_node_id, item_id) {
                Ok(result) => Ok(result),
                Err(goal_error)
                    if matches!(
                        feature_error.category(),
                        crate::error::ErrorCategory::NotFound
                    ) && matches!(
                        goal_error.category(),
                        crate::error::ErrorCategory::NotFound
                    ) =>
                {
                    Err(RefineError::NotFound(format!(
                        "Goal or Feature {item_id} was not found in refine state"
                    )))
                }
                Err(goal_error) => Err(goal_error),
            },
        }
    }

    pub(crate) fn verify_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let current = self.show_goal_summary(goal_id)?;
        validate_goal_operation(&current.goal.status, &GoalOperation::VerifyReview)?;
        self.set_goal_status_unchecked(goal_id, &GoalStatus::Done)?;
        self.show_goal_summary(goal_id)
    }
}

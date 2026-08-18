use super::*;

impl FileWorkItemService {
    pub(super) fn validate_feature_goal_placement(
        &self,
        feature_id: &str,
        goal_id: Option<&str>,
        placement: &FeatureGoalPlacement,
    ) -> RefineResult<()> {
        let FeatureGoalPlacement::After(prerequisite_id) = placement else {
            return Ok(());
        };
        let prerequisite_id = prerequisite_id.trim();
        if prerequisite_id.is_empty() || goal_id == Some(prerequisite_id) {
            return Err(RefineError::InvalidInput(
                "placement prerequisite must name a different Goal".to_string(),
            ));
        }
        let prerequisite = self.show_goal_summary(prerequisite_id).map_err(|_| {
            RefineError::InvalidInput(format!(
                "placement prerequisite {prerequisite_id} is not ordered in Feature {feature_id}"
            ))
        })?;
        if prerequisite.goal.feature_id.as_deref() != Some(feature_id)
            || !is_ordered_feature_goal(prerequisite.goal.feature_order)
        {
            return Err(RefineError::InvalidInput(format!(
                "placement prerequisite {prerequisite_id} is not ordered in Feature {feature_id}"
            )));
        }
        Ok(())
    }

    pub(super) fn apply_feature_goal_placement(
        &self,
        feature_id: &str,
        goal_id: &str,
        placement: &FeatureGoalPlacement,
    ) -> RefineResult<()> {
        let current = self.show_goal_summary(goal_id)?;
        match placement {
            FeatureGoalPlacement::Unordered => {
                if is_ordered_feature_goal(current.goal.feature_order) {
                    self.unorder_goal_in_feature(feature_id, goal_id)?;
                }
            }
            FeatureGoalPlacement::First => {
                if !is_ordered_feature_goal(current.goal.feature_order) {
                    self.order_goal_in_feature(feature_id, goal_id)?;
                }
                self.reorder_goal_in_feature(feature_id, goal_id, 1)?;
            }
            FeatureGoalPlacement::After(prerequisite_id) => {
                if !is_ordered_feature_goal(current.goal.feature_order) {
                    self.order_goal_in_feature(feature_id, goal_id)?;
                }
                let feature = self.show_feature_summary(feature_id)?;
                let mut ordered_ids = feature
                    .goal_ids
                    .iter()
                    .filter(|candidate_id| {
                        self.show_goal_summary(candidate_id)
                            .ok()
                            .is_some_and(|goal| is_ordered_feature_goal(goal.goal.feature_order))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let source_index = ordered_ids
                    .iter()
                    .position(|candidate_id| candidate_id == goal_id)
                    .ok_or_else(|| {
                        RefineError::NotFound(format!(
                            "Goal {goal_id} was not found in Feature {feature_id} order"
                        ))
                    })?;
                ordered_ids.remove(source_index);
                let target_index = ordered_ids
                    .iter()
                    .position(|candidate_id| candidate_id == prerequisite_id.trim())
                    .ok_or_else(|| {
                        RefineError::InvalidInput(format!(
                            "placement prerequisite {prerequisite_id} is not ordered in Feature {feature_id}"
                        ))
                    })?;
                self.reorder_goal_in_feature(feature_id, goal_id, target_index as i64 + 2)?;
            }
        }
        Ok(())
    }

    pub(super) fn node_identity(
        &self,
        node_id: Option<&str>,
    ) -> Option<crate::application::fleet::nodes::NodeIdentity> {
        let node_id = node_id.unwrap_or("default");
        self.node_registry_service().node_identity(node_id).ok()
    }

    pub(super) fn attach_round_logs(
        &self,
        goal_id: &str,
        object: &mut Map<String, Value>,
    ) -> RefineResult<()> {
        let Some(rounds) = object.get_mut("rounds").and_then(Value::as_array_mut) else {
            return Ok(());
        };
        let log_service = FileLogService::new(&self.refine_dir);
        // One sidecar read for all rounds; per-round reads reparsed the whole
        // log file once per round.
        let mut logs_by_round: std::collections::BTreeMap<usize, Vec<_>> =
            std::collections::BTreeMap::new();
        for entry in log_service.all_round_logs(goal_id)? {
            if let Some(round_idx) = entry.round_idx {
                logs_by_round.entry(round_idx).or_default().push(entry);
            }
        }
        let round_count = rounds.len();
        for (idx, round) in rounds.iter_mut().enumerate() {
            let logs = logs_by_round.remove(&idx).unwrap_or_default();
            let Some(round_object) = round.as_object_mut() else {
                continue;
            };
            if !logs.is_empty() {
                let value = serde_json::to_value(&logs).map_err(|error| {
                    RefineError::Serialization(format!("failed to encode Goal logs: {error}"))
                })?;
                round_object.insert("logs".to_string(), value);
            }
            if idx + 1 == round_count {
                attach_latest_log_fields(round_object, &logs)?;
            }
        }
        Ok(())
    }
}

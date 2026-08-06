use super::*;

impl FileWorkItemService {
    pub fn create_goal_summary(
        &self,
        name: &str,
        id: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RefineError::InvalidInput(
                "Goal name is required".to_string(),
            ));
        }
        let goal_id = id
            .map(|id| id.trim().to_uppercase())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(new_ulid_like);
        if goal_id.len() < 3 {
            return Err(RefineError::InvalidInput(
                "Goal id must be at least three characters".to_string(),
            ));
        }

        let goal_path = goal_json_path(&self.refine_dir, &goal_id);
        if goal_path.exists() {
            return Err(RefineError::Conflict(format!(
                "Goal {goal_id} already exists"
            )));
        }
        let node_id = self.active_node_id()?;
        let now = now_timestamp();
        let mut object = Map::new();
        object.insert("id".to_string(), Value::String(goal_id.clone()));
        object.insert("name".to_string(), Value::String(name.to_string()));
        object.insert("status".to_string(), Value::String("backlog".to_string()));
        object.insert("priority".to_string(), Value::String("low".to_string()));
        object.insert("reporter".to_string(), Value::Null);
        object.insert("branch_name".to_string(), Value::Null);
        object.insert("target_branch".to_string(), Value::Null);
        object.insert("base_commit".to_string(), Value::Null);
        object.insert("candidate_commit".to_string(), Value::Null);
        object.insert("feature_id".to_string(), Value::Null);
        object.insert("feature_order".to_string(), Value::Null);
        object.insert("node_id".to_string(), Value::String(node_id));
        object.insert("created".to_string(), Value::String(now.clone()));
        object.insert("updated".to_string(), Value::String(now));
        object.insert("notes".to_string(), Value::Array(Vec::new()));
        object.insert("rounds".to_string(), Value::Array(Vec::new()));
        write_json_atomically(&goal_path, &Value::Object(object))?;
        self.show_goal_summary(&goal_id)
    }

    pub fn show_goal_summary(&self, goal_id: &str) -> RefineResult<GoalSummaryProjection> {
        let snapshot = self.projection_snapshot()?;
        snapshot.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} was not found in refine state"))
        })
    }

    pub fn show_goal_detail(&self, goal_id: &str) -> RefineResult<Value> {
        let snapshot = self.projection_snapshot()?;
        let current = snapshot.goals.get(goal_id).cloned().ok_or_else(|| {
            RefineError::NotFound(format!("Goal {goal_id} was not found in refine state"))
        })?;
        let (_goal_lock, _, mut value) = self.read_goal_value_unchecked(&current)?;
        let object = value.as_object_mut().ok_or_else(|| {
            RefineError::Serialization(format!("Goal {goal_id} is not a JSON object"))
        })?;
        object.insert(
            "reporter".to_string(),
            current
                .goal
                .reporter
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        object.insert(
            "round_count".to_string(),
            Value::from(current.goal.round_count),
        );
        object.insert(
            "assignee".to_string(),
            current
                .goal
                .assignee
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        if let Some(identity) = self.node_identity(current.goal.node_id.as_deref()) {
            object.insert(
                "node_display_name".to_string(),
                Value::String(identity.display_name),
            );
            if !identity.diagnostics.is_empty() {
                object.insert(
                    "node_identity_diagnostics".to_string(),
                    serde_json::to_value(identity.diagnostics).map_err(|error| {
                        RefineError::Serialization(format!(
                            "failed to encode node identity diagnostics: {error}"
                        ))
                    })?,
                );
            }
        } else {
            object.insert(
                "node_display_name".to_string(),
                Value::String(current.goal.node_id.clone().map_or_else(
                    || "Default".to_string(),
                    |node_id| {
                        if node_id == "default" {
                            "Default".to_string()
                        } else {
                            node_id
                        }
                    },
                )),
            );
        }
        if let Some(feature_id) = current.goal.feature_id.as_deref() {
            let mut feature_goals = snapshot
                .goals
                .values()
                .filter(|projection| projection.goal.feature_id.as_deref() == Some(feature_id))
                .map(|projection| projection.goal.clone())
                .collect::<Vec<_>>();
            feature_goals.sort_by(|a, b| {
                compare_feature_goal_order(a.feature_order, b.feature_order)
                    .then_with(|| a.id.cmp(&b.id))
            });
            if let Some(notice) = failed_goal_feature_blocking_notice(&current.goal, &feature_goals)
            {
                let notice = serde_json::to_value(notice).map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to encode Feature blocking notice: {error}"
                    ))
                })?;
                object.insert("feature_blocking_notice".to_string(), notice);
            }
        }
        self.attach_round_logs(goal_id, object)?;
        Ok(value)
    }

    pub fn list_goal_summaries(&self) -> RefineResult<Vec<GoalSummaryProjection>> {
        let snapshot = self.projection_snapshot()?;
        Ok(snapshot.goals.into_values().collect())
    }
}

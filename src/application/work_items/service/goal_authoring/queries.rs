use super::*;
use serde_json::json;

impl FileWorkItemService {
    pub fn create_goal_summary(
        &self,
        name: &str,
        id: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        self.create_goal_in_step(name, id, GoalStatus::Backlog, None, None, None)
    }

    pub fn create_goal_in_step(
        &self,
        name: &str,
        id: Option<&str>,
        status: GoalStatus,
        description: Option<&str>,
        reporter: Option<&str>,
        priority: Option<&str>,
    ) -> RefineResult<GoalSummaryProjection> {
        self.create_goal_record(name, id, status, description, reporter, priority, None)
    }

    pub(in crate::application::work_items::service) fn create_goal_record(
        &self,
        name: &str,
        id: Option<&str>,
        status: GoalStatus,
        description: Option<&str>,
        reporter: Option<&str>,
        priority: Option<&str>,
        historical: Option<&Value>,
    ) -> RefineResult<GoalSummaryProjection> {
        if !matches!(status, GoalStatus::Draft | GoalStatus::Backlog) {
            return Err(RefineError::InvalidInput(
                "New Goals must start in Draft or Backlog".into(),
            ));
        }
        let priority = GoalPriority::parse_wire(priority.unwrap_or("low"))
            .ok_or_else(|| RefineError::InvalidInput("Invalid priority".into()))?;
        if let Some(reporter) = reporter {
            Self::validate_goal_reporter(reporter)?;
        }
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
        if goal_id.len() < 3 || !crate::model::automation::valid_id(&goal_id) {
            return Err(RefineError::InvalidInput(
                "Goal id must be 3 to 120 letters, digits, dots, underscores or hyphens"
                    .to_string(),
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
        object.insert("status".to_string(), json!(status));
        object.insert("priority".to_string(), json!(priority));
        object.insert("reporter".to_string(), json!(reporter));
        object.insert(
            "description".to_string(),
            json!(description.unwrap_or_default()),
        );
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
        if let Some(historical) = historical {
            for key in ["created", "updated", "planning_origin"] {
                if let Some(value) = historical.get(key).filter(|value| !value.is_null()) {
                    object.insert(key.into(), value.clone());
                }
            }
        }
        let value = Value::Object(object);
        if let Some(reporter) = reporter {
            self.with_goal_reporter_registered(reporter, || {
                write_json_atomically(&goal_path, &value)
            })?;
        } else {
            write_json_atomically(&goal_path, &value)?;
        }
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
        let planning_path = self
            .refine_dir
            .join("planning/cards")
            .join(format!("{goal_id}.json"));
        if planning_path.exists() {
            let placement: Value =
                crate::infrastructure::storage::automation::read_json(&planning_path)?;
            let board_id = placement["board_id"]
                .as_str()
                .filter(|id| crate::model::automation::valid_id(id));
            let deleted = if let Some(board_id) = board_id {
                let board_path = self
                    .refine_dir
                    .join("planning/boards")
                    .join(format!("{board_id}.json"));
                if board_path.exists() {
                    crate::infrastructure::storage::automation::read_json::<Value>(&board_path)?["deleted"].as_bool().unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            };
            if !deleted {
                object.insert("planning".into(), placement);
            }
        }
        self.attach_round_logs(goal_id, object)?;
        Ok(value)
    }

    /// Read authored Goal records once, without loading logs or changing workflow state.
    pub(crate) fn metrics_goal_records(&self) -> RefineResult<Vec<Value>> {
        let snapshot = self.projection_snapshot()?;
        snapshot
            .goals
            .values()
            .map(|goal| {
                let (_lock, _, record) = self.read_goal_value_unchecked(goal)?;
                Ok(serde_json::json!({
                    "status":record["status"], "created":record["created"], "updated":record["updated"],
                    "node_id":record["node_id"], "reporter":record["reporter"],
                    "rounds":record["rounds"].as_array().into_iter().flatten().map(|round| serde_json::json!({
                        "created":round["created"], "workflow_integration":{"integrated_at":round["workflow_integration"]["integrated_at"]}
                    })).collect::<Vec<_>>()
                }))
            })
            .collect()
    }

    pub fn list_goal_summaries(&self) -> RefineResult<Vec<GoalSummaryProjection>> {
        let snapshot = self.projection_snapshot()?;
        Ok(snapshot.goals.values().cloned().collect())
    }
}

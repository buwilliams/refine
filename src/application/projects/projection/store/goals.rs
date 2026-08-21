use super::*;

impl FileProjectProjectionStore {
    pub(crate) fn project_goal(&self, path: &Path) -> RefineResult<Option<GoalSummaryProjection>> {
        let value = Self::read_json(path)?;
        let Some(object) = value.as_object() else {
            return Ok(None);
        };
        let id = text(object.get("id")).unwrap_or_default();
        if id.is_empty() {
            return Ok(None);
        }
        let rel_path = self.relative_path(path)?;
        let rounds = object
            .get("rounds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let valid_rounds: Vec<&Value> = rounds.iter().filter(|round| round.is_object()).collect();
        let reporter = goal_reporter(object, &valid_rounds);
        let assignee = latest_round_assignee(object, &valid_rounds);
        // Match the legacy detail-based duplicate decision exactly: inspect
        // only the literal final array entry, require an object and a string
        // prompt, then trim it.
        let latest_round_prompt = rounds
            .last()
            .and_then(Value::as_object)
            .and_then(|round| round.get("prompt"))
            .and_then(Value::as_str)
            .map(|prompt| prompt.trim().to_string());
        // Only a prefix stays resident. Notes and Round prompts are unbounded
        // and accumulate for as long as a Goal is worked, so carrying them in
        // full made this the largest per-Goal cost in the projection. Queries
        // the prefix cannot decide fall through to reading the record.
        let searchable_text = resident_search_text(&goal_searchable_parts(object).join("\n"));

        Ok(Some(GoalSummaryProjection {
            goal: GoalIndexProjection {
                id,
                name: text(object.get("name")).unwrap_or_else(|| "Untitled Goal".to_string()),
                status: goal_status(object),
                priority: goal_priority(object.get("priority")),
                reporter,
                assignee,
                round_count: valid_rounds.len(),
                created: text(object.get("created")).unwrap_or_else(|| "unknown".to_string()),
                updated: text(object.get("updated"))
                    .or_else(|| text(object.get("created")))
                    .unwrap_or_else(|| "unknown".to_string()),
                branch_name: nullable_text(object.get("branch_name")),
                node_id: Some(
                    nullable_text(object.get("node_id"))
                        .or_else(|| nullable_text(object.get("instance_id")))
                        .unwrap_or_else(|| "default".to_string()),
                ),
                feature_id: nullable_text(object.get("feature_id")),
                feature_order: nullable_i64(object.get("feature_order")),
                json_path: rel_path,
                mission: object
                    .get("mission")
                    .and_then(Value::as_object)
                    .and_then(|mission| {
                        let mission_id = mission.get("mission_id")?.as_str()?;
                        let mission_goal_key = mission.get("mission_goal_key")?.as_str()?;
                        Some(crate::model::mission::MissionGoalBinding {
                            mission_id: mission_id.to_string(),
                            mission_goal_key: mission_goal_key.to_string(),
                        })
                    }),
            },
            node_display_name: None,
            latest_round_prompt,
            searchable_text,
            activity_ids: Vec::new(),
        }))
    }
}

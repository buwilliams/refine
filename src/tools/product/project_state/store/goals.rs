use super::*;

impl FileProjectStateStore {
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
        let notes = object
            .get("notes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut searchable_parts = vec![
            text(object.get("name")).unwrap_or_else(|| "Untitled Goal".to_string()),
            reporter.clone().unwrap_or_default(),
            assignee.clone().unwrap_or_default(),
        ];
        for note in notes.iter().filter_map(Value::as_object) {
            if let Some(body) = text(note.get("body")) {
                searchable_parts.push(body);
            }
        }
        for round in &valid_rounds {
            if let Some(round) = round.as_object() {
                for key in ["reporter", "assignee", "prompt"] {
                    if let Some(value) = text(round.get(key)) {
                        searchable_parts.push(value);
                    }
                }
            }
        }

        Ok(Some(GoalSummaryProjection {
            goal: GoalIndexProjection {
                id,
                name: text(object.get("name")).unwrap_or_else(|| "Untitled Goal".to_string()),
                status: goal_status(object.get("status")),
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
            },
            node_display_name: None,
            latest_round_prompt,
            searchable_text: searchable_parts.join("\n"),
            activity_ids: Vec::new(),
        }))
    }
}

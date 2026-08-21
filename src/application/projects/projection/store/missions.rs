use super::*;

impl FileProjectProjectionStore {
    pub(super) fn project_mission(
        &self,
        path: &Path,
    ) -> RefineResult<Option<MissionIndexProjection>> {
        let value = Self::read_json(path)?;
        let Some(object) = value.as_object() else {
            return Ok(None);
        };
        let id = text(object.get("id")).unwrap_or_default();
        if id.is_empty() {
            return Ok(None);
        }
        let status = object
            .get("status")
            .and_then(Value::as_str)
            .and_then(MissionStatus::parse_wire)
            .unwrap_or(MissionStatus::Draft);
        let current_round = object
            .get("current_round")
            .and_then(Value::as_u64)
            .map(|round| round as usize);
        let current_wave = current_round
            .and_then(|round| {
                object
                    .get("rounds")
                    .and_then(Value::as_array)
                    .and_then(|rounds| {
                        rounds
                            .iter()
                            .find(|r| r.get("number").and_then(Value::as_u64) == Some(round as u64))
                    })
            })
            .and_then(|round| round.get("plan"))
            .and_then(|plan| plan.get("waves"))
            .and_then(Value::as_array)
            .and_then(|waves| waves.first())
            .and_then(|wave| wave.get("number"))
            .and_then(Value::as_u64)
            .map(|number| number as usize);
        let criteria_summary = mission_criteria_summary(object, current_round);
        let outcome_available =
            object
                .get("rounds")
                .and_then(Value::as_array)
                .is_some_and(|rounds| {
                    rounds.iter().any(|round| {
                        round
                            .get("outcome")
                            .is_some_and(|outcome| !outcome.is_null())
                    })
                });
        Ok(Some(MissionIndexProjection {
            id,
            name: text(object.get("name")).unwrap_or_else(|| "Untitled Mission".to_string()),
            status,
            reporter: nullable_text(object.get("reporter")),
            assignee: nullable_text(object.get("assignee")),
            coordinator_node_id: nullable_text(object.get("coordinator_node_id")),
            current_round,
            current_wave,
            criteria_summary,
            outcome_available,
            created: text(object.get("created")).unwrap_or_else(|| "unknown".to_string()),
            updated: text(object.get("updated"))
                .or_else(|| text(object.get("created")))
                .unwrap_or_else(|| "unknown".to_string()),
            json_path: self.relative_path(path)?,
        }))
    }
}

fn mission_criteria_summary(
    object: &serde_json::Map<String, Value>,
    current_round: Option<usize>,
) -> MissionCriteriaSummary {
    let total = object
        .get("success_criteria")
        .and_then(Value::as_array)
        .map(|criteria| criteria.len())
        .unwrap_or(0);
    let mut summary = MissionCriteriaSummary {
        total,
        ..MissionCriteriaSummary::default()
    };
    if let Some(round) = current_round
        && let Some(review) = object
            .get("rounds")
            .and_then(Value::as_array)
            .and_then(|rounds| {
                rounds
                    .iter()
                    .find(|r| r.get("number").and_then(Value::as_u64) == Some(round as u64))
            })
            .and_then(|round| round.get("review"))
            .and_then(|review| review.get("criteria_results"))
            .and_then(Value::as_array)
    {
        for result in review {
            match result.get("result").and_then(Value::as_str) {
                Some("met") => summary.met += 1,
                Some("partial") => summary.partial += 1,
                Some("unmet") => summary.unmet += 1,
                Some("contradicted") => summary.contradicted += 1,
                Some("waived") => summary.waived += 1,
                _ => {}
            }
        }
    }
    summary
}

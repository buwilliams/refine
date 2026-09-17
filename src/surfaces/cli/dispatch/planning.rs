use super::*;
pub(super) fn planning(action: PlanningCliAction) -> RefineResult<()> {
    let value = match action {
        PlanningCliAction::List => daemon_json("GET", "/planning", None)?,
        PlanningCliAction::Action { id } => daemon_json(
            "GET",
            &format!("/planning/actions/{}", path_segment(&id)),
            None,
        )?,
        PlanningCliAction::Cancel { id } => daemon_json(
            "POST",
            &format!("/planning/actions/{}/cancel", path_segment(&id)),
            Some(json!({})),
        )?,
        PlanningCliAction::Apply {
            operation,
            request_id,
            board_id,
            lane_id,
            goal_id,
            expected_revision,
            actor,
            data,
        } => {
            let data: Value = serde_json::from_str(&data)
                .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
            daemon_json(
                "POST",
                "/planning/commands",
                Some(
                    json!({"operation":operation,"request_id":request_id,"board_id":board_id,"lane_id":lane_id,"goal_id":goal_id,"expected_revision":expected_revision,"actor":actor,"data":data}),
                ),
            )?
        }
    };
    print_json(&value);
    Ok(())
}

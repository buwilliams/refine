use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Workflow { action } => dispatch_workflow_daemon(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn dispatch_workflow_daemon(action: WorkflowAction) -> RefineResult<()> {
    let response = match action {
        WorkflowAction::Show { id } => {
            daemon_json("GET", &format!("/workflow/goals/{}", id), None)?
        }
        WorkflowAction::Move {
            id,
            to,
            reason,
            context_file,
            expected_revision,
            request_id,
            force,
            actor,
            invocation_id,
        } => {
            let context = context_file
                .map(fs::read_to_string)
                .transpose()
                .map_err(|e| RefineError::Io(e.to_string()))?
                .unwrap_or_default();
            daemon_json(
                "POST",
                &format!("/workflow/goals/{id}/move"),
                Some(
                    json!({"to":GoalStatus::from(to),"reason":reason,"context":context,"expected_revision":expected_revision,"request_id":request_id,"force":force,"actor":actor,"invocation_id":invocation_id}),
                ),
            )?
        }
        WorkflowAction::Integrate {
            id,
            reason,
            expected_revision,
            request_id,
            force,
            actor,
        } => daemon_json(
            "POST",
            &format!("/workflow/goals/{id}/integrate"),
            Some(
                json!({"to":"governance","reason":reason,"expected_revision":expected_revision,"request_id":request_id,"force":force,"actor":actor}),
            ),
        )?,
        WorkflowAction::Pause { .. } => {
            daemon_json("POST", "/workflow/pause", Some(json!({ "paused": true })))?
        }
        WorkflowAction::Resume { .. } => {
            daemon_json("POST", "/workflow/pause", Some(json!({ "paused": false })))?
        }
    };
    print_json(&response);
    Ok(())
}

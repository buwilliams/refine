use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Workflow { action } => dispatch_workflow_daemon(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn dispatch_workflow_daemon(action: WorkflowAction) -> RefineResult<()> {
    let response = match action {
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

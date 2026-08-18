use super::{
    ApiResponse, FileOperationRegistry, InProcessWebServer, OperationState, PathBuf, RefineError,
    ToolbarGoalAgentAttachmentStatus, error_response, json, operation_response,
    queue_toolbar_goal_agent_attachment, thread, toolbar_goal_agent_attachment_status,
};
use crate::application::agents::sessions::AgentSessionSnapshot;

const SETTLEMENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const SETTLEMENT_POLL: std::time::Duration = std::time::Duration::from_millis(10);

impl InProcessWebServer {
    pub(super) fn queue_toolbar_goal_agent_attachment(
        &self,
        runtime_root: PathBuf,
        goal_id: &str,
        snapshot: AgentSessionSnapshot,
    ) -> ApiResponse {
        let attachment = match queue_toolbar_goal_agent_attachment(&runtime_root, &snapshot.id) {
            Ok(attachment) => attachment,
            Err(error) => return error_response(error),
        };
        if attachment.process_id != snapshot.process_id {
            return error_response(RefineError::Conflict(format!(
                "Goal Agent session {} changed while Toolbar attachment was queued",
                snapshot.id
            )));
        }
        let registry = FileOperationRegistry::new(&runtime_root);
        let operation = match registry.register_with_id(
            &attachment.acknowledgment_id,
            "terminal:toolbar-goal-agent-attachment",
            json!({
                "goal_id": goal_id,
                "session_id": attachment.session_id,
                "process_id": attachment.process_id,
                "acknowledgment_id": attachment.acknowledgment_id
            }),
        ) {
            Ok(operation) => operation,
            Err(error) => return error_response(error),
        };
        let operation_id = operation.id.clone();
        thread::spawn(move || {
            let registry = FileOperationRegistry::new(&runtime_root);
            let deadline = std::time::Instant::now() + SETTLEMENT_TIMEOUT;
            loop {
                match toolbar_goal_agent_attachment_status(&runtime_root, &attachment) {
                    Ok(ToolbarGoalAgentAttachmentStatus::Acknowledged(snapshot)) => {
                        match serde_json::to_value(snapshot) {
                            Ok(result) => {
                                let _ = registry.finish_with_result(
                                    &operation_id,
                                    OperationState::Succeeded,
                                    result,
                                );
                            }
                            Err(error) => {
                                let _ = registry.fail_with_error(
                                    &operation_id,
                                    json!({
                                        "code": "toolbar_attachment_unavailable",
                                        "message": format!(
                                            "failed to encode acknowledged Goal Agent session: {error}"
                                        )
                                    }),
                                );
                            }
                        }
                        return;
                    }
                    Ok(ToolbarGoalAgentAttachmentStatus::Pending) => {}
                    Err(error) => {
                        let _ = registry.fail_with_error(
                            &operation_id,
                            json!({
                                "code": "toolbar_attachment_conflict",
                                "message": error.to_string()
                            }),
                        );
                        return;
                    }
                }
                if std::time::Instant::now() >= deadline {
                    let _ = registry.fail_with_error(
                        &operation_id,
                        json!({
                            "code": "toolbar_attachment_unavailable",
                            "message": format!(
                                "Goal Agent session {} did not acknowledge Toolbar attachment",
                                attachment.session_id
                            )
                        }),
                    );
                    return;
                }
                thread::sleep(SETTLEMENT_POLL);
            }
        });
        ApiResponse::json(202, json!({"operation": operation_response(operation)}))
    }
}

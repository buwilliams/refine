use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessOwner, ProcessSupervisor,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::product::chat::{ChatAttachment, ChatSessionRecord, FileChatService};
use crate::tools::product::project_registry::FileProjectRegistryService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::{WorkflowClaimState, WorkflowEngine};

const DEFAULT_AGENT_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
struct WorkflowGoalOwnership {
    process_id: String,
    claim_id: String,
    execution_id: String,
    round_idx: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkflowOwnershipPhase {
    BeforeTermination,
    BeforeCancellation,
}

/// Authoritative process-stop capability.
///
/// Agent records are resolved across the port and nested agent registries, terminated with exact
/// PID identity confirmation, and only then allowed to close linked chat state or cancel a Goal.
/// Surfaces adapt this one result rather than composing process and workflow mutations themselves.
#[derive(Clone, Debug)]
pub struct FileProcessControlService {
    runtime_root: PathBuf,
    refine_dir: Option<PathBuf>,
    agent_exit_timeout: Duration,
}

impl FileProcessControlService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: None,
            agent_exit_timeout: DEFAULT_AGENT_EXIT_TIMEOUT,
        }
    }

    pub fn with_refine_dir(
        runtime_root: impl Into<PathBuf>,
        refine_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: Some(refine_dir.into()),
            agent_exit_timeout: DEFAULT_AGENT_EXIT_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_agent_exit_timeout(mut self, timeout: Duration) -> Self {
        self.agent_exit_timeout = timeout;
        self
    }

    pub fn stop(&self, process_id: &str, signal: &str) -> RefineResult<Value> {
        validate_process_id(process_id)?;
        if !matches!(signal, "stop" | "terminate" | "kill") {
            return Err(RefineError::InvalidInput(format!(
                "unsupported termination signal {signal}"
            )));
        }
        if let Some((supervisor, process)) = self.find_managed_process(process_id)? {
            if is_agent_process(&process) {
                return self.stop_managed_agent(supervisor, process, signal);
            }
            let mut stopped = supervisor.signal(process_id, signal)?;
            stopped.state = "stopped".to_string();
            return Ok(json!({
                "stopped": true,
                "process": stopped.api_json()
            }));
        }
        if let Some(session_id) = process_id.strip_prefix("chat-session-") {
            return self.stop_synthetic_chat(process_id, session_id, signal);
        }
        Err(RefineError::NotFound(format!(
            "Process {process_id} was not found"
        )))
    }

    fn stop_managed_agent(
        &self,
        supervisor: FileProcessSupervisor,
        process: ManagedProcess,
        signal: &str,
    ) -> RefineResult<Value> {
        let process_value = process.api_json();
        let goal_id = process_value
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let chat_session_id = (process_value.get("kind").and_then(Value::as_str) == Some("chat"))
            .then(|| {
                process_value
                    .get("session_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .flatten();
        let refine_dir = if goal_id.is_some() || chat_session_id.is_some() {
            Some(self.resolve_refine_dir()?)
        } else {
            None
        };
        let workflow_ownership = match (refine_dir.as_deref(), goal_id.as_deref()) {
            (Some(refine_dir), Some(goal_id)) => preflight_goal_for_process(
                refine_dir,
                &self.runtime_root,
                goal_id,
                &process,
                WorkflowOwnershipPhase::BeforeTermination,
            )?,
            _ => None,
        };
        if let (Some(refine_dir), Some(session_id)) =
            (refine_dir.as_deref(), chat_session_id.as_deref())
        {
            preflight_chat(refine_dir, &self.runtime_root, session_id)?;
        }

        let termination = supervisor
            .terminate_owned_and_confirm_exit(&process, signal, self.agent_exit_timeout)
            .map_err(|error| {
                stop_failure_with_goal_context(error, &process.id, goal_id.as_deref())
            })?;

        if let (Some(refine_dir), Some(session_id)) =
            (refine_dir.as_deref(), chat_session_id.as_deref())
        {
            FileChatService::with_runtime_root(refine_dir, &self.runtime_root).stop(session_id)?;
        }
        let goal = match (refine_dir.as_deref(), goal_id.as_deref()) {
            (Some(refine_dir), Some(goal_id)) => {
                validate_goal_cancellation(
                    refine_dir,
                    &self.runtime_root,
                    goal_id,
                    workflow_ownership.as_ref(),
                )?;
                Some(
                    FileWorkItemService::new(refine_dir)
                        .cancel_goal_summary(goal_id)?
                        .goal,
                )
            }
            _ => None,
        };

        let mut stopped_process = process;
        stopped_process.state = "stopped".to_string();
        let mut result = json!({
            "stopped": true,
            "process": stopped_process.api_json(),
            "termination": termination
        });
        if let Some(goal) = goal
            && let Some(object) = result.as_object_mut()
        {
            object.insert("goal".to_string(), json!(goal));
        }
        Ok(result)
    }

    fn stop_synthetic_chat(
        &self,
        process_id: &str,
        session_id: &str,
        signal: &str,
    ) -> RefineResult<Value> {
        let refine_dir = self.resolve_refine_dir()?;
        let chat = FileChatService::with_runtime_root(&refine_dir, &self.runtime_root);
        let session = chat
            .list_sessions()?
            .into_iter()
            .find(|session| session.id == session_id && !session.closed)
            .ok_or_else(|| RefineError::NotFound(format!("Process {process_id} was not found")))?;
        let goal_id = match &session.attachment {
            ChatAttachment::Goal(goal_id) => Some(goal_id.clone()),
            _ => None,
        };
        if let Some(goal_id) = goal_id.as_deref() {
            preflight_goal_state(&refine_dir, goal_id)?;
        }

        let managed = self.managed_processes_for_session(session_id)?;
        if managed.is_empty() && (session.in_flight || session.queue_dispatching) {
            return Err(stop_failure_with_goal_context(
                RefineError::Degraded(format!(
                    "chat agent process {process_id} reports active work but has no exact managed-process identity to terminate; the chat record was kept open for recovery"
                )),
                process_id,
                goal_id.as_deref(),
            ));
        }
        if managed.is_empty()
            && let Some(goal_id) = goal_id.as_deref()
        {
            ensure_goal_has_no_active_workflow_claim(&self.runtime_root, goal_id, process_id)?;
        }
        let mut workflow_ownership = Vec::new();
        if let Some(goal_id) = goal_id.as_deref() {
            for (_, process) in &managed {
                if let Some(ownership) = preflight_goal_for_process(
                    &refine_dir,
                    &self.runtime_root,
                    goal_id,
                    process,
                    WorkflowOwnershipPhase::BeforeTermination,
                )? {
                    workflow_ownership.push(ownership);
                }
            }
        }
        let mut terminations = Vec::new();
        for (supervisor, process) in managed {
            terminations.push(
                supervisor
                    .terminate_owned_and_confirm_exit(&process, signal, self.agent_exit_timeout)
                    .map_err(|error| {
                        stop_failure_with_goal_context(error, process_id, goal_id.as_deref())
                    })?,
            );
        }
        let stopped_session = chat.stop(session_id)?;
        let goal = match goal_id.as_deref() {
            Some(goal_id) => {
                preflight_goal_state(&refine_dir, goal_id)?;
                for ownership in &workflow_ownership {
                    validate_workflow_goal_ownership(
                        &self.runtime_root,
                        goal_id,
                        ownership,
                        WorkflowOwnershipPhase::BeforeCancellation,
                    )?;
                    validate_goal_round(&refine_dir, goal_id, ownership)?;
                }
                Some(
                    FileWorkItemService::new(&refine_dir)
                        .cancel_goal_summary(goal_id)?
                        .goal,
                )
            }
            None => None,
        };
        let already_idle = terminations.is_empty();
        let mut result = json!({
            "stopped": true,
            "process": synthetic_chat_process_value(process_id, &stopped_session),
            "termination": {
                "confirmed_exit": true,
                "registry_retained_until_exit": true,
                "managed_processes": terminations,
                "already_idle": already_idle
            }
        });
        if let Some(goal) = goal
            && let Some(object) = result.as_object_mut()
        {
            object.insert("goal".to_string(), json!(goal));
        }
        Ok(result)
    }

    fn find_managed_process(
        &self,
        process_id: &str,
    ) -> RefineResult<Option<(FileProcessSupervisor, ManagedProcess)>> {
        for root in managed_process_roots(&self.runtime_root) {
            let supervisor = FileProcessSupervisor::new(root);
            match supervisor.inspect(process_id) {
                Ok(process) => return Ok(Some((supervisor, process))),
                Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(None)
    }

    fn managed_processes_for_session(
        &self,
        session_id: &str,
    ) -> RefineResult<Vec<(FileProcessSupervisor, ManagedProcess)>> {
        let mut matches = Vec::new();
        for root in managed_process_roots(&self.runtime_root) {
            let supervisor = FileProcessSupervisor::new(root);
            for process in supervisor.list()? {
                if process_metadata(&process)
                    .get("session_id")
                    .and_then(Value::as_str)
                    == Some(session_id)
                {
                    matches.push((supervisor.clone(), process));
                }
            }
        }
        Ok(matches)
    }

    fn resolve_refine_dir(&self) -> RefineResult<PathBuf> {
        if let Some(refine_dir) = &self.refine_dir {
            return Ok(refine_dir.clone());
        }
        let registry = FileProjectRegistryService::new(&self.runtime_root, None).load()?;
        let target_root = registry
            .active_app
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                RefineError::Degraded(
                    "cannot stop a Goal-linked agent because the runtime has no active app; process and Goal state were left unchanged"
                        .to_string(),
                )
            })?;
        refine_dir_for_target_root(Path::new(&target_root))
    }
}

fn managed_process_roots(runtime_root: &Path) -> [PathBuf; 2] {
    [runtime_root.to_path_buf(), runtime_root.join("agents")]
}

fn process_metadata(process: &ManagedProcess) -> Map<String, Value> {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| details.as_object().cloned())
        .unwrap_or_default()
}

fn is_agent_process(process: &ManagedProcess) -> bool {
    if process.owner == ProcessOwner::Agent {
        return true;
    }
    let value = process.api_json();
    matches!(
        value.get("kind").and_then(Value::as_str),
        Some("agent" | "chat")
    ) || (value.get("kind").and_then(Value::as_str) == Some("interactive_session")
        && value.get("provider").and_then(Value::as_str).is_some())
}

fn preflight_goal_state(refine_dir: &Path, goal_id: &str) -> RefineResult<()> {
    let goal = FileWorkItemService::new(refine_dir).show_goal_summary(goal_id)?;
    if goal.goal.status == GoalStatus::Done {
        return Err(RefineError::InvalidInput(format!(
            "done Goal {goal_id} cannot be cancelled; its linked process was left running"
        )));
    }
    Ok(())
}

fn preflight_goal_for_process(
    refine_dir: &Path,
    runtime_root: &Path,
    goal_id: &str,
    process: &ManagedProcess,
    phase: WorkflowOwnershipPhase,
) -> RefineResult<Option<WorkflowGoalOwnership>> {
    preflight_goal_state(refine_dir, goal_id)?;
    let metadata = process_metadata(process);
    let has_workflow_identity = ["claim_id", "execution_id"]
        .iter()
        .any(|field| metadata.contains_key(*field));
    let state = WorkflowEngine::new(runtime_root).load_state()?;
    if !has_workflow_identity {
        ensure_goal_has_no_active_workflow_claim(runtime_root, goal_id, &process.id)?;
        return Ok(None);
    }

    let execution_id = metadata
        .get("execution_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "managed process {} has incomplete workflow ownership: execution_id is required; termination was not requested",
                process.id
            ))
        })?
        .to_string();
    let round_idx = metadata
        .get("round_idx")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "managed process {} has incomplete workflow ownership: round_idx is required; termination was not requested",
                process.id
            ))
        })?;
    let recorded_claim_id = metadata
        .get("claim_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let claim = match recorded_claim_id {
        Some(claim_id) => state
            .claims
            .iter()
            .find(|claim| claim.claim_id == claim_id),
        None => state
            .claims
            .iter()
            .find(|claim| claim.execution_id.as_deref() == Some(execution_id.as_str())),
    }
    .ok_or_else(|| {
        RefineError::Conflict(format!(
            "managed process {} no longer has a matching workflow claim for execution {execution_id}; termination was not requested and Goal {goal_id} remains non-cancelled",
            process.id
        ))
    })?;
    let ownership = WorkflowGoalOwnership {
        process_id: process.id.clone(),
        claim_id: claim.claim_id.clone(),
        execution_id,
        round_idx,
    };
    validate_workflow_goal_ownership(runtime_root, goal_id, &ownership, phase)?;
    validate_goal_round(refine_dir, goal_id, &ownership)?;
    Ok(Some(ownership))
}

fn ensure_goal_has_no_active_workflow_claim(
    runtime_root: &Path,
    goal_id: &str,
    process_id: &str,
) -> RefineResult<()> {
    let state = WorkflowEngine::new(runtime_root).load_state()?;
    if state.claims.iter().any(|claim| {
        claim.goal_id == goal_id
            && matches!(
                claim.state,
                WorkflowClaimState::Claimed | WorkflowClaimState::Running
            )
    }) {
        return Err(RefineError::Conflict(format!(
            "managed process {process_id} has no workflow execution ownership, but Goal {goal_id} has an active claim; termination was not requested and the Goal remains non-cancelled"
        )));
    }
    Ok(())
}

fn validate_goal_cancellation(
    refine_dir: &Path,
    runtime_root: &Path,
    goal_id: &str,
    ownership: Option<&WorkflowGoalOwnership>,
) -> RefineResult<()> {
    preflight_goal_state(refine_dir, goal_id)?;
    if let Some(ownership) = ownership {
        validate_workflow_goal_ownership(
            runtime_root,
            goal_id,
            ownership,
            WorkflowOwnershipPhase::BeforeCancellation,
        )?;
        validate_goal_round(refine_dir, goal_id, ownership)?;
    }
    Ok(())
}

fn validate_workflow_goal_ownership(
    runtime_root: &Path,
    goal_id: &str,
    ownership: &WorkflowGoalOwnership,
    phase: WorkflowOwnershipPhase,
) -> RefineResult<()> {
    let state = WorkflowEngine::new(runtime_root).load_state()?;
    let claim = state
        .claims
        .iter()
        .find(|claim| claim.claim_id == ownership.claim_id)
        .ok_or_else(|| {
            stale_workflow_ownership(goal_id, ownership, "claim is no longer present")
        })?;
    if claim.goal_id != goal_id
        || claim.execution_id.as_deref() != Some(ownership.execution_id.as_str())
    {
        return Err(stale_workflow_ownership(
            goal_id,
            ownership,
            "claim identity or execution changed",
        ));
    }
    let competing_active_claim = state.claims.iter().any(|candidate| {
        candidate.goal_id == goal_id
            && candidate.claim_id != ownership.claim_id
            && matches!(
                candidate.state,
                WorkflowClaimState::Claimed | WorkflowClaimState::Running
            )
    });
    if competing_active_claim {
        return Err(stale_workflow_ownership(
            goal_id,
            ownership,
            "a newer workflow claim owns the Goal",
        ));
    }
    if phase == WorkflowOwnershipPhase::BeforeTermination
        && claim.state != WorkflowClaimState::Running
    {
        return Err(stale_workflow_ownership(
            goal_id,
            ownership,
            "the recorded workflow claim is not running",
        ));
    }
    Ok(())
}

fn validate_goal_round(
    refine_dir: &Path,
    goal_id: &str,
    ownership: &WorkflowGoalOwnership,
) -> RefineResult<()> {
    let goal = FileWorkItemService::new(refine_dir).show_goal_summary(goal_id)?;
    if goal.goal.round_count != ownership.round_idx.saturating_add(1) {
        return Err(stale_workflow_ownership(
            goal_id,
            ownership,
            &format!(
                "process round {} is not the current Goal round {}",
                ownership.round_idx + 1,
                goal.goal.round_count
            ),
        ));
    }
    Ok(())
}

fn stale_workflow_ownership(
    goal_id: &str,
    ownership: &WorkflowGoalOwnership,
    reason: &str,
) -> RefineError {
    RefineError::Conflict(format!(
        "managed process {} has stale workflow ownership for Goal {goal_id} (claim {}, execution {}, round {}): {reason}; termination was not requested and the Goal remains non-cancelled",
        ownership.process_id,
        ownership.claim_id,
        ownership.execution_id,
        ownership.round_idx + 1
    ))
}

fn preflight_chat(
    refine_dir: &Path,
    runtime_root: &Path,
    session_id: &str,
) -> RefineResult<ChatSessionRecord> {
    FileChatService::with_runtime_root(refine_dir, runtime_root)
        .list_sessions()?
        .into_iter()
        .find(|session| session.id == session_id && !session.closed)
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "chat session {session_id} is unavailable; its managed process was left running"
            ))
        })
}

fn synthetic_chat_process_value(process_id: &str, session: &ChatSessionRecord) -> Value {
    let goal_id = match &session.attachment {
        ChatAttachment::Goal(goal_id) => Some(goal_id.as_str()),
        _ => None,
    };
    json!({
        "id": process_id,
        "kind": "chat",
        "session_id": session.id,
        "goal_id": goal_id,
        "status": "stopped",
        "pid": null
    })
}

fn stop_failure_with_goal_context(
    error: RefineError,
    process_id: &str,
    goal_id: Option<&str>,
) -> RefineError {
    let goal_context = goal_id
        .map(|goal_id| format!("; linked Goal {goal_id} remains non-cancelled"))
        .unwrap_or_default();
    let message = format!("{error}{goal_context}; retry process {process_id} after recovery");
    match error {
        RefineError::InvalidInput(_) => RefineError::InvalidInput(message),
        RefineError::NotFound(_) => RefineError::NotFound(message),
        RefineError::Unauthorized(_) => RefineError::Unauthorized(message),
        RefineError::Conflict(_) => RefineError::Conflict(message),
        RefineError::Degraded(_) => RefineError::Degraded(message),
        RefineError::Io(_) => RefineError::Io(message),
        RefineError::Serialization(_) => RefineError::Serialization(message),
        RefineError::NotImplemented(_) => RefineError::NotImplemented(message),
    }
}

fn validate_process_id(process_id: &str) -> RefineResult<()> {
    if process_id.trim().is_empty() || process_id.contains('/') || process_id.contains('\\') {
        return Err(RefineError::InvalidInput(
            "process id is required and cannot contain path separators".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;
    use crate::process::subprocess::{ManagedProcessSpec, managed_pid_is_alive};

    #[test]
    fn confirmed_agent_exit_precedes_linked_goal_cancellation() {
        let temp_root = unique_temp_dir("process-control-confirmed");
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        create_in_progress_goal(&refine_dir, "GOAL-CONFIRMED");
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_agent(&supervisor, "GOAL-CONFIRMED", None);
        let pid = process.pid.unwrap();

        let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .stop(&process.id, "terminate")
            .unwrap();

        assert_eq!(result["stopped"], true);
        assert_eq!(result["termination"]["confirmed_exit"], true);
        assert_eq!(result["termination"]["registry_retained_until_exit"], true);
        assert!(!managed_pid_is_alive(pid).unwrap());
        assert!(supervisor.inspect(&process.id).is_err());
        assert_eq!(result["goal"]["status"], "cancelled");
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary("GOAL-CONFIRMED")
                .unwrap()
                .goal
                .status,
            GoalStatus::Cancelled
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resistant_agent_retains_process_evidence_and_goal_state() {
        let temp_root = unique_temp_dir("process-control-resistant");
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        create_in_progress_goal(&refine_dir, "GOAL-RESIST");
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_agent(
            &supervisor,
            "GOAL-RESIST",
            Some(("sh", vec!["-c", "trap '' TERM; while :; do sleep 1; done"])),
        );

        let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .with_agent_exit_timeout(Duration::from_millis(100))
            .stop(&process.id, "terminate")
            .unwrap_err();

        assert!(matches!(error, RefineError::Degraded(_)), "{error}");
        assert!(
            error
                .to_string()
                .contains("identity evidence were retained")
        );
        assert!(error.to_string().contains("remains non-cancelled"));
        assert!(supervisor.inspect(&process.id).is_ok());
        assert!(
            runtime_root
                .join("agents/process-identities")
                .join(format!("{}.json", process.id))
                .exists()
        );
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary("GOAL-RESIST")
                .unwrap()
                .goal
                .status,
            GoalStatus::InProgress
        );

        supervisor.request_termination(&process.id, "kill").unwrap();
        wait_for_exit(process.pid.unwrap());
        let _ = supervisor.recover();
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pid_identity_mismatch_never_signals_or_cancels() {
        let temp_root = unique_temp_dir("process-control-identity");
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        create_in_progress_goal(&refine_dir, "GOAL-IDENTITY");
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_agent(&supervisor, "GOAL-IDENTITY", None);
        let identity_path = runtime_root
            .join("agents/process-identities")
            .join(format!("{}.json", process.id));
        let mut identity: Value =
            serde_json::from_slice(&fs::read(&identity_path).unwrap()).unwrap();
        identity["os_identity"] = json!("linux:different-boot:different-start");
        fs::write(
            &identity_path,
            serde_json::to_vec_pretty(&identity).unwrap(),
        )
        .unwrap();

        let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .stop(&process.id, "terminate")
            .unwrap_err();

        assert!(matches!(error, RefineError::Conflict(_)), "{error}");
        assert!(error.to_string().contains("PID identity mismatch"));
        assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
        assert!(supervisor.inspect(&process.id).is_ok());
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary("GOAL-IDENTITY")
                .unwrap()
                .goal
                .status,
            GoalStatus::InProgress
        );

        Command::new("kill")
            .args(["-KILL", &process.pid.unwrap().to_string()])
            .status()
            .unwrap();
        wait_for_exit(process.pid.unwrap());
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn legacy_missing_identity_never_adopts_current_pid_or_cancels() {
        let temp_root = unique_temp_dir("process-control-legacy-identity");
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        create_in_progress_goal(&refine_dir, "GOAL-LEGACY-IDENTITY");
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_agent(&supervisor, "GOAL-LEGACY-IDENTITY", None);
        let identity_path = runtime_root
            .join("agents/process-identities")
            .join(format!("{}.json", process.id));
        fs::remove_file(&identity_path).unwrap();
        let registration_error = supervisor.register(process.clone()).unwrap_err();
        assert!(
            registration_error
                .to_string()
                .contains("no registration-time PID identity evidence")
        );

        let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .stop(&process.id, "terminate")
            .unwrap_err();

        assert!(matches!(error, RefineError::Conflict(_)), "{error}");
        assert!(
            error
                .to_string()
                .contains("no registration-time PID identity evidence")
        );
        assert!(
            error
                .to_string()
                .contains("recorded PID may have been reused")
        );
        assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
        assert!(supervisor.inspect(&process.id).is_ok());
        assert!(
            !identity_path.exists(),
            "stop-time control must not invent identity evidence"
        );
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary("GOAL-LEGACY-IDENTITY")
                .unwrap()
                .goal
                .status,
            GoalStatus::InProgress
        );

        force_kill(process.pid.unwrap());
        wait_for_exit(process.pid.unwrap());
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn current_workflow_execution_can_stop_and_cancel_its_goal() {
        let temp_root = unique_temp_dir("process-control-current-execution");
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        create_in_progress_goal_with_rounds(&refine_dir, "GOAL-CURRENT", 1);
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_workflow_agent(
            &supervisor,
            "GOAL-CURRENT",
            "claim-current",
            "exec-current",
            0,
        );
        write_workflow_state(
            &runtime_root,
            json!([{
                "claim_id": "claim-current",
                "goal_id": "GOAL-CURRENT",
                "execution_id": "exec-current",
                "state": "running",
                "created_at": "2026-07-23T00:00:00Z",
                "updated_at": "2026-07-23T00:00:00Z"
            }]),
        );

        let result = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .stop(&process.id, "terminate")
            .unwrap();

        assert_eq!(result["termination"]["confirmed_exit"], true);
        assert!(!managed_pid_is_alive(process.pid.unwrap()).unwrap());
        assert_eq!(result["goal"]["status"], "cancelled");
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn stale_execution_and_round_never_stop_or_cancel_newer_goal_work() {
        let temp_root = unique_temp_dir("process-control-stale-execution");
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        create_in_progress_goal_with_rounds(&refine_dir, "GOAL-STALE", 2);
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_workflow_agent(&supervisor, "GOAL-STALE", "claim-old", "exec-old", 0);
        write_workflow_state(
            &runtime_root,
            json!([
                {
                    "claim_id": "claim-old",
                    "goal_id": "GOAL-STALE",
                    "execution_id": "exec-old",
                    "state": "failed",
                    "created_at": "2026-07-23T00:00:00Z",
                    "updated_at": "2026-07-23T00:01:00Z"
                },
                {
                    "claim_id": "claim-new",
                    "goal_id": "GOAL-STALE",
                    "execution_id": "exec-new",
                    "state": "running",
                    "created_at": "2026-07-23T00:02:00Z",
                    "updated_at": "2026-07-23T00:02:00Z"
                }
            ]),
        );

        let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .stop(&process.id, "terminate")
            .unwrap_err();

        assert!(matches!(error, RefineError::Conflict(_)), "{error}");
        assert!(error.to_string().contains("stale workflow ownership"));
        assert!(error.to_string().contains("newer workflow claim"));
        assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
        assert!(supervisor.inspect(&process.id).is_ok());
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary("GOAL-STALE")
                .unwrap()
                .goal
                .status,
            GoalStatus::InProgress
        );

        force_kill(process.pid.unwrap());
        wait_for_exit(process.pid.unwrap());
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn current_execution_with_stale_round_never_stops_or_cancels() {
        let temp_root = unique_temp_dir("process-control-stale-round");
        let runtime_root = temp_root.join("run/8080");
        let refine_dir = temp_root.join(".refine");
        create_in_progress_goal_with_rounds(&refine_dir, "GOAL-STALE-ROUND", 2);
        let supervisor = FileProcessSupervisor::new(runtime_root.join("agents"));
        let process = launch_workflow_agent(
            &supervisor,
            "GOAL-STALE-ROUND",
            "claim-current",
            "exec-current",
            0,
        );
        write_workflow_state(
            &runtime_root,
            json!([{
                "claim_id": "claim-current",
                "goal_id": "GOAL-STALE-ROUND",
                "execution_id": "exec-current",
                "state": "running",
                "created_at": "2026-07-23T00:02:00Z",
                "updated_at": "2026-07-23T00:02:00Z"
            }]),
        );

        let error = FileProcessControlService::with_refine_dir(&runtime_root, &refine_dir)
            .stop(&process.id, "terminate")
            .unwrap_err();

        assert!(matches!(error, RefineError::Conflict(_)), "{error}");
        assert!(
            error
                .to_string()
                .contains("process round 1 is not the current Goal round 2")
        );
        assert!(managed_pid_is_alive(process.pid.unwrap()).unwrap());
        assert!(supervisor.inspect(&process.id).is_ok());
        assert_eq!(
            FileWorkItemService::new(&refine_dir)
                .show_goal_summary("GOAL-STALE-ROUND")
                .unwrap()
                .goal
                .status,
            GoalStatus::InProgress
        );

        force_kill(process.pid.unwrap());
        wait_for_exit(process.pid.unwrap());
        fs::remove_dir_all(temp_root).unwrap();
    }

    fn launch_agent(
        supervisor: &FileProcessSupervisor,
        goal_id: &str,
        command: Option<(&str, Vec<&str>)>,
    ) -> ManagedProcess {
        launch_agent_with_metadata(supervisor, goal_id, command, Map::new())
    }

    fn launch_workflow_agent(
        supervisor: &FileProcessSupervisor,
        goal_id: &str,
        claim_id: &str,
        execution_id: &str,
        round_idx: usize,
    ) -> ManagedProcess {
        launch_agent_with_metadata(
            supervisor,
            goal_id,
            None,
            Map::from_iter([
                ("claim_id".to_string(), json!(claim_id)),
                ("execution_id".to_string(), json!(execution_id)),
                ("round_idx".to_string(), json!(round_idx)),
                ("workflow_state".to_string(), json!("in-progress")),
            ]),
        )
    }

    fn launch_agent_with_metadata(
        supervisor: &FileProcessSupervisor,
        goal_id: &str,
        command: Option<(&str, Vec<&str>)>,
        extra_metadata: Map<String, Value>,
    ) -> ManagedProcess {
        let (command, args) = command
            .map(|(command, args)| {
                (
                    command.to_string(),
                    args.into_iter().map(str::to_string).collect(),
                )
            })
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    (
                        "cmd".to_string(),
                        vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()],
                    )
                } else {
                    ("sleep".to_string(), vec!["30".to_string()])
                }
            });
        let mut metadata = Map::from_iter([
            ("kind".to_string(), json!("interactive_session")),
            ("provider".to_string(), json!("smoke-ai")),
            ("goal_id".to_string(), json!(goal_id)),
        ]);
        metadata.extend(extra_metadata);
        supervisor
            .launch(ManagedProcessSpec {
                owner: ProcessOwner::Agent,
                command,
                args,
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: None,
                authorization_command: None,
                sensitive: false,
                metadata,
            })
            .unwrap()
    }

    fn create_in_progress_goal(refine_dir: &Path, goal_id: &str) {
        let service = FileWorkItemService::new(refine_dir);
        service
            .create_goal_summary("Process control test", Some(goal_id))
            .unwrap();
        service
            .transition_goal_status(goal_id, GoalStatus::Todo)
            .unwrap();
        service
            .advance_automated_goal_status(goal_id, GoalStatus::InProgress)
            .unwrap();
    }

    fn create_in_progress_goal_with_rounds(refine_dir: &Path, goal_id: &str, rounds: usize) {
        let service = FileWorkItemService::new(refine_dir);
        service
            .create_goal_summary("Process control workflow test", Some(goal_id))
            .unwrap();
        for round in 0..rounds {
            service
                .append_goal_round_summary(
                    goal_id,
                    "Process Control",
                    &format!("Round {}", round + 1),
                )
                .unwrap();
        }
        service
            .transition_goal_status(goal_id, GoalStatus::Todo)
            .unwrap();
        service
            .advance_automated_goal_status(goal_id, GoalStatus::InProgress)
            .unwrap();
    }

    fn write_workflow_state(runtime_root: &Path, claims: Value) {
        fs::create_dir_all(runtime_root).unwrap();
        fs::write(
            runtime_root.join(crate::workflow::WORKFLOW_AUTOMATION_STATE_FILE),
            serde_json::to_vec_pretty(&json!({
                "paused": [],
                "claims": claims,
                "updated_at": "2026-07-23T00:02:00Z"
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn wait_for_exit(pid: u32) {
        for _ in 0..100 {
            if !managed_pid_is_alive(pid).unwrap_or(false) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn force_kill(pid: u32) {
        #[cfg(windows)]
        Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status()
            .unwrap();
        #[cfg(not(windows))]
        Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}

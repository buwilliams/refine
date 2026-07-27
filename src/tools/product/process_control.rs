use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::model::workflow::GoalStatus;
#[cfg(test)]
use crate::process::subprocess::ProcessCleanupStage;
use crate::process::subprocess::{
    ConfirmedProcessExit, FileProcessSupervisor, ManagedProcess, ProcessOwner, ProcessSupervisor,
    acquire_workflow_process_registration_lock,
};
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::FileOperationRegistry;
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::product::chat::{ChatAttachment, ChatSessionRecord, FileChatService};
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_registry::FileProjectRegistryService;
#[cfg(test)]
use crate::tools::product::work_items::workflow_revision;
use crate::tools::product::work_items::{
    BulkGoalSelection, BulkSkippedDetail, BulkUpdateResult, FileWorkItemService,
    GoalCancellationExpectation,
};
use crate::workflow::capacity::AgentCapacityState;
use crate::workflow::{WorkflowAutomationState, WorkflowClaimState, WorkflowEngine};

const DEFAULT_AGENT_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

mod bulk_cancellation;

#[derive(Clone, Debug)]
struct WorkflowGoalOwnership {
    process_id: String,
    claim_id: String,
    execution_id: Option<String>,
    round_idx: Option<usize>,
}

#[derive(Clone, Debug)]
struct ProcessGoalFence {
    goal: GoalCancellationExpectation,
    workflow: Option<WorkflowGoalOwnership>,
}

#[derive(Clone, Debug)]
struct RecoveredWorkflowTermination {
    ownership: WorkflowGoalOwnership,
    termination: ConfirmedProcessExit,
    worktree: Option<WorkflowWorktree>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkflowOwnershipPhase {
    BeforeTermination,
    BeforeCancellation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancellationSettlementFailureStage {
    AfterClaimPersistence,
    AfterCapacityRelease,
    AfterGoalPersistence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancellationRollbackFailureStage {
    CapacityRestore,
    ClaimRestore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DurableReceiptBoundary {
    FileSyncedBeforeRename,
    RenamedBeforeDirectorySync,
    DirectorySynced,
}

/// Authoritative process-stop capability.
///
/// Agent records are resolved across the port and nested agent registries, terminated with exact
/// PID identity confirmation, and only then allowed to close linked chat state or settle a Goal.
/// Surfaces adapt this one result rather than composing process and workflow mutations themselves.
#[derive(Clone)]
pub struct FileProcessControlService {
    runtime_root: PathBuf,
    refine_dir: Option<PathBuf>,
    agent_exit_timeout: Duration,
    #[cfg(test)]
    settlement_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    post_exit_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    cleanup_failure: Option<ProcessCleanupStage>,
    #[cfg(test)]
    settlement_failure: Option<CancellationSettlementFailureStage>,
    #[cfg(test)]
    settlement_interruption: Option<CancellationSettlementFailureStage>,
    #[cfg(test)]
    rollback_failure: Option<CancellationRollbackFailureStage>,
    #[cfg(test)]
    after_bulk_goal_selection_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl FileProcessControlService {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: None,
            agent_exit_timeout: DEFAULT_AGENT_EXIT_TIMEOUT,
            #[cfg(test)]
            settlement_hook: None,
            #[cfg(test)]
            post_exit_hook: None,
            #[cfg(test)]
            cleanup_failure: None,
            #[cfg(test)]
            settlement_failure: None,
            #[cfg(test)]
            settlement_interruption: None,
            #[cfg(test)]
            rollback_failure: None,
            #[cfg(test)]
            after_bulk_goal_selection_hook: None,
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
            #[cfg(test)]
            settlement_hook: None,
            #[cfg(test)]
            post_exit_hook: None,
            #[cfg(test)]
            cleanup_failure: None,
            #[cfg(test)]
            settlement_failure: None,
            #[cfg(test)]
            settlement_interruption: None,
            #[cfg(test)]
            rollback_failure: None,
            #[cfg(test)]
            after_bulk_goal_selection_hook: None,
        }
    }

    #[cfg(test)]
    fn with_agent_exit_timeout(mut self, timeout: Duration) -> Self {
        self.agent_exit_timeout = timeout;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_settlement_hook(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.settlement_hook = Some(std::sync::Arc::new(hook));
        self
    }

    #[cfg(test)]
    pub(crate) fn with_post_exit_hook(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.post_exit_hook = Some(std::sync::Arc::new(hook));
        self
    }

    #[cfg(test)]
    fn with_cleanup_failure(mut self, stage: ProcessCleanupStage) -> Self {
        self.cleanup_failure = Some(stage);
        self
    }

    #[cfg(test)]
    fn with_settlement_failure(mut self, stage: CancellationSettlementFailureStage) -> Self {
        self.settlement_failure = Some(stage);
        self
    }

    #[cfg(test)]
    fn with_settlement_interruption(mut self, stage: CancellationSettlementFailureStage) -> Self {
        self.settlement_interruption = Some(stage);
        self
    }

    #[cfg(test)]
    fn with_rollback_failure(mut self, stage: CancellationRollbackFailureStage) -> Self {
        self.rollback_failure = Some(stage);
        self
    }

    #[cfg(test)]
    fn with_after_bulk_goal_selection_hook(
        mut self,
        hook: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.after_bulk_goal_selection_hook = Some(std::sync::Arc::new(hook));
        self
    }
}

fn workflow_ownership_json(ownership: &WorkflowGoalOwnership) -> Value {
    json!({
        "process_id": ownership.process_id,
        "claim_id": ownership.claim_id,
        "execution_id": ownership.execution_id,
        "round_idx": ownership.round_idx
    })
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

fn preflight_goal_state(
    refine_dir: &Path,
    goal_id: &str,
) -> RefineResult<GoalCancellationExpectation> {
    let goal = FileWorkItemService::new(refine_dir).show_goal_summary(goal_id)?;
    if goal.goal.status == GoalStatus::Done {
        return Err(RefineError::InvalidInput(format!(
            "done Goal {goal_id} cannot be settled by process control; its linked process was left running"
        )));
    }
    Ok(GoalCancellationExpectation {
        status: goal.goal.status,
        round_count: goal.goal.round_count,
        updated: goal.goal.updated,
        node_id: goal
            .goal
            .node_id
            .filter(|node_id| !node_id.is_empty())
            .unwrap_or_else(|| "default".to_string()),
    })
}

fn preflight_goal_for_process(
    refine_dir: &Path,
    runtime_root: &Path,
    goal_id: &str,
    process: &ManagedProcess,
    phase: WorkflowOwnershipPhase,
) -> RefineResult<ProcessGoalFence> {
    let goal = preflight_goal_state(refine_dir, goal_id)?;
    let metadata = process_metadata(process);
    let has_workflow_identity = ["claim_id", "execution_id"]
        .iter()
        .any(|field| metadata.contains_key(*field));
    let state = WorkflowEngine::new(runtime_root).load_state()?;
    if !has_workflow_identity {
        ensure_goal_has_no_active_workflow_claim(runtime_root, goal_id, &process.id)?;
        return Ok(ProcessGoalFence {
            goal,
            workflow: None,
        });
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
        execution_id: Some(execution_id),
        round_idx: Some(round_idx),
    };
    validate_workflow_goal_ownership(runtime_root, goal_id, &ownership, phase)?;
    validate_expected_goal_round(&goal, goal_id, &ownership, phase)?;
    Ok(ProcessGoalFence {
        goal,
        workflow: Some(ownership),
    })
}

fn ensure_goal_has_no_active_workflow_claim(
    runtime_root: &Path,
    goal_id: &str,
    process_id: &str,
) -> RefineResult<()> {
    let state = WorkflowEngine::new(runtime_root).load_state()?;
    ensure_goal_has_no_active_workflow_claim_in_state(
        &state,
        goal_id,
        process_id,
        WorkflowOwnershipPhase::BeforeTermination,
    )
}

fn ensure_goal_has_no_active_workflow_claim_in_state(
    state: &WorkflowAutomationState,
    goal_id: &str,
    process_id: &str,
    phase: WorkflowOwnershipPhase,
) -> RefineResult<()> {
    if state.claims.iter().any(|claim| {
        claim.goal_id == goal_id
            && matches!(
                claim.state,
                WorkflowClaimState::Claimed | WorkflowClaimState::Running
            )
    }) {
        let outcome = match phase {
            WorkflowOwnershipPhase::BeforeTermination => {
                "termination was not requested and the Goal remains non-cancelled"
            }
            WorkflowOwnershipPhase::BeforeCancellation => {
                "the process exit is confirmed, but the Goal remains non-cancelled"
            }
        };
        return Err(RefineError::Conflict(format!(
            "managed process {process_id} has no workflow execution ownership, but Goal {goal_id} has an active competing claim; {outcome}"
        )));
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
    validate_workflow_goal_ownership_in_state(&state, goal_id, ownership, phase)
}

fn validate_workflow_goal_ownership_in_state(
    state: &WorkflowAutomationState,
    goal_id: &str,
    ownership: &WorkflowGoalOwnership,
    phase: WorkflowOwnershipPhase,
) -> RefineResult<()> {
    let claim = state
        .claims
        .iter()
        .find(|claim| claim.claim_id == ownership.claim_id)
        .ok_or_else(|| {
            stale_workflow_ownership(goal_id, ownership, "claim is no longer present", phase)
        })?;
    if claim.goal_id != goal_id || claim.execution_id != ownership.execution_id {
        return Err(stale_workflow_ownership(
            goal_id,
            ownership,
            "claim identity or execution changed",
            phase,
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
            phase,
        ));
    }
    if phase == WorkflowOwnershipPhase::BeforeTermination
        && claim.state != WorkflowClaimState::Running
    {
        return Err(stale_workflow_ownership(
            goal_id,
            ownership,
            "the recorded workflow claim is not running",
            phase,
        ));
    }
    Ok(())
}

fn validate_expected_goal_round(
    goal: &GoalCancellationExpectation,
    goal_id: &str,
    ownership: &WorkflowGoalOwnership,
    phase: WorkflowOwnershipPhase,
) -> RefineResult<()> {
    let Some(round_idx) = ownership.round_idx else {
        return Ok(());
    };
    if goal.round_count != round_idx.saturating_add(1) {
        return Err(stale_workflow_ownership(
            goal_id,
            ownership,
            &format!(
                "process round {} is not the current Goal round {}",
                round_idx + 1,
                goal.round_count
            ),
            phase,
        ));
    }
    Ok(())
}

fn stale_workflow_ownership(
    goal_id: &str,
    ownership: &WorkflowGoalOwnership,
    reason: &str,
    phase: WorkflowOwnershipPhase,
) -> RefineError {
    let outcome = match phase {
        WorkflowOwnershipPhase::BeforeTermination => {
            "termination was not requested and the Goal remains non-cancelled"
        }
        WorkflowOwnershipPhase::BeforeCancellation => {
            "the process exit is confirmed, but the Goal remains non-cancelled"
        }
    };
    RefineError::Conflict(format!(
        "managed process {} has stale workflow ownership for Goal {goal_id} (claim {}, execution {}, round {}): {reason}; {outcome}",
        ownership.process_id,
        ownership.claim_id,
        ownership.execution_id.as_deref().unwrap_or("not-started"),
        ownership
            .round_idx
            .map(|round_idx| (round_idx + 1).to_string())
            .unwrap_or_else(|| "unrecorded".to_string())
    ))
}

fn workflow_claim_state_label(state: &WorkflowClaimState) -> &'static str {
    match state {
        WorkflowClaimState::Claimed => "claimed",
        WorkflowClaimState::Running => "running",
        WorkflowClaimState::Completed => "completed",
        WorkflowClaimState::Failed => "failed",
        WorkflowClaimState::Cancelled => "cancelled",
        WorkflowClaimState::Interrupted => "interrupted",
    }
}

#[cfg(test)]
fn cancellation_settlement_stage_label(stage: CancellationSettlementFailureStage) -> &'static str {
    match stage {
        CancellationSettlementFailureStage::AfterClaimPersistence => "claim persistence",
        CancellationSettlementFailureStage::AfterCapacityRelease => "capacity release",
        CancellationSettlementFailureStage::AfterGoalPersistence => "Goal persistence",
    }
}

fn cancellation_settlement_recovery(state: &str) -> &'static str {
    match state {
        "committed" => {
            "Goal, claim, capacity, and receipts are settled; all workflow worktrees and branches remain available for inspection, commit, preservation, or a separate explicit human-controlled cleanup operation"
        }
        "rolled_back" => {
            "the exact pre-settlement Goal, claim, capacity, workflow policy, and target context were restored; retry the same explicit termination intent through the shared Process capability after resolving the cause"
        }
        _ => {
            "retry the same explicit termination intent through the shared Process capability; it will replay this journal before any terminal-state shortcut and deterministically finish the exact Goal, claim, capacity, workflow policy, target context, and retained-worktree evidence"
        }
    }
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
    error_with_message(error, message)
}

fn error_with_message(error: RefineError, message: String) -> RefineError {
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

fn termination_outcome_flag(termination: &Value, key: &str) -> bool {
    if let Some(value) = termination.get(key).and_then(Value::as_bool) {
        return value;
    }
    termination
        .get("managed_processes")
        .and_then(Value::as_array)
        .is_some_and(|processes| {
            processes
                .iter()
                .all(|process| process.get(key).and_then(Value::as_bool) == Some(true))
        })
}

fn write_json_receipt(path: &Path, value: &Value) -> RefineResult<()> {
    write_json_receipt_with_boundary(path, value, |_| Ok(()))
}

fn write_json_receipt_with_boundary(
    path: &Path,
    value: &Value,
    mut boundary: impl FnMut(DurableReceiptBoundary) -> RefineResult<()>,
) -> RefineResult<()> {
    let parent = path.parent().ok_or_else(|| {
        RefineError::InvalidInput(format!(
            "partial process-stop receipt {} has no parent",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        RefineError::Io(format!(
            "failed to create partial process-stop receipt directory {}: {error}",
            parent.display()
        ))
    })?;
    let encoded = serde_json::to_vec_pretty(value).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode partial process-stop receipt: {error}"
        ))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("process-stop-receipt");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> RefineResult<()> {
        let mut temp = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to create partial process-stop receipt {}: {error}",
                    temp_path.display()
                ))
            })?;
        temp.write_all(&encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write partial process-stop receipt {}: {error}",
                temp_path.display()
            ))
        })?;
        temp.sync_all().map_err(|error| {
            RefineError::Io(format!(
                "failed to sync partial process-stop receipt {}: {error}",
                temp_path.display()
            ))
        })?;
        boundary(DurableReceiptBoundary::FileSyncedBeforeRename)
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    fs::rename(&temp_path, path).map_err(|error| {
        let _ = fs::remove_file(&temp_path);
        RefineError::Io(format!(
            "failed to commit partial process-stop receipt {}: {error}",
            path.display()
        ))
    })?;
    boundary(DurableReceiptBoundary::RenamedBeforeDirectorySync)?;
    sync_receipt_directory(parent).map_err(|error| {
        RefineError::Io(format!(
            "failed to sync partial process-stop receipt directory {}: {error}",
            parent.display()
        ))
    })?;
    boundary(DurableReceiptBoundary::DirectorySynced)
}

#[cfg(unix)]
fn sync_receipt_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_receipt_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn validate_process_id(process_id: &str) -> RefineResult<()> {
    if process_id.trim().is_empty() || process_id.contains('/') || process_id.contains('\\') {
        return Err(RefineError::InvalidInput(
            "process id is required and cannot contain path separators".to_string(),
        ));
    }
    Ok(())
}

mod discovery;
mod receipts;
mod settlement;
mod stop_intent;
mod termination;
mod termination_recovery;
#[cfg(test)]
mod tests;

#[cfg(test)]
use settlement::CancellationSettlementJournal;
use stop_intent::*;

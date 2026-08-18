use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use fs2::FileExt;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::application::agent_io::prompts::{PromptTemplate, render};
use crate::application::agent_io::structured_output::MAX_INVALID_SIGNAL_REPLACEMENTS;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::{
    HostAgentProviderService, ProviderSessionContinuity,
};
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
    ProcessSupervisor, signal_os_process,
};
use crate::model::goal::ImplementationExecutionEvidence;

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(40);
const SIGNAL_WRITE_GRACE_PERIOD: Duration = Duration::from_secs(2);
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 36;
const MAX_INPUT_BYTES: usize = 16_000;
const MAX_EVENT_BYTES: usize = 64 * 1024;
const TOOLBAR_ATTACHMENT_ACK_TIMEOUT: Duration = Duration::from_secs(2);
const TOOLBAR_TIMEOUT_PROTECTED_KEY: &str = "toolbar_timeout_protected";
const TOOLBAR_ATTACHMENT_ACKS_KEY: &str = "toolbar_attachment_acknowledgments";

#[derive(Clone, Debug)]
pub struct GoalAgentLaunch {
    pub runtime_root: PathBuf,
    pub cwd: PathBuf,
    pub provider: String,
    pub prompt: String,
    pub metadata: Map<String, Value>,
    pub completion_timeout: Option<Duration>,
    /// Fail the session after this long without any PTY output, attached
    /// input, or signal activity — unless the agent has signalled that it is
    /// waiting for user input. A stalled agent then fails in minutes with its
    /// transcript preserved instead of silently burning the whole
    /// `completion_timeout`.
    pub idle_timeout: Option<Duration>,
    /// Pin or resume a provider-native session so consecutive workflow steps
    /// can share one provider context instead of re-reading the repository.
    pub provider_session: Option<ProviderSessionContinuity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalAgentResult {
    pub output: String,
    pub session_id: String,
    pub process_id: String,
    pub guidance_applied: Option<Vec<usize>>,
    pub implementation_evidence: Option<ImplementationExecutionEvidence>,
    pub planning_result: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalAgentSettlement {
    pub output: String,
    pub process_id: String,
    pub session_id: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub guidance_applied: Option<Vec<usize>>,
    pub implementation_evidence: Option<ImplementationExecutionEvidence>,
    pub planning_result: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalAgentAttention {
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSessionSnapshot {
    pub id: String,
    pub process_id: String,
    pub profile: String,
    pub provider: Option<String>,
    pub cwd: String,
    pub goal_id: Option<String>,
    pub worktree: Option<Value>,
    pub attention_state: Option<String>,
    pub attention_message: Option<String>,
    #[serde(default)]
    pub toolbar_timeout_protected: bool,
    #[serde(default)]
    pub transcript_bytes: u64,
    pub alive: bool,
    pub exited: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AgentSessionCommand {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
    ToolbarAttach { acknowledgment_id: String },
}

#[derive(Clone, Debug, Deserialize)]
struct AgentSessionSignal {
    state: AgentSessionState,
    #[serde(default)]
    message: String,
    #[serde(default)]
    guidance_applied: Option<Vec<usize>>,
    #[serde(default)]
    implementation_evidence: Option<ImplementationExecutionEvidence>,
    #[serde(default)]
    planning_result: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
enum AgentSessionState {
    #[serde(rename = "completed", alias = "complete")]
    Completed,
    #[serde(rename = "needs_input", alias = "waiting_for_user")]
    NeedsInput,
}

mod codec;
mod session_runtime;
mod signal_recovery;

#[cfg(test)]
use session_runtime::{pump_pty_output, run_goal_agent_session, transcript_capture_failure};
pub use session_runtime::{run_goal_agent, run_goal_agent_with_settlement};

use codec::*;
use signal_recovery::*;

pub fn find_goal_agent_session(
    runtime_root: &Path,
    goal_id: &str,
) -> RefineResult<AgentSessionSnapshot> {
    let goal_id = goal_id.trim();
    if goal_id.is_empty() {
        return Err(RefineError::InvalidInput(
            "Goal id is required to open its agent".to_string(),
        ));
    }
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let process = supervisor
        .list()?
        .into_iter()
        .filter_map(|process| {
            let metadata = process_metadata(&process)?;
            let matches = metadata.get("kind").and_then(Value::as_str)
                == Some("interactive_session")
                && metadata.get("profile").and_then(Value::as_str) == Some("goal")
                && metadata.get("goal_id").and_then(Value::as_str) == Some(goal_id)
                && metadata
                    .get("command_path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| Path::new(path).is_file())
                && FileProcessSupervisor::process_is_alive(&process).unwrap_or(false);
            matches.then_some((process, metadata))
        })
        .max_by(|(left, _), (right, _)| left.started_at.cmp(&right.started_at))
        .ok_or_else(|| {
            RefineError::NotFound(format!(
                "Goal {goal_id} does not have a running Goal Agent. Start or restart its workflow, then open the agent while implementation is active."
            ))
        })?;
    snapshot_from_process(&process.0, &process.1)
}

pub fn find_agent_session(
    runtime_root: &Path,
    session_id: &str,
) -> RefineResult<AgentSessionSnapshot> {
    let (process, metadata) = session_process(runtime_root, session_id)?;
    snapshot_from_process(&process, &metadata)
}

/// Protect an exact live workflow Goal Agent from automatic watchdog
/// termination after a Toolbar open has been acknowledged by its runtime.
///
/// The session runtime owns the one-way state transition. The caller never
/// treats appending the request as success: it waits for the matching identity
/// in the same still-live process record, so an exit or a losing deadline race
/// cannot be reported as an attached Toolbar session.
pub fn attach_toolbar_goal_agent_session(
    runtime_root: &Path,
    session_id: &str,
) -> RefineResult<AgentSessionSnapshot> {
    attach_toolbar_goal_agent_session_with_timeout(
        runtime_root,
        session_id,
        TOOLBAR_ATTACHMENT_ACK_TIMEOUT,
    )
}

fn attach_toolbar_goal_agent_session_with_timeout(
    runtime_root: &Path,
    session_id: &str,
    acknowledgment_timeout: Duration,
) -> RefineResult<AgentSessionSnapshot> {
    let attachment_ended = || {
        RefineError::Conflict(format!(
            "Goal Agent session {session_id} exited before acknowledging Toolbar attachment"
        ))
    };
    let (expected_process, metadata) =
        session_process(runtime_root, session_id).map_err(|error| match error {
            RefineError::NotFound(_) => attachment_ended(),
            other => other,
        })?;
    if metadata.get("profile").and_then(Value::as_str) != Some("goal") {
        return Err(RefineError::Conflict(format!(
            "terminal session {session_id} is not a workflow Goal Agent"
        )));
    }
    if !FileProcessSupervisor::process_is_alive(&expected_process)? {
        return Err(RefineError::Conflict(format!(
            "Goal Agent session {session_id} exited before Toolbar attachment"
        )));
    }

    let acknowledgment_id = Uuid::new_v4().to_string();
    append_command(
        runtime_root,
        session_id,
        AgentSessionCommand::ToolbarAttach {
            acknowledgment_id: acknowledgment_id.clone(),
        },
    )
    .map_err(|error| match error {
        RefineError::NotFound(_) => attachment_ended(),
        other => other,
    })?;

    let deadline = std::time::Instant::now() + acknowledgment_timeout;
    loop {
        let (process, metadata) =
            session_process(runtime_root, session_id).map_err(|error| match error {
                RefineError::NotFound(_) => attachment_ended(),
                other => other,
            })?;
        if process.id != expected_process.id {
            return Err(RefineError::Conflict(format!(
                "Goal Agent session {session_id} changed while Toolbar attachment was pending"
            )));
        }
        if !FileProcessSupervisor::process_is_alive(&process)? {
            return Err(RefineError::Conflict(format!(
                "Goal Agent session {session_id} exited before acknowledging Toolbar attachment"
            )));
        }
        let acknowledged = metadata
            .get(TOOLBAR_ATTACHMENT_ACKS_KEY)
            .and_then(Value::as_array)
            .is_some_and(|acknowledgments| {
                acknowledgments
                    .iter()
                    .any(|value| value.as_str() == Some(&acknowledgment_id))
            });
        if acknowledged
            && metadata
                .get(TOOLBAR_TIMEOUT_PROTECTED_KEY)
                .and_then(Value::as_bool)
                == Some(true)
        {
            let snapshot = snapshot_from_process(&process, &metadata)?;
            if !snapshot.alive {
                return Err(attachment_ended());
            }
            return Ok(snapshot);
        }
        if std::time::Instant::now() >= deadline {
            return Err(RefineError::Degraded(format!(
                "Goal Agent session {session_id} did not acknowledge Toolbar attachment"
            )));
        }
        thread::sleep(COMMAND_POLL_INTERVAL);
    }
}

pub fn send_agent_session_input(
    runtime_root: &Path,
    session_id: &str,
    data: &str,
) -> RefineResult<()> {
    if data.len() > MAX_INPUT_BYTES {
        return Err(RefineError::InvalidInput(format!(
            "terminal input is limited to {MAX_INPUT_BYTES} bytes"
        )));
    }
    append_command(
        runtime_root,
        session_id,
        AgentSessionCommand::Input {
            data: data.to_string(),
        },
    )
}

pub fn resize_agent_session(
    runtime_root: &Path,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> RefineResult<()> {
    append_command(
        runtime_root,
        session_id,
        AgentSessionCommand::Resize { cols, rows },
    )
}

pub fn agent_session_events_since(
    runtime_root: &Path,
    session_id: &str,
    after: u64,
) -> RefineResult<Vec<Value>> {
    agent_session_events_range(runtime_root, session_id, after, None)
}

pub fn agent_session_events_range(
    runtime_root: &Path,
    session_id: &str,
    after: u64,
    before: Option<u64>,
) -> RefineResult<Vec<Value>> {
    let (process, _) = session_process(runtime_root, session_id)?;
    let path = process
        .stdout_path
        .as_deref()
        .map(Path::new)
        .ok_or_else(|| RefineError::NotFound("Goal Agent transcript is unavailable".to_string()))?;
    let mut file = fs::File::open(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read Goal Agent transcript {}: {error}",
            path.display()
        ))
    })?;
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let end = before.unwrap_or(length).min(length);
    let start = after.min(end);
    file.seek(SeekFrom::Start(start)).map_err(|error| {
        RefineError::Io(format!(
            "failed to seek Goal Agent transcript {}: {error}",
            path.display()
        ))
    })?;
    let mut bytes = Vec::new();
    file.take((end - start).min(MAX_EVENT_BYTES as u64))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to stream Goal Agent transcript {}: {error}",
                path.display()
            ))
        })?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let seq = start + bytes.len() as u64;
    Ok(vec![json!({
        "seq": seq,
        "event": "terminal_output",
        "data": String::from_utf8_lossy(&bytes)
    })])
}

#[cfg(test)]
mod attachment_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod tests;

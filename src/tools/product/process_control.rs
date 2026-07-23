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

const DEFAULT_AGENT_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

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
        if let (Some(refine_dir), Some(goal_id)) = (refine_dir.as_deref(), goal_id.as_deref()) {
            preflight_goal(refine_dir, goal_id)?;
        }
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
            (Some(refine_dir), Some(goal_id)) => Some(
                FileWorkItemService::new(refine_dir)
                    .cancel_goal_summary(goal_id)?
                    .goal,
            ),
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
            preflight_goal(&refine_dir, goal_id)?;
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
            Some(goal_id) => Some(
                FileWorkItemService::new(&refine_dir)
                    .cancel_goal_summary(goal_id)?
                    .goal,
            ),
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

fn preflight_goal(refine_dir: &Path, goal_id: &str) -> RefineResult<()> {
    let goal = FileWorkItemService::new(refine_dir).show_goal_summary(goal_id)?;
    if goal.goal.status == GoalStatus::Done {
        return Err(RefineError::InvalidInput(format!(
            "done Goal {goal_id} cannot be cancelled; its linked process was left running"
        )));
    }
    Ok(())
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

    fn launch_agent(
        supervisor: &FileProcessSupervisor,
        goal_id: &str,
        command: Option<(&str, Vec<&str>)>,
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
                metadata: Map::from_iter([
                    ("kind".to_string(), json!("interactive_session")),
                    ("provider".to_string(), json!("smoke-ai")),
                    ("goal_id".to_string(), json!(goal_id)),
                ]),
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

    fn wait_for_exit(pid: u32) {
        for _ in 0..100 {
            if !managed_pid_is_alive(pid).unwrap_or(false) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::Timestamp;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{FileProcessSupervisor, ManagedProcess, ProcessOwner};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptTemplate, render};
use crate::tools::product::chat::{
    ChatAttachment, ChatService, ChatSessionRecord, FileChatService,
};

pub const SUPERVISOR_AGENT_STATE_FILE: &str = "supervisor-agent.json";
const SUPERVISOR_AGENT_LOCK_FILE: &str = "supervisor-agent.lock";
const DEFAULT_STALL_SECONDS: i64 = 15 * 60;
const EVENT_LIMIT: usize = 250;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SupervisorAgentEvent {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub message: String,
    pub created_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub actionable: bool,
    #[serde(default)]
    pub retryable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SupervisorAgentSnapshot {
    pub version: u64,
    pub lifecycle: String,
    pub health: String,
    pub active_work: usize,
    pub queued_work: usize,
    pub failed_work: usize,
    pub supervisor_process: String,
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default)]
    pub initialized: bool,
    #[serde(default)]
    pub goal_states: BTreeMap<String, String>,
    #[serde(default)]
    pub open_issues: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_context_key: String,
    pub updated_at: Timestamp,
    pub events: Vec<SupervisorAgentEvent>,
}

impl Default for SupervisorAgentSnapshot {
    fn default() -> Self {
        Self {
            version: 2,
            lifecycle: "idle".to_string(),
            health: "healthy".to_string(),
            active_work: 0,
            queued_work: 0,
            failed_work: 0,
            supervisor_process: "ordinary CLI agent via the shared provider/process path"
                .to_string(),
            session_id: None,
            provider: None,
            initialized: false,
            goal_states: BTreeMap::new(),
            open_issues: BTreeMap::new(),
            last_context_key: String::new(),
            updated_at: Utc::now().to_rfc3339(),
            events: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FileSupervisorAgentService {
    pub refine_dir: PathBuf,
    pub runtime_root: PathBuf,
}

#[derive(Clone, Debug)]
struct GoalObservation {
    id: String,
    status: GoalStatus,
    updated: Timestamp,
}

#[derive(Clone, Debug, Default)]
struct ProcessEvidence {
    live_goal_ids: BTreeSet<String>,
    quiet_goal_ids: BTreeSet<String>,
    supervisor_sessions: BTreeSet<String>,
}

impl FileSupervisorAgentService {
    pub fn new(refine_dir: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
        }
    }

    pub fn snapshot(&self) -> RefineResult<SupervisorAgentSnapshot> {
        let path = self.state_path();
        if !path.exists() {
            return Ok(SupervisorAgentSnapshot::default());
        }
        let bytes = fs::read_to_string(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read supervisor agent state {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_str(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse supervisor agent state {}: {error}",
                path.display()
            ))
        })
    }

    /// Reconcile the durable cross-surface projection and coordinate one ordinary CLI-agent
    /// session for the current active-work window.
    pub fn reconcile(&self) -> RefineResult<SupervisorAgentSnapshot> {
        self.reconcile_at(Utc::now(), self.configured_stall_seconds())
    }

    pub fn reconcile_with_stall_seconds(
        &self,
        stall_seconds: u64,
    ) -> RefineResult<SupervisorAgentSnapshot> {
        self.reconcile_at(Utc::now(), stall_seconds.max(1) as i64)
    }

    pub fn record_failure(
        &self,
        scope: &str,
        message: impl Into<String>,
        retryable: bool,
    ) -> RefineResult<SupervisorAgentSnapshot> {
        let scope = scope.to_string();
        let message = message.into();
        self.mutate(|state| {
            let changed = state.open_issues.get(&scope) != Some(&message);
            state.open_issues.insert(scope, message.clone());
            state.health = "degraded".to_string();
            if changed {
                push_event(state, "failure", "error", message, None, true, retryable);
            }
        })
    }

    pub fn record_recovery(
        &self,
        scope: &str,
        action: impl Into<String>,
        outcome: impl Into<String>,
        succeeded: bool,
    ) -> RefineResult<SupervisorAgentSnapshot> {
        let scope = scope.to_string();
        let message = format!("{}: {}", action.into(), outcome.into());
        self.mutate(|state| {
            let had_issue = succeeded && state.open_issues.remove(&scope).is_some();
            if !succeeded {
                state.open_issues.insert(scope, message.clone());
            }
            state.health = snapshot_health(state.failed_work, &state.open_issues).to_string();
            if had_issue || !succeeded {
                push_event(
                    state,
                    "recovery",
                    if succeeded { "complete" } else { "error" },
                    message,
                    None,
                    !succeeded,
                    !succeeded,
                );
            }
        })
    }

    pub fn ensure_chat_session(&self, chat: &FileChatService) -> RefineResult<ChatSessionRecord> {
        let _guard = self.acquire_lock()?;
        let configured_provider = self.configured_provider();
        let (session, duplicates) = self.ensure_chat_session_locked(chat, &configured_provider)?;
        for duplicate in duplicates {
            let _ = chat.interrupt(
                &duplicate,
                "closed because another supervisor agent session owns this project",
            );
        }
        let mut state = self.snapshot()?;
        if state.session_id.as_deref() != Some(&session.id)
            || state.provider.as_deref() != Some(&session.provider)
        {
            state.session_id = Some(session.id.clone());
            state.provider = Some(session.provider.clone());
            push_event(
                &mut state,
                "observation",
                "info",
                "Supervisor CLI-agent session is available through shared chat.".to_string(),
                None,
                false,
                false,
            );
            self.write_state(&mut state)?;
        }
        Ok(session)
    }

    pub fn reconcile_chat_session(
        &self,
        chat: &FileChatService,
    ) -> RefineResult<SupervisorAgentSnapshot> {
        let sessions = chat.list_sessions()?;
        self.mutate(|state| {
            let session = state.session_id.as_deref().and_then(|id| {
                sessions
                    .iter()
                    .find(|session| session.id == id && !session.closed)
            });
            if let Some(session) = session {
                state.provider = Some(session.provider.clone());
            } else if state.session_id.is_some() {
                state.session_id = None;
                state.provider = None;
                state.last_context_key.clear();
                push_event(
                    state,
                    "observation",
                    "info",
                    "Supervisor conversation ended; active work will start a new singleton session."
                        .to_string(),
                    None,
                    false,
                    false,
                );
            }
        })
    }

    fn reconcile_at(
        &self,
        now: DateTime<Utc>,
        stall_seconds: i64,
    ) -> RefineResult<SupervisorAgentSnapshot> {
        let _guard = self.acquire_lock()?;
        let goals = collect_goals(&self.refine_dir.join("goals"))?;
        let process_evidence = self.process_evidence(now, stall_seconds)?;
        let active = goals.iter().filter(|goal| is_active(&goal.status)).count();
        let queued = goals
            .iter()
            .filter(|goal| goal.status == GoalStatus::Todo)
            .count();
        let failed = goals
            .iter()
            .filter(|goal| goal.status == GoalStatus::Failed)
            .count();
        let active_window = active + queued > 0;
        let goal_states = goals
            .iter()
            .map(|goal| (goal.id.clone(), goal.status.as_str().to_string()))
            .collect::<BTreeMap<_, _>>();
        let stalled = stalled_goals(&goals, &process_evidence, now, stall_seconds);
        let chat = FileChatService::with_runtime_root(&self.refine_dir, &self.runtime_root);
        let configured_provider = self.configured_provider();
        let mut state = self.snapshot()?;
        let before = state.clone();

        state
            .open_issues
            .retain(|scope, _| !scope.starts_with("workflow_stall:"));
        for (goal_id, message) in &stalled {
            state
                .open_issues
                .insert(format!("workflow_stall:{goal_id}"), message.clone());
        }

        if state.initialized {
            for (goal_id, status) in &goal_states {
                let previous = state.goal_states.get(goal_id).cloned();
                if previous.as_ref() != Some(status) {
                    push_event(
                        &mut state,
                        "observation",
                        if status == "failed" { "error" } else { "info" },
                        previous.as_ref().map_or_else(
                            || format!("Goal {goal_id} entered workflow state {status}."),
                            |previous| {
                                format!("Goal {goal_id} changed from {previous} to {status}.")
                            },
                        ),
                        Some(goal_id.clone()),
                        status == "failed",
                        status == "failed",
                    );
                }
            }
        }
        for (goal_id, message) in &stalled {
            let scope = format!("workflow_stall:{goal_id}");
            if before.open_issues.get(&scope) != Some(message) {
                push_event(
                    &mut state,
                    "decision",
                    "blocked",
                    message.clone(),
                    Some(goal_id.clone()),
                    true,
                    true,
                );
            }
        }

        state.initialized = true;
        state.goal_states = goal_states;
        state.lifecycle = if active_window { "observing" } else { "idle" }.to_string();
        state.active_work = active;
        state.queued_work = queued;
        state.failed_work = failed;
        state.health = snapshot_health(failed, &state.open_issues).to_string();

        let mut context_session = None;
        if active_window {
            let has_open_session = chat.list_sessions()?.into_iter().any(|session| {
                !session.closed && matches!(session.attachment, ChatAttachment::Supervisor)
            });
            if !has_open_session
                && let Some(running_session_id) =
                    process_evidence.supervisor_sessions.iter().next().cloned()
            {
                // A stopped/closed toolbar session can still have a provider process winding
                // down. Wait for the shared process supervisor to reap it before launching a
                // replacement, preserving the one-supervisor invariant.
                state.session_id = Some(running_session_id);
            } else {
                let (session, duplicates) =
                    self.ensure_chat_session_locked(&chat, &configured_provider)?;
                for duplicate in duplicates {
                    let _ = chat.interrupt(
                        &duplicate,
                        "closed because another supervisor agent session owns this project",
                    );
                }
                state.session_id = Some(session.id.clone());
                state.provider = Some(session.provider.clone());
                context_session = Some(session);
            }
        } else {
            // A Goal can reach failed, review, or done while the singleton Supervisor provider
            // turn is still running. Keep those final transitions in the same durable queue and
            // transcript, but do not wake an otherwise idle Supervisor session.
            context_session = chat.list_sessions()?.into_iter().find(|session| {
                !session.closed
                    && matches!(session.attachment, ChatAttachment::Supervisor)
                    && (session.queue_dispatching
                        || !session.queued_messages.is_empty()
                        || process_evidence.supervisor_sessions.contains(&session.id))
            });
            if let Some(session) = &context_session {
                state.session_id = Some(session.id.clone());
                state.provider = Some(session.provider.clone());
            } else {
                state.last_context_key.clear();
            }
            if process_evidence.supervisor_sessions.is_empty() && before.lifecycle != "idle" {
                push_event(
                    &mut state,
                    "observation",
                    "complete",
                    "Workflow queue is idle; no further automatic supervisor turns will start."
                        .to_string(),
                    None,
                    false,
                    false,
                );
            }
        }

        if let Some(session) = context_session {
            let provider_blocked = session.interrupted
                && session
                    .interruption_detail
                    .as_deref()
                    .is_some_and(provider_failure_requires_user);
            if session.interrupted && !provider_blocked {
                // A daemon/process interruption is safe to resume through the existing chat
                // queue and provider-session ID. Provider/auth failures instead wait for user
                // action so the coordinator never disguises or loops over them.
                state.last_context_key.clear();
            }
            let context_key = supervision_context_key(&state, &process_evidence);
            if context_key != state.last_context_key && !provider_blocked {
                let prompt = supervision_prompt(&state, &process_evidence, &stalled);
                match chat.append_user_message(&session.id, &prompt) {
                    Ok(_) => {
                        state.last_context_key = context_key;
                        push_event(
                            &mut state,
                            "decision",
                            "running",
                            "Queued current workflow evidence to the singleton supervisor CLI agent."
                                .to_string(),
                            None,
                            false,
                            false,
                        );
                    }
                    Err(error) => {
                        let message =
                            format!("Could not queue supervisor CLI-agent context: {error}");
                        state
                            .open_issues
                            .insert("provider".to_string(), message.clone());
                        push_event(&mut state, "failure", "error", message, None, true, true);
                    }
                }
            } else if provider_blocked {
                let message = session
                    .interruption_detail
                    .clone()
                    .unwrap_or_else(|| "Supervisor provider turn is interrupted.".to_string());
                state.open_issues.insert("provider".to_string(), message);
            }
        }
        state.health = snapshot_health(failed, &state.open_issues).to_string();

        if state != before || !self.state_path().exists() {
            self.write_state(&mut state)?;
        }
        Ok(state)
    }

    fn ensure_chat_session_locked(
        &self,
        chat: &FileChatService,
        configured_provider: &str,
    ) -> RefineResult<(ChatSessionRecord, Vec<String>)> {
        let mut sessions = chat
            .list_sessions()?
            .into_iter()
            .filter(|session| {
                !session.closed && matches!(session.attachment, ChatAttachment::Supervisor)
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        let session = match sessions.first() {
            Some(session) => {
                match chat.migrate_supervisor_provider(&session.id, configured_provider)? {
                    Some(session) => session,
                    None => chat.start_with_options(
                        ChatAttachment::Supervisor,
                        Some(configured_provider),
                        Some("supervisor"),
                    )?,
                }
            }
            None => chat.start_with_options(
                ChatAttachment::Supervisor,
                Some(configured_provider),
                Some("supervisor"),
            )?,
        };
        let duplicates = sessions
            .into_iter()
            .skip(1)
            .map(|session| session.id)
            .collect();
        Ok((session, duplicates))
    }

    fn process_evidence(
        &self,
        now: DateTime<Utc>,
        stall_seconds: i64,
    ) -> RefineResult<ProcessEvidence> {
        let mut evidence = ProcessEvidence::default();
        // Goal workflow providers use the agent-scoped registry, while chat providers use the
        // port-scoped registry. Both are the shared process-supervisor substrate and both must be
        // observed or live Goal agents would be misclassified as missing.
        for process_root in [&self.runtime_root, &self.runtime_root.join("agents")] {
            for process in
                FileProcessSupervisor::new(process_root).recover_owner(ProcessOwner::Agent)?
            {
                let details = process_details(&process);
                if details.get("kind").and_then(Value::as_str) == Some("workflow")
                    && let Some(goal_id) = details.get("goal_id").and_then(Value::as_str)
                {
                    evidence.live_goal_ids.insert(goal_id.to_string());
                    if process_is_quiet(&process, now, stall_seconds) {
                        evidence.quiet_goal_ids.insert(goal_id.to_string());
                    }
                }
                if details.get("mode").and_then(Value::as_str) == Some("supervisor")
                    && let Some(session_id) = details.get("session_id").and_then(Value::as_str)
                {
                    evidence.supervisor_sessions.insert(session_id.to_string());
                }
            }
        }
        Ok(evidence)
    }

    fn configured_provider(&self) -> String {
        FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root)
            .load()
            .ok()
            .and_then(|settings| {
                settings
                    .get("agent_cli")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "claude".to_string())
    }

    fn configured_stall_seconds(&self) -> i64 {
        FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root)
            .load()
            .ok()
            .and_then(|settings| {
                settings
                    .get("supervisor_agent_stall_seconds")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<i64>().ok())
            })
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_STALL_SECONDS)
    }

    fn mutate(
        &self,
        update: impl FnOnce(&mut SupervisorAgentSnapshot),
    ) -> RefineResult<SupervisorAgentSnapshot> {
        let _guard = self.acquire_lock()?;
        let mut state = self.snapshot()?;
        let before = state.clone();
        update(&mut state);
        if state != before || !self.state_path().exists() {
            self.write_state(&mut state)?;
        }
        Ok(state)
    }

    fn write_state(&self, state: &mut SupervisorAgentSnapshot) -> RefineResult<()> {
        fs::create_dir_all(&self.refine_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create supervisor agent state directory {}: {error}",
                self.refine_dir.display()
            ))
        })?;
        state.updated_at = Utc::now().to_rfc3339();
        if state.events.len() > EVENT_LIMIT {
            state.events = state.events.split_off(state.events.len() - EVENT_LIMIT);
        }
        let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
            RefineError::Serialization(format!("failed to encode supervisor agent state: {error}"))
        })?;
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = self.refine_dir.join(format!(
            ".supervisor-agent-{}-{sequence}.tmp",
            std::process::id()
        ));
        fs::write(&temp, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write supervisor agent state {}: {error}",
                temp.display()
            ))
        })?;
        fs::rename(&temp, self.state_path()).map_err(|error| {
            RefineError::Io(format!("failed to publish supervisor agent state: {error}"))
        })
    }

    fn state_path(&self) -> PathBuf {
        self.refine_dir.join(SUPERVISOR_AGENT_STATE_FILE)
    }

    fn acquire_lock(&self) -> RefineResult<SupervisorAgentLock> {
        fs::create_dir_all(&self.refine_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create supervisor agent directory {}: {error}",
                self.refine_dir.display()
            ))
        })?;
        let path = self.refine_dir.join(SUPERVISOR_AGENT_LOCK_FILE);
        for _ in 0..100 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(SupervisorAgentLock { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > Duration::from_secs(30));
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to lock supervisor agent state {}: {error}",
                        path.display()
                    )));
                }
            }
        }
        Err(RefineError::Conflict(
            "supervisor agent state is busy; retry shortly".to_string(),
        ))
    }
}

struct SupervisorAgentLock {
    path: PathBuf,
}

impl Drop for SupervisorAgentLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn is_active(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::InProgress | GoalStatus::ReadyMerge | GoalStatus::Build | GoalStatus::Qa
    )
}

fn snapshot_health(failed: usize, open_issues: &BTreeMap<String, String>) -> &'static str {
    if !open_issues.is_empty() {
        "degraded"
    } else if failed > 0 {
        "attention"
    } else {
        "healthy"
    }
}

fn stalled_goals(
    goals: &[GoalObservation],
    processes: &ProcessEvidence,
    now: DateTime<Utc>,
    stall_seconds: i64,
) -> BTreeMap<String, String> {
    let mut stalled = BTreeMap::new();
    for goal in goals
        .iter()
        .filter(|goal| goal.status == GoalStatus::InProgress)
    {
        if processes.live_goal_ids.contains(&goal.id) {
            if processes.quiet_goal_ids.contains(&goal.id) {
                stalled.insert(
                    goal.id.clone(),
                    format!(
                        "Goal {} still has a live workflow agent, but its process output has been quiet for at least {stall_seconds} seconds; investigate before retrying.",
                        goal.id
                    ),
                );
            }
            continue;
        }
        let old_enough = DateTime::parse_from_rfc3339(&goal.updated)
            .ok()
            .is_some_and(|updated| {
                now.signed_duration_since(updated.with_timezone(&Utc))
                    .num_seconds()
                    >= stall_seconds
            });
        if old_enough {
            stalled.insert(
                goal.id.clone(),
                format!(
                    "Goal {} is in progress without a live workflow-agent process; inspect existing workflow evidence and retry explicitly if the worker was lost.",
                    goal.id
                ),
            );
        }
    }
    stalled
}

fn process_details(process: &ManagedProcess) -> serde_json::Map<String, Value> {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| details.as_object().cloned())
        .unwrap_or_default()
}

fn process_is_quiet(process: &ManagedProcess, now: DateTime<Utc>, stall_seconds: i64) -> bool {
    let latest_output = [
        process.stdout_path.as_deref(),
        process.stderr_path.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
    .max();
    let reference =
        latest_output.or_else(|| {
            process.started_at.parse::<u64>().ok().and_then(|millis| {
                SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(millis))
            })
        });
    reference
        .and_then(|time| {
            now.signed_duration_since(DateTime::<Utc>::from(time))
                .to_std()
                .ok()
        })
        .is_some_and(|age| age.as_secs() >= stall_seconds as u64)
}

fn provider_failure_requires_user(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    [
        "provider turn failed",
        "provider session resume failed",
        "authentication",
        "auth failed",
        "not found on path",
        "permission denied",
        "rate limit",
    ]
    .iter()
    .any(|marker| detail.contains(marker))
}

fn supervision_context_key(state: &SupervisorAgentSnapshot, processes: &ProcessEvidence) -> String {
    let goals = state
        .goal_states
        .iter()
        .map(|(id, status)| format!("{id}:{status}"))
        .collect::<Vec<_>>()
        .join(",");
    let issues = state
        .open_issues
        .iter()
        .map(|(scope, message)| format!("{scope}:{message}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "goals={goals}|live={:?}|quiet={:?}|issues={issues}",
        processes.live_goal_ids, processes.quiet_goal_ids
    )
}

fn supervision_prompt(
    state: &SupervisorAgentSnapshot,
    processes: &ProcessEvidence,
    stalled: &BTreeMap<String, String>,
) -> String {
    let active_work = state.active_work.to_string();
    let queued_work = state.queued_work.to_string();
    let failed_work = state.failed_work.to_string();
    let goal_states = format!("{:?}", state.goal_states);
    let live_goal_ids = format!("{:?}", processes.live_goal_ids);
    let stalled = format!("{stalled:?}");
    render(
        PromptTemplate::Supervisor,
        &[
            ("active_work", &active_work),
            ("queued_work", &queued_work),
            ("failed_work", &failed_work),
            ("goal_states", &goal_states),
            ("live_goal_ids", &live_goal_ids),
            ("stalled", &stalled),
        ],
    )
}

fn collect_goals(root: &Path) -> RefineResult<Vec<GoalObservation>> {
    let mut goals = Vec::new();
    collect_goals_into(root, &mut goals)?;
    Ok(goals)
}

fn collect_goals_into(root: &Path, goals: &mut Vec<GoalObservation>) -> RefineResult<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| {
        RefineError::Io(format!(
            "failed to inspect Goals at {}: {error}",
            root.display()
        ))
    })? {
        let entry = entry
            .map_err(|error| RefineError::Io(format!("failed to inspect Goal entry: {error}")))?;
        let path = entry.path();
        if path.is_dir() {
            collect_goals_into(&path, goals)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("goal.json") {
            let value: Value = serde_json::from_slice(&fs::read(&path).map_err(|error| {
                RefineError::Io(format!("failed to read {}: {error}", path.display()))
            })?)
            .map_err(|error| {
                RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
            })?;
            if let (Some(id), Some(status), Some(updated)) = (
                value.get("id").and_then(Value::as_str),
                value
                    .get("status")
                    .and_then(Value::as_str)
                    .and_then(GoalStatus::parse_wire),
                value.get("updated").and_then(Value::as_str),
            ) {
                goals.push(GoalObservation {
                    id: id.to_string(),
                    status,
                    updated: updated.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn push_event(
    state: &mut SupervisorAgentSnapshot,
    kind: &str,
    status: &str,
    message: String,
    goal_id: Option<String>,
    actionable: bool,
    retryable: bool,
) {
    if state.events.iter().rev().take(50).any(|event| {
        event.kind == kind
            && event.status == status
            && event.message == message
            && event.goal_id == goal_id
    }) {
        return;
    }
    let created_at = Utc::now().to_rfc3339();
    state.events.push(SupervisorAgentEvent {
        id: format!("supervisor-{}-{}", created_at, state.events.len() + 1),
        kind: kind.to_string(),
        status: status.to_string(),
        message,
        created_at,
        goal_id,
        actionable,
        retryable,
    });
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};
    use std::time::Instant;

    use serde_json::json;

    use super::*;
    use crate::workflow::WorkflowEngine;
    use crate::workflow::capacity::{AgentCapacityRequest, AgentCapacityService};

    fn test_service(name: &str) -> FileSupervisorAgentService {
        let root = std::env::temp_dir().join(format!(
            "refine-supervisor-agent-{name}-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let service = FileSupervisorAgentService::new(root.join(".refine"), root.join("run/8080"));
        fs::create_dir_all(&service.refine_dir).unwrap();
        FileSettingsService::with_active_root(&service.refine_dir, &service.runtime_root)
            .update(&json!({"agent_cli": "smoke-ai"}))
            .unwrap();
        service
    }

    fn write_goal(service: &FileSupervisorAgentService, id: &str, status: &str, updated: &str) {
        let path = service
            .refine_dir
            .join("goals/GO")
            .join(id)
            .join("goal.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            serde_json::to_vec_pretty(&json!({
                "id": id,
                "name": id,
                "status": status,
                "updated": updated,
                "created": updated,
                "priority": "low",
                "reporter": "test",
                "rounds": []
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_smoke_provider(service: &FileSupervisorAgentService, output: &str) -> PathBuf {
        let bin = service.refine_dir.join("provider-bin/smoke-ai");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(
            &bin,
            format!("#!/usr/bin/env bash\nprintf '%s\\n' '{}'\n", output),
        )
        .unwrap();
        let mut permissions = fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&bin, permissions).unwrap();
        bin
    }

    fn write_slow_smoke_provider(service: &FileSupervisorAgentService, output: &str) {
        let bin = service.refine_dir.join("provider-bin/smoke-ai");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(
            &bin,
            format!(
                "#!/usr/bin/env bash\nsleep 0.05\nprintf '%s\\n' '{}'\n",
                output
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&bin, permissions).unwrap();
    }

    fn write_cancellable_smoke_provider(service: &FileSupervisorAgentService) -> PathBuf {
        let bin = service.refine_dir.join("provider-bin/smoke-ai");
        let marker = service.refine_dir.join("cancellable-provider.marker");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        fs::write(
            &bin,
            format!(
                "#!/usr/bin/env bash\ntrap 'printf terminated > \"{}\"; sleep 0.25; exit 143' TERM\nprintf started > \"{}\"\nwhile :; do :; done\n",
                marker.display(),
                marker.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&bin, permissions).unwrap();
        marker
    }

    fn wait_for_session(service: &FileSupervisorAgentService) -> ChatSessionRecord {
        let chat = FileChatService::with_runtime_root(&service.refine_dir, &service.runtime_root);
        for _ in 0..200 {
            if let Some(session) = chat
                .list_sessions()
                .unwrap()
                .into_iter()
                .find(|session| matches!(session.attachment, ChatAttachment::Supervisor))
                && !session.in_flight
                && !session.queue_dispatching
                && session.queued_messages.is_empty()
                && !session.transcript_events.is_empty()
            {
                return session;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("supervisor session did not finish");
    }

    #[test]
    fn active_work_automatically_launches_configured_cli_provider_once() {
        let service = test_service("provider-launch");
        write_smoke_provider(&service, "supervisor-provider-launched");
        write_goal(&service, "GOAL1", "todo", &Utc::now().to_rfc3339());

        let state = service.reconcile().unwrap();
        assert_eq!(state.lifecycle, "observing");
        let session = wait_for_session(&service);
        assert_eq!(session.provider, "smoke-ai");
        assert!(session.transcript_events.iter().any(|event| {
            event.get("text").and_then(Value::as_str) == Some("supervisor-provider-launched")
        }));

        let before = fs::metadata(service.state_path())
            .unwrap()
            .modified()
            .unwrap();
        let repeated = service.reconcile().unwrap();
        assert_eq!(repeated.session_id.as_deref(), Some(session.id.as_str()));
        assert_eq!(
            FileChatService::with_runtime_root(&service.refine_dir, &service.runtime_root)
                .list_sessions()
                .unwrap()
                .into_iter()
                .filter(|candidate| matches!(candidate.attachment, ChatAttachment::Supervisor))
                .count(),
            1
        );
        assert_eq!(
            fs::metadata(service.state_path())
                .unwrap()
                .modified()
                .unwrap(),
            before,
            "a no-op reconcile must not rewrite durable supervisor state"
        );
    }

    #[test]
    fn durable_supervisor_session_migrates_to_configured_provider() {
        let service = test_service("provider-migration");
        let chat = FileChatService::with_runtime_root(&service.refine_dir, &service.runtime_root);
        let mut legacy = chat
            .start_with_options(
                ChatAttachment::Supervisor,
                Some("claude"),
                Some("supervisor"),
            )
            .unwrap();
        legacy.provider_session_id = Some("legacy-provider-session".to_string());
        fs::write(
            service
                .refine_dir
                .join("chat/sessions")
                .join(format!("{}.json", legacy.id)),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let migrated = service.ensure_chat_session(&chat).unwrap();
        assert_eq!(migrated.id, legacy.id);
        assert_eq!(migrated.provider, "smoke-ai");
        assert_eq!(migrated.provider_session_id, None);
        assert!(migrated.transcript_events.iter().any(|event| {
            event
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("migrated from claude to smoke-ai"))
        }));

        FileSettingsService::with_active_root(&service.refine_dir, &service.runtime_root)
            .update(&json!({"agent_cli": "codex"}))
            .unwrap();
        let reconfigured = service.ensure_chat_session(&chat).unwrap();
        assert_eq!(reconfigured.id, legacy.id);
        assert_eq!(reconfigured.provider, "codex");
        assert_eq!(reconfigured.provider_session_id, None);
    }

    #[test]
    fn stop_signals_managed_provider_and_holds_capacity_until_exit() {
        let service = test_service("managed-stop");
        FileSettingsService::with_active_root(&service.refine_dir, &service.runtime_root)
            .update(&json!({
                "parallel_run_cap": "1",
                "parallel_per_node_cap": "1",
                "parallel_per_provider_cap": "1",
                "parallel_per_target_app_cap": "1"
            }))
            .unwrap();
        let marker = write_cancellable_smoke_provider(&service);
        write_goal(&service, "GOAL1", "todo", &Utc::now().to_rfc3339());
        let state = service.reconcile().unwrap();
        let session_id = state.session_id.unwrap();
        let process_supervisor = FileProcessSupervisor::new(service.runtime_root.join("agents"));
        for _ in 0..200 {
            if marker.exists() && !process_supervisor.list().unwrap().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(&marker).unwrap(), "started");
        assert_eq!(process_supervisor.list().unwrap().len(), 1);

        let chat = FileChatService::with_runtime_root(&service.refine_dir, &service.runtime_root);
        let started = Instant::now();
        let stopped = chat.stop(&session_id).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(stopped.closed);
        assert_eq!(stopped.interruption_detail.as_deref(), Some("stopped"));
        assert_eq!(process_supervisor.list().unwrap().len(), 1);

        let capacity = AgentCapacityService::new(&service.runtime_root);
        assert_eq!(capacity.snapshot().unwrap().leases.len(), 1);
        let policy = WorkflowEngine::with_target_root(
            &service.runtime_root,
            service.refine_dir.parent().unwrap(),
        )
        .policy()
        .unwrap();
        let workflow_request = AgentCapacityRequest {
            owner_id: "workflow:test".to_string(),
            role: "workflow".to_string(),
            node_id: policy.active_node_id.clone(),
            provider: policy.provider.clone(),
            target_app_id: policy.target_app_id.clone(),
        };
        assert!(
            !capacity
                .try_acquire(&policy, workflow_request.clone())
                .unwrap()
        );

        for _ in 0..300 {
            if process_supervisor.list().unwrap().is_empty()
                && capacity.snapshot().unwrap().leases.is_empty()
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(process_supervisor.list().unwrap().is_empty());
        assert!(capacity.snapshot().unwrap().leases.is_empty());
        assert!(capacity.try_acquire(&policy, workflow_request).unwrap());
        assert!(capacity.release("workflow:test").unwrap());
        let completed = chat.read(&session_id).unwrap();
        assert!(!completed.in_flight);
        assert_eq!(completed.closed_reason.as_deref(), Some("stopped"));
        assert!(
            completed.progress_lines.iter().any(|line| {
                line.contains("Managed provider process exited after cancellation")
            })
        );
        assert_eq!(fs::read_to_string(&marker).unwrap(), "terminated");
    }

    #[test]
    fn concurrent_reconcile_keeps_one_session_and_one_automatic_turn() {
        let service = Arc::new(test_service("concurrent"));
        write_smoke_provider(&service, "one-supervisor-turn");
        write_goal(&service, "GOAL1", "todo", &Utc::now().to_rfc3339());
        write_goal(&service, "GOAL2", "todo", &Utc::now().to_rfc3339());
        let barrier = Arc::new(Barrier::new(5));
        let handles = (0..4)
            .map(|_| {
                let service = Arc::clone(&service);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    service.reconcile().unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for handle in handles {
            assert_eq!(handle.join().unwrap().queued_work, 2);
        }
        let session = wait_for_session(&service);
        let assistant_turns = session
            .transcript_events
            .iter()
            .filter(|event| event.get("role").and_then(Value::as_str) == Some("assistant"))
            .count();
        assert_eq!(assistant_turns, 1);
    }

    #[test]
    fn user_and_system_followups_share_the_supervisor_queue_and_transcript() {
        let service = test_service("shared-followups");
        write_slow_smoke_provider(&service, "shared-supervisor-output");
        write_goal(&service, "GOAL1", "todo", &Utc::now().to_rfc3339());
        let state = service.reconcile().unwrap();
        let session_id = state.session_id.unwrap();
        let chat = FileChatService::with_runtime_root(&service.refine_dir, &service.runtime_root);

        chat.append_user_message(&session_id, "user steering while active")
            .unwrap();
        let session = wait_for_session(&service);
        assert_eq!(session.id, session_id);
        assert!(
            session
                .transcript_events
                .iter()
                .any(|event| event.get("role").and_then(Value::as_str) == Some("assistant"))
        );
        let user_transcript = session
            .transcript_events
            .iter()
            .filter(|event| event.get("role").and_then(Value::as_str) == Some("user"))
            .filter_map(|event| event.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(user_transcript.contains("Supervise until the queue is idle"));
        assert!(user_transcript.contains("user steering while active"));
    }

    #[test]
    fn long_running_turn_tracks_every_goal_transition_without_duplicate_provider() {
        let service = test_service("live-goal-transitions");
        let marker = write_cancellable_smoke_provider(&service);
        write_goal(&service, "GOAL1", "todo", &Utc::now().to_rfc3339());
        let launched = service.reconcile().unwrap();
        let session_id = launched.session_id.unwrap();
        let process_supervisor = FileProcessSupervisor::new(service.runtime_root.join("agents"));
        for _ in 0..200 {
            if marker.exists() && !process_supervisor.list().unwrap().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(&marker).unwrap(), "started");
        assert_eq!(process_supervisor.list().unwrap().len(), 1);

        let mut previous_updated_at = launched.updated_at;
        for (status, active, failed, health, lifecycle) in [
            ("in-progress", 1, 0, "healthy", "observing"),
            ("failed", 0, 1, "attention", "idle"),
            ("review", 0, 0, "healthy", "idle"),
            ("done", 0, 0, "healthy", "idle"),
        ] {
            write_goal(&service, "GOAL1", status, &Utc::now().to_rfc3339());
            let snapshot = service.reconcile().unwrap();
            assert_eq!(
                snapshot.goal_states.get("GOAL1").map(String::as_str),
                Some(status)
            );
            assert_eq!(snapshot.active_work, active);
            assert_eq!(snapshot.queued_work, 0);
            assert_eq!(snapshot.failed_work, failed);
            assert_eq!(snapshot.health, health);
            assert_eq!(snapshot.lifecycle, lifecycle);
            assert_ne!(snapshot.updated_at, previous_updated_at);
            previous_updated_at = snapshot.updated_at;
        }

        let chat = FileChatService::with_runtime_root(&service.refine_dir, &service.runtime_root);
        let sessions = chat
            .list_sessions()
            .unwrap()
            .into_iter()
            .filter(|session| matches!(session.attachment, ChatAttachment::Supervisor))
            .collect::<Vec<_>>();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, session_id);
        assert!(sessions[0].queue_dispatching);
        let queued_context = sessions[0]
            .queued_messages
            .iter()
            .map(|message| message.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for status in ["in-progress", "failed", "review", "done"] {
            assert!(
                queued_context.contains(&format!("\"GOAL1\": \"{status}\"")),
                "missing {status} context from the shared Supervisor queue"
            );
        }
        assert_eq!(process_supervisor.list().unwrap().len(), 1);

        let final_state = service.snapshot().unwrap();
        for status in ["in-progress", "failed", "review", "done"] {
            assert_eq!(
                final_state
                    .events
                    .iter()
                    .filter(|event| event.message.ends_with(&format!("to {status}.")))
                    .count(),
                1,
                "expected exactly one durable observation for {status}"
            );
        }

        chat.stop(&session_id).unwrap();
        for _ in 0..300 {
            if process_supervisor.list().unwrap().is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(process_supervisor.list().unwrap().is_empty());
    }

    #[test]
    fn daemon_interruption_resumes_same_session_without_duplicate() {
        let service = test_service("daemon-reconnect");
        write_smoke_provider(&service, "reconnected-supervisor-output");
        write_goal(&service, "GOAL1", "todo", &Utc::now().to_rfc3339());
        let state = service.reconcile().unwrap();
        let session_id = state.session_id.unwrap();
        let mut session = wait_for_session(&service);
        session.interrupted = true;
        session.interruption_detail = Some("daemon restarted during provider turn".to_string());
        fs::write(
            service
                .refine_dir
                .join("chat/sessions")
                .join(format!("{session_id}.json")),
            serde_json::to_vec_pretty(&session).unwrap(),
        )
        .unwrap();

        service.reconcile().unwrap();
        let resumed = wait_for_session(&service);
        assert_eq!(resumed.id, session_id);
        assert_eq!(
            resumed
                .transcript_events
                .iter()
                .filter(|event| event.get("role").and_then(Value::as_str) == Some("assistant"))
                .count(),
            2
        );
        assert_eq!(
            FileChatService::with_runtime_root(&service.refine_dir, &service.runtime_root)
                .list_sessions()
                .unwrap()
                .into_iter()
                .filter(|candidate| matches!(candidate.attachment, ChatAttachment::Supervisor))
                .count(),
            1
        );
    }

    #[test]
    fn live_process_progress_prevents_timestamp_only_false_stall() {
        let service = test_service("live-progress");
        write_goal(&service, "GOAL1", "in-progress", "2025-01-01T00:00:00Z");
        let now = Utc::now();
        let processes = ProcessEvidence {
            live_goal_ids: BTreeSet::from(["GOAL1".to_string()]),
            ..Default::default()
        };
        let goals = collect_goals(&service.refine_dir.join("goals")).unwrap();
        assert!(stalled_goals(&goals, &processes, now, 1).is_empty());
    }

    #[test]
    fn workflow_agent_registry_is_part_of_live_process_evidence() {
        let service = test_service("workflow-agent-registry");
        let stdout = service
            .runtime_root
            .join("agents/processes/workflow.stdout.log");
        fs::create_dir_all(stdout.parent().unwrap()).unwrap();
        fs::write(&stdout, "live workflow output\n").unwrap();
        FileProcessSupervisor::new(service.runtime_root.join("agents"))
            .register(ManagedProcess {
                id: "workflow-agent".to_string(),
                owner: ProcessOwner::Agent,
                pid: Some(std::process::id()),
                state: "running".to_string(),
                label: Some("smoke-ai".to_string()),
                details: Some(
                    json!({
                        "kind": "workflow",
                        "goal_id": "GOAL1",
                        "workflow_state": "in-progress"
                    })
                    .to_string(),
                ),
                stdout_path: Some(stdout.display().to_string()),
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
                    .to_string(),
                exit_code: None,
            })
            .unwrap();

        let evidence = service.process_evidence(Utc::now(), 30).unwrap();
        assert!(evidence.live_goal_ids.contains("GOAL1"));
        assert!(!evidence.quiet_goal_ids.contains("GOAL1"));
    }

    #[test]
    fn lost_worker_is_actionable_and_idle_work_stops_new_turns() {
        let service = test_service("lost-idle");
        write_smoke_provider(&service, "observed-lost-worker");
        write_goal(&service, "GOAL1", "in-progress", "2025-01-01T00:00:00Z");
        let state = service.reconcile_with_stall_seconds(1).unwrap();
        assert!(state.open_issues.contains_key("workflow_stall:GOAL1"));
        assert!(state.events.iter().any(|event| event.actionable));
        let session = wait_for_session(&service);

        write_goal(&service, "GOAL1", "done", &Utc::now().to_rfc3339());
        assert_eq!(service.reconcile().unwrap().lifecycle, "idle");
        thread::sleep(Duration::from_millis(30));
        let after = FileChatService::with_runtime_root(&service.refine_dir, &service.runtime_root)
            .list_sessions()
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == session.id)
            .unwrap();
        let assistant_turns = after
            .transcript_events
            .iter()
            .filter(|event| event.get("role").and_then(Value::as_str) == Some("assistant"))
            .count();
        assert_eq!(assistant_turns, 1);
    }
}

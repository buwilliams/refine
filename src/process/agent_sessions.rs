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

use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::agent_providers::HostAgentProviderService;

const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(40);
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 36;
const MAX_INPUT_BYTES: usize = 16_000;
const MAX_EVENT_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct GoalAgentLaunch {
    pub runtime_root: PathBuf,
    pub cwd: PathBuf,
    pub provider: String,
    pub prompt: String,
    pub metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoalAgentResult {
    pub output: String,
    pub session_id: String,
    pub process_id: String,
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
    pub alive: bool,
    pub exited: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AgentSessionCommand {
    Input { data: String },
    Resize { cols: u16, rows: u16 },
}

#[derive(Clone, Debug, Deserialize)]
struct AgentSessionSignal {
    state: String,
    #[serde(default)]
    message: String,
}

/// Run the workflow's implementation agent in its native interactive harness.
///
/// The PTY is owned by the workflow runner, while its process record, transcript,
/// command queue, and signal file are ordinary runtime artifacts. That split lets
/// the daemon, browser, and CLI attach to the same Goal Agent without making a
/// browser connection part of workflow execution.
pub fn run_goal_agent<F>(
    launch: GoalAgentLaunch,
    mut on_attention: F,
) -> RefineResult<GoalAgentResult>
where
    F: FnMut(GoalAgentAttention),
{
    let cwd = launch.cwd.canonicalize().map_err(|error| {
        RefineError::InvalidInput(format!(
            "Goal Agent cwd {} is not available: {error}",
            launch.cwd.display()
        ))
    })?;
    let session_id = Uuid::new_v4().to_string();
    let process_id = format!("goal-agent-{session_id}");
    let supervisor = FileProcessSupervisor::new(&launch.runtime_root);
    fs::create_dir_all(supervisor.processes_dir()).map_err(|error| {
        RefineError::Io(format!(
            "failed to create Goal Agent process registry {}: {error}",
            supervisor.processes_dir().display()
        ))
    })?;
    let launch_lock_path = supervisor.processes_dir().join(".goal-agent-launch.lock");
    let launch_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&launch_lock_path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open Goal Agent launch lock {}: {error}",
                launch_lock_path.display()
            ))
        })?;
    launch_lock.lock_exclusive().map_err(|error| {
        RefineError::Io(format!(
            "failed to lock Goal Agent launch coordination {}: {error}",
            launch_lock_path.display()
        ))
    })?;
    if let Some(goal_id) = launch
        .metadata
        .get("goal_id")
        .and_then(Value::as_str)
        .filter(|goal_id| !goal_id.trim().is_empty())
    {
        match find_goal_agent_session(&launch.runtime_root, goal_id) {
            Ok(_) => {
                return Err(RefineError::Conflict(format!(
                    "Goal {goal_id} already has a running Goal Agent"
                )));
            }
            Err(RefineError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    let stdout_path = supervisor
        .processes_dir()
        .join(format!("{process_id}.stdout.log"));
    let command_path = supervisor
        .processes_dir()
        .join(format!("{process_id}.commands.jsonl"));
    let signal_path = supervisor
        .processes_dir()
        .join(format!("{process_id}.signal.json"));
    fs::File::create(&stdout_path).map_err(|error| {
        RefineError::Io(format!(
            "failed to create Goal Agent transcript {}: {error}",
            stdout_path.display()
        ))
    })?;
    if let Err(error) = fs::File::create(&command_path) {
        let _ = fs::remove_file(&stdout_path);
        return Err(RefineError::Io(format!(
            "failed to create Goal Agent command queue {}: {error}",
            command_path.display()
        )));
    }

    let provider_service = HostAgentProviderService::with_runtime_root(&launch.runtime_root);
    let protocol_prompt = goal_agent_protocol_prompt(&launch.prompt, &signal_path);
    let command = match provider_service.interactive_command(&launch.provider, &protocol_prompt) {
        Ok(command) => command,
        Err(error) => {
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(error);
        }
    };
    let mut metadata = launch.metadata;
    metadata.insert("kind".to_string(), json!("interactive_session"));
    metadata.insert("profile".to_string(), json!("goal"));
    metadata.insert("role".to_string(), json!("goal"));
    metadata.insert("mode".to_string(), json!("goal"));
    metadata.insert("provider".to_string(), json!(&launch.provider));
    metadata.insert("session_id".to_string(), json!(&session_id));
    metadata.insert("cwd".to_string(), json!(cwd.display().to_string()));
    metadata.insert("attention_state".to_string(), json!("working"));
    metadata.insert(
        "command_path".to_string(),
        json!(command_path.display().to_string()),
    );
    metadata.insert(
        "signal_path".to_string(),
        json!(signal_path.display().to_string()),
    );

    let managed_spec = ManagedProcessSpec {
        owner: ProcessOwner::Agent,
        command: command.binary.clone(),
        args: command.args.clone(),
        cwd: Some(cwd.display().to_string()),
        env: vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("REFINE_TERMINAL".to_string(), "1".to_string()),
            ("REFINE_SESSION_ROLE".to_string(), "goal".to_string()),
            ("REFINE_AGENT_SESSION_ID".to_string(), session_id.clone()),
            (
                "REFINE_AGENT_SIGNAL_PATH".to_string(),
                signal_path.display().to_string(),
            ),
        ],
        stdin: None,
        limits: Some(ProcessResourceLimits {
            kill_on_parent_exit: true,
            ..Default::default()
        }),
        authorization_command: Some(
            std::iter::once(command.binary.as_str())
                .chain(command.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        sensitive: false,
        metadata: metadata.clone(),
    };
    if let Err(error) = supervisor.validate_interactive_launch(&managed_spec) {
        cleanup_session_artifacts(&command_path, &signal_path);
        let _ = fs::remove_file(&stdout_path);
        return Err(error);
    }
    let workflow_registration_guard =
        match supervisor.workflow_process_registration_guard(&managed_spec) {
            Ok(guard) => guard,
            Err(error) => {
                cleanup_session_artifacts(&command_path, &signal_path);
                let _ = fs::remove_file(&stdout_path);
                return Err(error);
            }
        };

    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(pty_size(DEFAULT_COLS, DEFAULT_ROWS)) {
        Ok(pair) => pair,
        Err(error) => {
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(RefineError::Io(format!(
                "failed to open Goal Agent PTY: {error}"
            )));
        }
    };
    let mut pty_command = CommandBuilder::new(&command.binary);
    pty_command.args(&command.args);
    pty_command.cwd(&cwd);
    for (key, value) in &managed_spec.env {
        pty_command.env(key, value);
    }
    for key in [
        "ANTHROPIC_API_KEY",
        "CLAUDE_API_KEY",
        "CODEX_API_KEY",
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "GOOGLE_GENAI_API_KEY",
        "OPENAI_API_KEY",
    ] {
        pty_command.env_remove(key);
    }
    let mut child = match pair.slave.spawn_command(pty_command) {
        Ok(child) => child,
        Err(error) => {
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(RefineError::Io(format!(
                "failed to start interactive Goal Agent with {}: {error}",
                launch.provider
            )));
        }
    };
    let pid = child.process_id();
    let mut reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(RefineError::Io(format!(
                "failed to read Goal Agent output: {error}"
            )));
        }
    };
    let mut writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(RefineError::Io(format!(
                "failed to open Goal Agent input: {error}"
            )));
        }
    };
    drop(pair.slave);

    let details = match encode_metadata(&metadata) {
        Ok(details) => details,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            cleanup_session_artifacts(&command_path, &signal_path);
            let _ = fs::remove_file(&stdout_path);
            return Err(error);
        }
    };
    let mut process = ManagedProcess {
        id: process_id.clone(),
        owner: ProcessOwner::Agent,
        pid,
        state: "running".to_string(),
        label: Some(format!(
            "Goal {} agent",
            metadata
                .get("goal_id")
                .and_then(Value::as_str)
                .unwrap_or("workflow")
        )),
        details: Some(details),
        stdout_path: Some(stdout_path.display().to_string()),
        stderr_path: None,
        stdin_path: Some(command_path.display().to_string()),
        limits: managed_spec.limits,
        started_at: Utc::now().to_rfc3339(),
        exit_code: None,
    };
    if let Err(error) = supervisor.register(process.clone()) {
        let _ = child.kill();
        let _ = child.wait();
        cleanup_session_artifacts(&command_path, &signal_path);
        let _ = fs::remove_file(&stdout_path);
        return Err(error);
    }
    drop(workflow_registration_guard);
    let _ = FileExt::unlock(&launch_lock);
    drop(launch_lock);

    let reader_path = stdout_path.clone();
    let reader_thread = thread::spawn(move || -> RefineResult<()> {
        let mut output = OpenOptions::new()
            .append(true)
            .open(&reader_path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open Goal Agent transcript {}: {error}",
                    reader_path.display()
                ))
            })?;
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(count) => {
                    output.write_all(&buffer[..count]).map_err(|error| {
                        RefineError::Io(format!(
                            "failed to append Goal Agent transcript {}: {error}",
                            reader_path.display()
                        ))
                    })?;
                    output.flush().map_err(|error| {
                        RefineError::Io(format!(
                            "failed to flush Goal Agent transcript {}: {error}",
                            reader_path.display()
                        ))
                    })?;
                }
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "Goal Agent output stream failed: {error}"
                    )));
                }
            }
        }
    });

    let mut command_offset = 0_u64;
    let mut completed_by_signal = false;
    let mut completion_report = None;
    let status_result = (|| -> RefineResult<_> {
        loop {
            for command in read_commands_since(&command_path, &mut command_offset)? {
                match command {
                    AgentSessionCommand::Input { data } => {
                        writer
                            .write_all(data.as_bytes())
                            .and_then(|_| writer.flush())
                            .map_err(|error| {
                                RefineError::Io(format!(
                                    "failed to send attached input to Goal Agent: {error}"
                                ))
                            })?;
                        if metadata.get("attention_state").and_then(Value::as_str)
                            == Some("needs_input")
                        {
                            metadata.insert("attention_state".to_string(), json!("working"));
                            metadata.remove("attention_message");
                            metadata.remove("attention_reason");
                            process.details = Some(encode_metadata(&metadata)?);
                            supervisor.register(process.clone())?;
                        }
                    }
                    AgentSessionCommand::Resize { cols, rows } => {
                        pair.master.resize(pty_size(cols, rows)).map_err(|error| {
                            RefineError::Io(format!("failed to resize Goal Agent PTY: {error}"))
                        })?;
                    }
                }
            }

            if let Some(signal) = take_signal(&signal_path)? {
                match signal.state.trim() {
                    "completed" | "complete" => {
                        completed_by_signal = true;
                        completion_report = (!signal.message.trim().is_empty())
                            .then(|| signal.message.trim().to_string());
                        metadata.insert("attention_state".to_string(), json!("completed"));
                        metadata.remove("attention_message");
                        process.details = Some(encode_metadata(&metadata)?);
                        supervisor.register(process.clone())?;
                        let _ = child.kill();
                    }
                    "needs_input" | "waiting_for_user" => {
                        let message = if signal.message.trim().is_empty() {
                            "The Goal Agent needs user input before it can continue.".to_string()
                        } else {
                            signal.message.trim().to_string()
                        };
                        metadata.insert("attention_state".to_string(), json!("needs_input"));
                        metadata.insert("attention_message".to_string(), json!(&message));
                        metadata.insert("attention_reason".to_string(), json!("agent_signal"));
                        process.details = Some(encode_metadata(&metadata)?);
                        supervisor.register(process.clone())?;
                        on_attention(GoalAgentAttention { message });
                    }
                    other => {
                        return Err(RefineError::InvalidInput(format!(
                            "Goal Agent wrote unsupported session state {other}"
                        )));
                    }
                }
            }

            if let Some(status) = child.try_wait().map_err(|error| {
                RefineError::Io(format!("failed to inspect Goal Agent process: {error}"))
            })? {
                break Ok(status);
            }
            thread::sleep(COMMAND_POLL_INTERVAL);
        }
    })();
    let status = match status_result {
        Ok(status) => status,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader_thread.join();
            process.state = "failed".to_string();
            let _ = supervisor.register(process);
            cleanup_session_artifacts(&command_path, &signal_path);
            return Err(error);
        }
    };

    let reader_result = reader_thread
        .join()
        .map_err(|_| RefineError::Io("Goal Agent output reader panicked".to_string()))
        .and_then(|result| result);
    if let Err(error) = reader_result {
        process.state = "failed".to_string();
        let _ = supervisor.register(process);
        cleanup_session_artifacts(&command_path, &signal_path);
        return Err(error);
    }
    let output = match fs::read(&stdout_path) {
        Ok(output) => String::from_utf8_lossy(&output).into_owned(),
        Err(error) => {
            process.state = "failed".to_string();
            let _ = supervisor.register(process);
            cleanup_session_artifacts(&command_path, &signal_path);
            return Err(RefineError::Io(format!(
                "failed to read Goal Agent transcript {}: {error}",
                stdout_path.display()
            )));
        }
    };
    // Give attached SSE readers one final polling interval to consume the fully
    // flushed transcript before process cleanup removes transient channels.
    thread::sleep(Duration::from_millis(120));
    process.state = if status.success() || completed_by_signal {
        "exited".to_string()
    } else {
        "failed".to_string()
    };
    process.exit_code = i32::try_from(status.exit_code()).ok();
    let _ = supervisor.register(process);
    cleanup_session_artifacts(&command_path, &signal_path);

    if !status.success() && !completed_by_signal {
        return Err(RefineError::Degraded(format!(
            "Goal Agent exited unsuccessfully: {}",
            status.exit_code()
        )));
    }
    Ok(GoalAgentResult {
        output: completion_report
            .unwrap_or_else(|| strip_terminal_control(&output).trim().to_string()),
        session_id,
        process_id,
    })
}

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

pub fn stop_agent_session(runtime_root: &Path, session_id: &str) -> RefineResult<()> {
    let (process, _) = session_process(runtime_root, session_id)?;
    FileProcessSupervisor::new(runtime_root).request_termination(&process.id, "terminate")?;
    Ok(())
}

pub fn agent_session_events_since(
    runtime_root: &Path,
    session_id: &str,
    after: u64,
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
    let start = after.min(length);
    file.seek(SeekFrom::Start(start)).map_err(|error| {
        RefineError::Io(format!(
            "failed to seek Goal Agent transcript {}: {error}",
            path.display()
        ))
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_EVENT_BYTES as u64)
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

fn append_command(
    runtime_root: &Path,
    session_id: &str,
    command: AgentSessionCommand,
) -> RefineResult<()> {
    let (process, metadata) = session_process(runtime_root, session_id)?;
    if !FileProcessSupervisor::process_is_alive(&process)? {
        return Err(RefineError::NotFound(format!(
            "Goal Agent session {session_id} is no longer running"
        )));
    }
    let path = metadata
        .get("command_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| {
            RefineError::NotFound(format!(
                "Goal Agent session {session_id} has no command channel"
            ))
        })?;
    let encoded = serde_json::to_string(&command).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Goal Agent command: {error}"))
    })?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open Goal Agent command channel {}: {error}",
                path.display()
            ))
        })?;
    writeln!(file, "{encoded}").map_err(|error| {
        RefineError::Io(format!(
            "failed to write Goal Agent command channel {}: {error}",
            path.display()
        ))
    })
}

fn session_process(
    runtime_root: &Path,
    session_id: &str,
) -> RefineResult<(ManagedProcess, Map<String, Value>)> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return Err(RefineError::InvalidInput(
            "terminal session id is required".to_string(),
        ));
    }
    FileProcessSupervisor::new(runtime_root)
        .list()?
        .into_iter()
        .find_map(|process| {
            let metadata = process_metadata(&process)?;
            (metadata.get("kind").and_then(Value::as_str) == Some("interactive_session")
                && metadata
                    .get("command_path")
                    .and_then(Value::as_str)
                    .is_some_and(|path| Path::new(path).is_file())
                && metadata.get("session_id").and_then(Value::as_str) == Some(session_id))
            .then_some((process, metadata))
        })
        .ok_or_else(|| {
            RefineError::NotFound(format!("terminal session {session_id} was not found"))
        })
}

fn snapshot_from_process(
    process: &ManagedProcess,
    metadata: &Map<String, Value>,
) -> RefineResult<AgentSessionSnapshot> {
    let alive = FileProcessSupervisor::process_is_alive(process)?;
    Ok(AgentSessionSnapshot {
        id: metadata
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        process_id: process.id.clone(),
        profile: metadata
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or("goal")
            .to_string(),
        provider: metadata
            .get("provider")
            .and_then(Value::as_str)
            .map(str::to_string),
        cwd: metadata
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default(),
        goal_id: metadata
            .get("goal_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        worktree: metadata.get("worktree").cloned(),
        attention_state: metadata
            .get("attention_state")
            .and_then(Value::as_str)
            .map(str::to_string),
        attention_message: metadata
            .get("attention_message")
            .and_then(Value::as_str)
            .map(str::to_string),
        alive,
        exited: !alive,
    })
}

fn process_metadata(process: &ManagedProcess) -> Option<Map<String, Value>> {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| details.as_object().cloned())
}

fn encode_metadata(metadata: &Map<String, Value>) -> RefineResult<String> {
    serde_json::to_string(metadata).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Goal Agent metadata: {error}"))
    })
}

fn read_commands_since(path: &Path, offset: &mut u64) -> RefineResult<Vec<AgentSessionCommand>> {
    let mut file = fs::File::open(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read Goal Agent command channel {}: {error}",
            path.display()
        ))
    })?;
    file.seek(SeekFrom::Start(*offset)).map_err(|error| {
        RefineError::Io(format!(
            "failed to seek Goal Agent command channel {}: {error}",
            path.display()
        ))
    })?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|error| {
        RefineError::Io(format!(
            "failed to read Goal Agent command channel {}: {error}",
            path.display()
        ))
    })?;
    let complete_len = text.rfind('\n').map(|index| index + 1).unwrap_or(0);
    *offset += complete_len as u64;
    text[..complete_len]
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                RefineError::Serialization(format!("failed to parse Goal Agent command: {error}"))
            })
        })
        .collect()
}

fn take_signal(path: &Path) -> RefineResult<Option<AgentSessionSignal>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read Goal Agent signal {}: {error}",
                path.display()
            )));
        }
    };
    let signal = match serde_json::from_slice(&bytes) {
        Ok(signal) => signal,
        Err(_) => return Ok(None),
    };
    fs::remove_file(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to consume Goal Agent signal {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(signal))
}

fn cleanup_session_artifacts(command_path: &Path, signal_path: &Path) {
    for path in [command_path, signal_path] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

fn goal_agent_protocol_prompt(prompt: &str, signal_path: &Path) -> String {
    let signal_path = signal_path.display().to_string();
    render(
        PromptTemplate::GoalAgentSession,
        &[("goal_prompt", prompt), ("signal_path", &signal_path)],
    )
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: if rows == 0 {
            DEFAULT_ROWS
        } else {
            rows.clamp(8, 80)
        },
        cols: if cols == 0 {
            DEFAULT_COLS
        } else {
            cols.clamp(20, 240)
        },
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn strip_terminal_control(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            if ch != '\r' {
                clean.push(ch);
            }
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn unique_temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("refine-{name}-{}", Uuid::new_v4()))
    }

    #[test]
    fn workflow_goal_agent_is_discoverable_and_attachable_while_running() {
        let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = unique_temp_dir("goal-agent-session");
        let runtime_root = root.join("run/8082");
        let app_root = root.join("app");
        let provider = root.join("smoke-ai");
        fs::create_dir_all(&app_root).unwrap();
        fs::write(
            &provider,
            "#!/bin/sh\nprintf 'ready\\n'\nread answer\nprintf 'answer:%s\\n' \"$answer\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&provider, permissions).unwrap();
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
        }

        let runtime_for_thread = runtime_root.clone();
        let app_for_thread = app_root.clone();
        let run = thread::spawn(move || {
            let mut metadata = Map::new();
            metadata.insert("goal_id".to_string(), json!("GOAL1"));
            run_goal_agent(
                GoalAgentLaunch {
                    runtime_root: runtime_for_thread,
                    cwd: app_for_thread,
                    provider: "smoke-ai".to_string(),
                    prompt: "test".to_string(),
                    metadata,
                },
                |_| {},
            )
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let snapshot = loop {
            if let Ok(snapshot) = find_goal_agent_session(&runtime_root, "GOAL1") {
                break snapshot;
            }
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        };
        assert!(snapshot.alive);
        let mut duplicate_metadata = Map::new();
        duplicate_metadata.insert("goal_id".to_string(), json!("GOAL1"));
        let duplicate = run_goal_agent(
            GoalAgentLaunch {
                runtime_root: runtime_root.clone(),
                cwd: app_root.clone(),
                provider: "smoke-ai".to_string(),
                prompt: "duplicate".to_string(),
                metadata: duplicate_metadata,
            },
            |_| {},
        );
        assert!(matches!(duplicate, Err(RefineError::Conflict(_))));
        send_agent_session_input(&runtime_root, &snapshot.id, "hello\r").unwrap();
        let result = run.join().unwrap().unwrap();
        assert!(result.output.contains("answer:hello"));
        assert!(matches!(
            find_agent_session(&runtime_root, &snapshot.id),
            Err(RefineError::NotFound(_))
        ));

        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workflow_goal_agent_surfaces_needs_input_and_continues_same_session() {
        let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = unique_temp_dir("goal-agent-needs-input");
        let runtime_root = root.join("run/8082");
        let app_root = root.join("app");
        let provider = root.join("smoke-ai");
        fs::create_dir_all(&app_root).unwrap();
        fs::write(
            &provider,
            "#!/bin/sh\n\
             printf '%s\\n' '{\"state\":\"needs_input\",\"message\":\"Choose the public name\"}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
             read answer\n\
             printf 'chosen:%s\\n' \"$answer\"\n\
             printf '%s\\n' '{\"state\":\"completed\",\"message\":\"Implemented and verified the selected public name.\"}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
             sleep 10\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&provider, permissions).unwrap();
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
        }

        let runtime_for_thread = runtime_root.clone();
        let app_for_thread = app_root.clone();
        let (attention_tx, attention_rx) = std::sync::mpsc::channel();
        let run = thread::spawn(move || {
            let mut metadata = Map::new();
            metadata.insert("goal_id".to_string(), json!("GOAL2"));
            run_goal_agent(
                GoalAgentLaunch {
                    runtime_root: runtime_for_thread,
                    cwd: app_for_thread,
                    provider: "smoke-ai".to_string(),
                    prompt: "test".to_string(),
                    metadata,
                },
                |attention| {
                    let _ = attention_tx.send(attention);
                },
            )
        });

        let attention = attention_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(attention.message, "Choose the public name");
        let snapshot = find_goal_agent_session(&runtime_root, "GOAL2").unwrap();
        assert_eq!(snapshot.attention_state.as_deref(), Some("needs_input"));
        assert_eq!(
            snapshot.attention_message.as_deref(),
            Some("Choose the public name")
        );
        send_agent_session_input(&runtime_root, &snapshot.id, "Refine\r").unwrap();
        let result = run.join().unwrap().unwrap();
        assert_eq!(
            result.output,
            "Implemented and verified the selected public name."
        );

        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn silent_goal_agent_remains_autonomous_without_requesting_input() {
        let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let root = unique_temp_dir("goal-agent-idle-attention");
        let runtime_root = root.join("run/8082");
        let app_root = root.join("app");
        let provider = root.join("smoke-ai");
        fs::create_dir_all(&app_root).unwrap();
        fs::write(
            &provider,
            "#!/bin/sh\nsleep 0.2\nprintf 'made-the-best-decision\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&provider).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&provider, permissions).unwrap();
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
        }

        let mut metadata = Map::new();
        metadata.insert("goal_id".to_string(), json!("GOAL3"));
        let mut attention = Vec::new();
        let result = run_goal_agent(
            GoalAgentLaunch {
                runtime_root,
                cwd: app_root,
                provider: "smoke-ai".to_string(),
                prompt: "test".to_string(),
                metadata,
            },
            |request| attention.push(request),
        )
        .unwrap();
        assert!(attention.is_empty());
        assert!(result.output.contains("made-the-best-decision"));

        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
            } else {
                std::env::remove_var("REFINE_SMOKE_AI_PATH");
            }
        }
        fs::remove_dir_all(root).unwrap();
    }
}

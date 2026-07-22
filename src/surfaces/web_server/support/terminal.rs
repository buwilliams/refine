use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use chrono::Utc;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};

const TERMINAL_INPUT_LIMIT: usize = 16_000;
const TERMINAL_EVENT_BACKLOG: usize = 1_000;
const TERMINAL_DEFAULT_COLS: u16 = 100;
const TERMINAL_DEFAULT_ROWS: u16 = 30;

#[derive(Clone, Debug)]
struct TerminalEvent {
    seq: u64,
    event: &'static str,
    data: String,
}

#[derive(Debug)]
struct TerminalEventLog {
    next_seq: u64,
    events: VecDeque<TerminalEvent>,
}

struct TerminalSession {
    id: String,
    process_id: String,
    profile: String,
    provider: Option<String>,
    cwd: PathBuf,
    supervisor: FileProcessSupervisor,
    process: ManagedProcess,
    stdout_path: PathBuf,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    child: Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>,
    events: Mutex<TerminalEventLog>,
    exited: AtomicBool,
}

static TERMINAL_SESSIONS: OnceLock<Mutex<BTreeMap<String, Arc<TerminalSession>>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub(in crate::surfaces::web_server) struct TerminalLaunchSpec {
    pub runtime_root: PathBuf,
    pub cwd: PathBuf,
    pub profile: String,
    pub provider: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub metadata: Map<String, Value>,
}

pub(in crate::surfaces::web_server) fn terminal_session_start_response(
    launch: TerminalLaunchSpec,
    cols: u16,
    rows: u16,
) -> RefineResult<Value> {
    let cwd = launch.cwd.canonicalize().map_err(|error| {
        RefineError::InvalidInput(format!(
            "terminal cwd {} is not available: {error}",
            launch.cwd.display()
        ))
    })?;
    let session = TerminalSession::spawn(launch, cwd, cols, rows)?;
    let id = session.id.clone();
    let process_id = session.process_id.clone();
    let profile = session.profile.clone();
    let provider = session.provider.clone();
    let cwd = session.cwd.display().to_string();
    let worktree = session
        .process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| details.get("worktree").cloned());
    sessions()
        .lock()
        .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
        .insert(id.clone(), session);
    Ok(json!({
        "id": id,
        "process_id": process_id,
        "profile": profile,
        "provider": provider,
        "cwd": cwd,
        "worktree": worktree,
    }))
}

pub(in crate::surfaces::web_server) fn terminal_input_response(
    session_id: &str,
    data: &str,
) -> RefineResult<Value> {
    if data.len() > TERMINAL_INPUT_LIMIT {
        return Err(RefineError::InvalidInput(format!(
            "terminal input is limited to {TERMINAL_INPUT_LIMIT} bytes"
        )));
    }
    let session = terminal_session(session_id)?;
    session.write_input(data.as_bytes())?;
    Ok(json!({"ok": true}))
}

pub(in crate::surfaces::web_server) fn terminal_resize_response(
    session_id: &str,
    cols: u16,
    rows: u16,
) -> RefineResult<Value> {
    let session = terminal_session(session_id)?;
    session.resize(cols, rows)?;
    Ok(json!({"ok": true}))
}

pub(in crate::surfaces::web_server) fn terminal_status_response(
    session_id: &str,
) -> RefineResult<Value> {
    let session = terminal_session(session_id)?;
    Ok(session.status_json())
}

pub(in crate::surfaces::web_server) fn terminal_stop_response(
    session_id: &str,
) -> RefineResult<Value> {
    let session = terminal_session(session_id)?;
    stop_terminal_session(&session)?;
    Ok(json!({"ok": true}))
}

pub(in crate::surfaces::web_server) fn terminal_stop_process_response(
    process_id: &str,
) -> RefineResult<Option<Value>> {
    let session = sessions()
        .lock()
        .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
        .values()
        .find(|session| session.process_id == process_id)
        .cloned();
    let Some(session) = session else {
        return Ok(None);
    };
    stop_terminal_session(&session)?;
    let mut process = session.process.clone();
    process.state = "stopping".to_string();
    Ok(Some(json!({
        "stopped": true,
        "process": process.api_json(),
    })))
}

fn stop_terminal_session(session: &Arc<TerminalSession>) -> RefineResult<()> {
    session.stop()?;
    sessions()
        .lock()
        .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
        .remove(&session.id);
    Ok(())
}

pub(in crate::surfaces::web_server) fn terminal_events_since(
    session_id: &str,
    after: u64,
) -> RefineResult<Vec<Value>> {
    let session = terminal_session(session_id)?;
    let events = session.events_since(after)?;
    Ok(events
        .into_iter()
        .map(|event| {
            json!({
                "seq": event.seq,
                "event": event.event,
                "data": event.data,
            })
        })
        .collect())
}

fn sessions() -> &'static Mutex<BTreeMap<String, Arc<TerminalSession>>> {
    TERMINAL_SESSIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn terminal_session(session_id: &str) -> RefineResult<Arc<TerminalSession>> {
    if session_id.trim().is_empty() {
        return Err(RefineError::InvalidInput(
            "terminal session id is required".to_string(),
        ));
    }
    sessions()
        .lock()
        .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
        .get(session_id)
        .cloned()
        .ok_or_else(|| {
            RefineError::NotFound(format!("terminal session {session_id} was not found"))
        })
}

impl TerminalSession {
    fn spawn(
        launch: TerminalLaunchSpec,
        cwd: PathBuf,
        cols: u16,
        rows: u16,
    ) -> RefineResult<Arc<Self>> {
        let owner = if launch.profile == "terminal" {
            ProcessOwner::UserHelper
        } else {
            ProcessOwner::Agent
        };
        let mut metadata = launch.metadata;
        let session_id = Uuid::new_v4().to_string();
        metadata.insert("kind".to_string(), json!("interactive_session"));
        metadata.insert("session_id".to_string(), json!(&session_id));
        metadata.insert("role".to_string(), json!(&launch.profile));
        if let Some(provider) = &launch.provider {
            metadata.insert("provider".to_string(), json!(provider));
        }
        let managed_spec = ManagedProcessSpec {
            owner: owner.clone(),
            command: launch.command.clone(),
            args: launch.args.clone(),
            cwd: Some(cwd.display().to_string()),
            env: vec![
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("COLORTERM".to_string(), "truecolor".to_string()),
                ("REFINE_TERMINAL".to_string(), "1".to_string()),
                ("REFINE_SESSION_ROLE".to_string(), launch.profile.clone()),
            ],
            stdin: None,
            limits: Some(ProcessResourceLimits {
                kill_on_parent_exit: true,
                ..Default::default()
            }),
            authorization_command: Some(
                std::iter::once(launch.command.as_str())
                    .chain(launch.args.iter().map(String::as_str))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            sensitive: false,
            metadata: metadata.clone(),
        };
        let supervisor = FileProcessSupervisor::new(&launch.runtime_root);
        supervisor.validate_interactive_launch(&managed_spec)?;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(pty_size(cols, rows))
            .map_err(|error| RefineError::Io(format!("failed to open terminal PTY: {error}")))?;
        let mut command = CommandBuilder::new(&launch.command);
        command.args(&launch.args);
        command.cwd(&cwd);
        for (key, value) in &managed_spec.env {
            command.env(key, value);
        }
        if owner == ProcessOwner::Agent {
            for key in [
                "ANTHROPIC_API_KEY",
                "CLAUDE_API_KEY",
                "CODEX_API_KEY",
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
                "GOOGLE_GENAI_API_KEY",
                "OPENAI_API_KEY",
            ] {
                command.env_remove(key);
            }
        }
        let mut child = pair.slave.spawn_command(command).map_err(|error| {
            RefineError::Io(format!(
                "failed to start interactive {} session: {error}",
                launch.profile
            ))
        })?;
        let pid = child.process_id();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| RefineError::Io(format!("failed to read terminal output: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| RefineError::Io(format!("failed to open terminal input: {error}")))?;
        drop(pair.slave);

        let process_id = format!("interactive-{session_id}");
        let stdout_path = supervisor
            .processes_dir()
            .join(format!("{process_id}.stdout.log"));
        fs::create_dir_all(supervisor.processes_dir()).map_err(|error| {
            RefineError::Io(format!(
                "failed to create interactive process registry {}: {error}",
                supervisor.processes_dir().display()
            ))
        })?;
        fs::File::create(&stdout_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create interactive process log {}: {error}",
                stdout_path.display()
            ))
        })?;
        let details = serde_json::to_string(&metadata).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode interactive process metadata: {error}"
            ))
        })?;
        let process = match supervisor.register(ManagedProcess {
            id: process_id.clone(),
            owner,
            pid,
            state: "running".to_string(),
            label: Some(match launch.profile.as_str() {
                "terminal" => "Terminal".to_string(),
                role => format!("{} agent", title_case(role)),
            }),
            details: Some(details),
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: None,
            stdin_path: None,
            limits: managed_spec.limits,
            started_at: Utc::now().to_rfc3339(),
            exit_code: None,
        }) {
            Ok(process) => process,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };

        let session = Arc::new(Self {
            id: session_id,
            process_id,
            profile: launch.profile,
            provider: launch.provider,
            cwd,
            supervisor,
            process,
            stdout_path,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(Some(child)),
            events: Mutex::new(TerminalEventLog {
                next_seq: 1,
                events: VecDeque::new(),
            }),
            exited: AtomicBool::new(false),
        });
        let reader_session = Arc::clone(&session);
        thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(count) => {
                        let text = String::from_utf8_lossy(&buf[..count]).to_string();
                        reader_session.append_process_output(text.as_bytes());
                        reader_session.push_event("terminal_output", text);
                    }
                    Err(error) => {
                        reader_session.push_event(
                            "terminal_error",
                            format!("terminal output stream failed: {error}"),
                        );
                        break;
                    }
                }
            }
            let exit = reader_session
                .child
                .lock()
                .ok()
                .and_then(|mut child| child.as_mut().and_then(|child| child.wait().ok()));
            let status = exit
                .as_ref()
                .map(|status| format!("{status:?}"))
                .unwrap_or_else(|| "closed".to_string());
            reader_session.exited.store(true, Ordering::Release);
            reader_session.finish_process(exit.as_ref());
            reader_session.push_event("terminal_exit", status);
        });
        Ok(session)
    }

    fn write_input(&self, data: &[u8]) -> RefineResult<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| RefineError::Io("terminal writer lock was poisoned".to_string()))?;
        writer
            .write_all(data)
            .and_then(|_| writer.flush())
            .map_err(|error| RefineError::Io(format!("failed to write terminal input: {error}")))
    }

    fn resize(&self, cols: u16, rows: u16) -> RefineResult<()> {
        let master = self
            .master
            .lock()
            .map_err(|_| RefineError::Io("terminal PTY lock was poisoned".to_string()))?;
        master
            .resize(pty_size(cols, rows))
            .map_err(|error| RefineError::Io(format!("failed to resize terminal: {error}")))
    }

    fn stop(&self) -> RefineResult<()> {
        let _ = self
            .supervisor
            .request_termination(&self.process_id, "terminate");
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            let exited = child.try_wait().map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect interactive {} session: {error}",
                    self.profile
                ))
            })?;
            if exited.is_none() {
                child.kill().map_err(|error| {
                    RefineError::Io(format!(
                        "failed to stop interactive {} session: {error}",
                        self.profile
                    ))
                })?;
            }
        }
        Ok(())
    }

    fn append_process_output(&self, bytes: &[u8]) {
        if let Ok(mut file) = OpenOptions::new().append(true).open(&self.stdout_path) {
            let _ = file.write_all(bytes);
            let _ = file.flush();
        }
    }

    fn finish_process(&self, status: Option<&portable_pty::ExitStatus>) {
        let mut process = self.process.clone();
        process.state = if status.is_some_and(|status| status.success()) {
            "exited".to_string()
        } else {
            "failed".to_string()
        };
        process.exit_code = status.and_then(|status| i32::try_from(status.exit_code()).ok());
        let _ = self.supervisor.register(process);
    }

    fn status_json(&self) -> Value {
        let worktree = self
            .process
            .details
            .as_deref()
            .and_then(|details| serde_json::from_str::<Value>(details).ok())
            .and_then(|details| details.get("worktree").cloned());
        let exited = self.exited.load(Ordering::Acquire);
        json!({
            "id": self.id,
            "process_id": self.process_id,
            "profile": self.profile,
            "provider": self.provider,
            "cwd": self.cwd.display().to_string(),
            "worktree": worktree,
            "alive": !exited,
            "exited": exited,
        })
    }

    fn push_event(&self, event: &'static str, data: String) {
        let Ok(mut log) = self.events.lock() else {
            return;
        };
        let seq = log.next_seq;
        log.next_seq += 1;
        log.events.push_back(TerminalEvent { seq, event, data });
        while log.events.len() > TERMINAL_EVENT_BACKLOG {
            log.events.pop_front();
        }
    }

    fn events_since(&self, after: u64) -> RefineResult<Vec<TerminalEvent>> {
        let log = self
            .events
            .lock()
            .map_err(|_| RefineError::Io("terminal event lock was poisoned".to_string()))?;
        Ok(log
            .events
            .iter()
            .filter(|event| event.seq > after)
            .cloned()
            .collect())
    }
}

pub(in crate::surfaces::web_server) fn default_interactive_shell() -> String {
    env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/bash".to_string())
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: if rows == 0 {
            TERMINAL_DEFAULT_ROWS
        } else {
            rows.clamp(8, 80)
        },
        cols: if cols == 0 {
            TERMINAL_DEFAULT_COLS
        } else {
            cols.clamp(20, 240)
        },
        pixel_width: 0,
        pixel_height: 0,
    }
}

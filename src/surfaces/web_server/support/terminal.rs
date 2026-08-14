use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::process::agent_sessions::{
    agent_session_events_range, find_agent_session, resize_agent_session, send_agent_session_input,
};
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::agent_providers::{PromptArtifactLease, PromptTransportMetadata};
use crate::tools::product::process_control::FileProcessControlService;

const TERMINAL_INPUT_LIMIT: usize = 16_000;
const TERMINAL_EVENT_BACKLOG: usize = 1_000;
/// How long a naturally-exited session stays registered so attached streams
/// can drain its final frames before the entry is dropped.
const TERMINAL_EXIT_RETENTION: Duration = Duration::from_secs(30);
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
    prompt_artifact: Mutex<Option<PromptArtifactLease>>,
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
    pub prompt_transport: Option<PromptTransportMetadata>,
    pub prompt_artifact: Option<PromptArtifactLease>,
    pub authorization_command: Option<String>,
    pub launch_environment: Option<crate::process::launch_environment::EffectiveLaunchEnvironment>,
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
    let mut response = session.launch_json();
    response["reattached"] = json!(false);
    let id = session.id.clone();
    sessions()
        .lock()
        .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
        .insert(id.clone(), session);
    Ok(response)
}

pub(in crate::surfaces::web_server) fn terminal_input_response(
    runtime_root: &std::path::Path,
    session_id: &str,
    data: &str,
) -> RefineResult<Value> {
    if data.len() > TERMINAL_INPUT_LIMIT {
        return Err(RefineError::InvalidInput(format!(
            "terminal input is limited to {TERMINAL_INPUT_LIMIT} bytes"
        )));
    }
    if let Some(session) = local_terminal_session(session_id)? {
        session.write_input(data.as_bytes())?;
    } else {
        send_agent_session_input(runtime_root, session_id, data)?;
    }
    Ok(json!({"ok": true}))
}

pub(in crate::surfaces::web_server) fn terminal_resize_response(
    runtime_root: &std::path::Path,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> RefineResult<Value> {
    if let Some(session) = local_terminal_session(session_id)? {
        session.resize(cols, rows)?;
    } else {
        resize_agent_session(runtime_root, session_id, cols, rows)?;
    }
    Ok(json!({"ok": true}))
}

pub(in crate::surfaces::web_server) fn terminal_status_response(
    runtime_root: &std::path::Path,
    session_id: &str,
) -> RefineResult<Value> {
    if let Some(session) = local_terminal_session(session_id)? {
        Ok(session.status_json())
    } else {
        serde_json::to_value(find_agent_session(runtime_root, session_id)?).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode Goal Agent session status: {error}"
            ))
        })
    }
}

pub(in crate::surfaces::web_server) fn terminal_stop_response(
    runtime_root: &Path,
    refine_dir: Option<&Path>,
    session_id: &str,
) -> RefineResult<Value> {
    if let Some(session) = local_terminal_session(session_id)? {
        let goal_linked = session
            .process
            .api_json()
            .get("goal_id")
            .and_then(Value::as_str)
            .is_some_and(|goal_id| !goal_id.trim().is_empty());
        if goal_linked {
            let service = match refine_dir {
                Some(refine_dir) => {
                    FileProcessControlService::with_refine_dir(runtime_root, refine_dir)
                }
                None => FileProcessControlService::new(runtime_root),
            };
            let result = service.stop(&session.process_id, "terminate")?;
            sessions()
                .lock()
                .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
                .remove(&session.id);
            Ok(result)
        } else {
            stop_terminal_session(&session)?;
            Ok(json!({"ok": true}))
        }
    } else {
        let snapshot = find_agent_session(runtime_root, session_id)?;
        let service = match refine_dir {
            Some(refine_dir) => {
                FileProcessControlService::with_refine_dir(runtime_root, refine_dir)
            }
            None => FileProcessControlService::new(runtime_root),
        };
        service.stop(&snapshot.process_id, "terminate")
    }
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
    runtime_root: &std::path::Path,
    session_id: &str,
    after: u64,
) -> RefineResult<Vec<Value>> {
    terminal_events_range(runtime_root, session_id, after, None)
}

pub(in crate::surfaces::web_server) fn terminal_events_range(
    runtime_root: &std::path::Path,
    session_id: &str,
    after: u64,
    before: Option<u64>,
) -> RefineResult<Vec<Value>> {
    if let Some(session) = local_terminal_session(session_id)? {
        let events = session
            .events_since(after)?
            .into_iter()
            .filter(|event| before.is_none_or(|before| event.seq <= before));
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
    } else {
        agent_session_events_range(runtime_root, session_id, after, before)
    }
}

fn sessions() -> &'static Mutex<BTreeMap<String, Arc<TerminalSession>>> {
    TERMINAL_SESSIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn local_terminal_session(session_id: &str) -> RefineResult<Option<Arc<TerminalSession>>> {
    if session_id.trim().is_empty() {
        return Err(RefineError::InvalidInput(
            "terminal session id is required".to_string(),
        ));
    }
    let session = sessions()
        .lock()
        .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
        .get(session_id)
        .cloned();
    Ok(session)
}

impl TerminalSession {
    fn launch_json(&self) -> Value {
        let details = self
            .process
            .details
            .as_deref()
            .and_then(|details| serde_json::from_str::<Value>(details).ok());
        let worktree = details
            .as_ref()
            .and_then(|details| details.get("worktree").cloned());
        let goal_id = details
            .as_ref()
            .and_then(|details| details.get("attached_goal_id").cloned());
        json!({
            "id": self.id,
            "process_id": self.process_id,
            "profile": self.profile,
            "provider": self.provider,
            "cwd": self.cwd.display().to_string(),
            "goal_id": goal_id,
            "worktree": worktree,
        })
    }

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
        if let Some(artifact) = &launch.prompt_artifact {
            artifact.validate()?;
        }
        if let Some(transport) = &launch.prompt_transport {
            metadata.insert(
                "prompt_transport".to_string(),
                serde_json::to_value(transport).map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to encode interactive prompt transport metadata: {error}"
                    ))
                })?,
            );
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
            authorization_command: launch.authorization_command.clone().or_else(|| {
                Some(
                    std::iter::once(launch.command.as_str())
                        .chain(launch.args.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }),
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
        let environment = match launch.launch_environment.as_ref() {
            Some(environment) => environment.clone(),
            None => crate::process::launch_environment::EffectiveLaunchEnvironment::assemble(
                &owner,
                &managed_spec.env,
            )?,
        };
        environment.validate_launch(&launch.command, &launch.args)?;
        environment.apply_to_pty(&mut command);
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
                "agent" => "Agent".to_string(),
                "standalone" => "Agent in Worktree".to_string(),
                "plan" => "Planing Agent".to_string(),
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
            prompt_artifact: Mutex::new(launch.prompt_artifact),
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
            reader_session.finish_process(exit.as_ref());
            reader_session.push_event("terminal_exit", status);
            reader_session.exited.store(true, Ordering::Release);
            // Attached streams poll every 80 ms and deliver the exit frame
            // well within the drain grace. After it, drop the dead session —
            // its PTY descriptors and scrollback otherwise stayed resident in
            // the registry for the life of the daemon, since only an explicit
            // stop removed entries. A reattach after removal reads as a closed
            // session, which is what it is.
            thread::sleep(TERMINAL_EXIT_RETENTION);
            if let Ok(mut sessions) = sessions().lock()
                && sessions
                    .get(&reader_session.id)
                    .is_some_and(|current| Arc::ptr_eq(current, &reader_session))
            {
                sessions.remove(&reader_session.id);
            }
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
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.exited.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if !self.exited.load(Ordering::Acquire) {
            return Err(RefineError::Degraded(format!(
                "interactive {} session did not finish cleanup within 2000 ms after termination",
                self.profile
            )));
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
        if let Ok(mut artifact) = self.prompt_artifact.lock() {
            artifact.take();
        }
    }

    fn status_json(&self) -> Value {
        let details = self
            .process
            .details
            .as_deref()
            .and_then(|details| serde_json::from_str::<Value>(details).ok());
        let worktree = details
            .as_ref()
            .and_then(|details| details.get("worktree").cloned());
        let goal_id = details
            .as_ref()
            .and_then(|details| details.get("attached_goal_id").cloned());
        let exited = self.exited.load(Ordering::Acquire);
        json!({
            "id": self.id,
            "process_id": self.process_id,
            "profile": self.profile,
            "provider": self.provider,
            "cwd": self.cwd.display().to_string(),
            "goal_id": goal_id,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use sha2::{Digest, Sha256};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn interactive_agent_profiles_start_independent_live_sessions() {
        let root =
            std::env::temp_dir().join(format!("refine-terminal-instances-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for profile in ["agent", "plan", "standalone"] {
            let launch = || TerminalLaunchSpec {
                runtime_root: root.join("run"),
                cwd: root.clone(),
                profile: profile.to_string(),
                provider: Some("test".to_string()),
                command: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), "sleep 10".to_string()],
                metadata: Map::new(),
                prompt_transport: None,
                prompt_artifact: None,
                authorization_command: None,
                launch_environment: None,
            };
            let first = terminal_session_start_response(launch(), 80, 24).unwrap();
            let second = terminal_session_start_response(launch(), 120, 36).unwrap();
            assert_ne!(first["id"], second["id"], "{profile}");
            assert_eq!(first["reattached"], false, "{profile}");
            assert_eq!(second["reattached"], false, "{profile}");
            terminal_stop_response(&root.join("run"), None, first["id"].as_str().unwrap()).unwrap();
            terminal_stop_response(&root.join("run"), None, second["id"].as_str().unwrap())
                .unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn interactive_pty_reads_oversized_prompt_from_owned_file() {
        let root =
            std::env::temp_dir().join(format!("refine-terminal-large-prompt-{}", Uuid::new_v4()));
        let runtime_root = root.join("run");
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let provider = bin_dir.join("smoke-ai");
        fs::write(
            &provider,
            concat!(
                "#!/bin/sh\n",
                "prompt_path=$(printf '%s' \"$1\" | sed -n '4{s/^`//;s/`$//;p;}')\n",
                "printf 'prompt-bytes:%s\\n' \"$(wc -c < \"$prompt_path\")\"\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
        let service = crate::tools::host::agent_providers::HostAgentProviderService {
            path_override: Some(bin_dir.display().to_string()),
            runtime_root: Some(runtime_root.clone()),
        };
        let command = service
            .interactive_command("smoke-ai", &"x".repeat(158_078))
            .unwrap();
        let artifact_path = command
            .prompt_artifact
            .as_ref()
            .unwrap()
            .path()
            .to_path_buf();
        let response = terminal_session_start_response(
            TerminalLaunchSpec {
                runtime_root: runtime_root.clone(),
                cwd: root.clone(),
                profile: "agent".to_string(),
                provider: Some("smoke-ai".to_string()),
                command: command.binary,
                args: command.args,
                metadata: Map::new(),
                prompt_transport: Some(command.prompt_transport),
                prompt_artifact: command.prompt_artifact,
                authorization_command: Some(command.authorization_command),
                launch_environment: Some(command.launch_environment),
            },
            80,
            24,
        )
        .unwrap();
        let session_id = response["id"].as_str().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !terminal_status_response(&runtime_root, session_id).unwrap()["exited"]
            .as_bool()
            .unwrap()
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let output = terminal_events_since(&runtime_root, session_id, 0)
            .unwrap()
            .into_iter()
            .filter_map(|event| event["data"].as_str().map(str::to_string))
            .collect::<String>();
        assert!(output.contains("prompt-bytes:158078"), "{output:?}");
        assert!(!artifact_path.exists());
        sessions().lock().unwrap().remove(session_id);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn interactive_pty_reselects_file_transport_for_final_session_environment() {
        let root = std::env::temp_dir().join(format!(
            "refine-terminal-final-environment-{}",
            Uuid::new_v4()
        ));
        let runtime_root = root.join("run");
        let bin_dir = root.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let provider = bin_dir.join("smoke-ai");
        fs::write(
            &provider,
            concat!(
                "#!/bin/sh\n",
                "prompt_path=$(printf '%s' \"$1\" | sed -n '4{s/^`//;s/`$//;p;}')\n",
                "printf 'prompt-sha:%s\\n' \"$(sha256sum \"$prompt_path\" | cut -d' ' -f1)\"\n",
                "printf 'session-role:%s\\n' \"$REFINE_SESSION_ROLE\"\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
        let mut environment = (0..23)
            .map(|index| (format!("REFINE_LARGE_{index}"), "e".repeat(65_800)))
            .collect::<Vec<_>>();
        environment.extend([
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("REFINE_TERMINAL".to_string(), "1".to_string()),
            ("REFINE_SESSION_ROLE".to_string(), "agent".to_string()),
        ]);
        let prompt = format!(
            "INTERACTIVE_FINAL_ENV_SECRET{}",
            "x".repeat(60_000 - "INTERACTIVE_FINAL_ENV_SECRET".len())
        );
        let expected_digest = format!("{:x}", Sha256::digest(prompt.as_bytes()));
        let service = crate::tools::host::agent_providers::HostAgentProviderService {
            path_override: Some(bin_dir.display().to_string()),
            runtime_root: Some(runtime_root.clone()),
        };
        let command = service
            .interactive_command_with_environment("smoke-ai", &prompt, &environment)
            .unwrap();
        assert_eq!(
            command.prompt_transport.kind,
            crate::tools::host::agent_providers::PromptTransportKind::File
        );
        let response = terminal_session_start_response(
            TerminalLaunchSpec {
                runtime_root: runtime_root.clone(),
                cwd: root.clone(),
                profile: "agent".to_string(),
                provider: Some("smoke-ai".to_string()),
                command: command.binary,
                args: command.args,
                metadata: Map::new(),
                prompt_transport: Some(command.prompt_transport),
                prompt_artifact: command.prompt_artifact,
                authorization_command: Some(command.authorization_command),
                launch_environment: Some(command.launch_environment),
            },
            80,
            24,
        )
        .unwrap();
        let session_id = response["id"].as_str().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !terminal_status_response(&runtime_root, session_id).unwrap()["exited"]
            .as_bool()
            .unwrap()
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
        let output = terminal_events_since(&runtime_root, session_id, 0)
            .unwrap()
            .into_iter()
            .filter_map(|event| event["data"].as_str().map(str::to_string))
            .collect::<String>();
        assert!(
            output.contains(&format!("prompt-sha:{expected_digest}")),
            "{output:?}"
        );
        assert!(output.contains("session-role:agent"), "{output:?}");
        sessions().lock().unwrap().remove(session_id);
        fs::remove_dir_all(root).unwrap();
    }
}

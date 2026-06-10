use std::collections::{BTreeMap, VecDeque};
use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::core::supervisor::errors::{RefineError, RefineResult};

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
    cwd: PathBuf,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    child: Mutex<Option<Box<dyn portable_pty::Child + Send + Sync>>>,
    events: Mutex<TerminalEventLog>,
}

static TERMINAL_SESSIONS: OnceLock<Mutex<BTreeMap<String, Arc<TerminalSession>>>> = OnceLock::new();

pub(in crate::surfaces::web_server) fn terminal_session_start_response(
    source_root: &Path,
    cols: u16,
    rows: u16,
) -> RefineResult<Value> {
    let cwd = source_root.canonicalize().map_err(|error| {
        RefineError::InvalidInput(format!(
            "terminal cwd {} is not available: {error}",
            source_root.display()
        ))
    })?;
    let session = TerminalSession::spawn(cwd, cols, rows)?;
    let id = session.id.clone();
    let cwd = session.cwd.display().to_string();
    sessions()
        .lock()
        .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
        .insert(id.clone(), session);
    Ok(json!({
        "id": id,
        "cwd": cwd,
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

pub(in crate::surfaces::web_server) fn terminal_stop_response(
    session_id: &str,
) -> RefineResult<Value> {
    let session = terminal_session(session_id)?;
    session.stop();
    sessions()
        .lock()
        .map_err(|_| RefineError::Io("terminal session lock was poisoned".to_string()))?
        .remove(session_id);
    Ok(json!({"ok": true}))
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
    fn spawn(cwd: PathBuf, cols: u16, rows: u16) -> RefineResult<Arc<Self>> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(pty_size(cols, rows))
            .map_err(|error| RefineError::Io(format!("failed to open terminal PTY: {error}")))?;
        let shell = env::var("SHELL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "/bin/bash".to_string());
        let mut command = CommandBuilder::new(shell);
        command.arg("-i");
        command.cwd(&cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("REFINE_TERMINAL", "1");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| RefineError::Io(format!("failed to start terminal shell: {error}")))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| RefineError::Io(format!("failed to read terminal output: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| RefineError::Io(format!("failed to open terminal input: {error}")))?;
        drop(pair.slave);

        let session = Arc::new(Self {
            id: Uuid::new_v4().to_string(),
            cwd,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(Some(child)),
            events: Mutex::new(TerminalEventLog {
                next_seq: 1,
                events: VecDeque::new(),
            }),
        });
        let reader_session = Arc::clone(&session);
        thread::spawn(move || {
            let mut buf = [0_u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(count) => {
                        let text = String::from_utf8_lossy(&buf[..count]).to_string();
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
            let status = reader_session
                .child
                .lock()
                .ok()
                .and_then(|mut child| child.as_mut().and_then(|child| child.wait().ok()))
                .map(|status| format!("{status:?}"))
                .unwrap_or_else(|| "closed".to_string());
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

    fn stop(&self) {
        if let Ok(mut child) = self.child.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.kill();
        }
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

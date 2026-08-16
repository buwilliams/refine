use super::*;

pub(super) fn append_command(
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

pub(super) fn session_process(
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

pub(super) fn snapshot_from_process(
    process: &ManagedProcess,
    metadata: &Map<String, Value>,
) -> RefineResult<AgentSessionSnapshot> {
    let alive = FileProcessSupervisor::process_is_alive(process)?;
    let transcript_bytes = process
        .stdout_path
        .as_deref()
        .and_then(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .unwrap_or(0);
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
        transcript_bytes,
        alive,
        exited: !alive,
    })
}

pub(super) fn process_metadata(process: &ManagedProcess) -> Option<Map<String, Value>> {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| details.as_object().cloned())
}

pub(super) fn encode_metadata(metadata: &Map<String, Value>) -> RefineResult<String> {
    serde_json::to_string(metadata).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Goal Agent metadata: {error}"))
    })
}

pub(super) fn read_commands_since(
    path: &Path,
    offset: &mut u64,
) -> RefineResult<Vec<AgentSessionCommand>> {
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

#[derive(Debug)]
struct IncompleteSignal {
    fingerprint: u64,
    first_seen: std::time::Instant,
}

#[derive(Debug)]
pub(super) struct SignalReader {
    incomplete: Option<IncompleteSignal>,
    write_grace_period: Duration,
    requires_planning_result: bool,
}

#[derive(Debug)]
pub(super) enum SignalRead {
    Pending,
    Valid(AgentSessionSignal),
    Rejected(String),
}

impl Default for SignalReader {
    fn default() -> Self {
        Self::new(SIGNAL_WRITE_GRACE_PERIOD)
    }
}

impl SignalReader {
    pub(super) fn new(write_grace_period: Duration) -> Self {
        Self {
            incomplete: None,
            write_grace_period,
            requires_planning_result: false,
        }
    }

    /// Reject completed signals that omit `planning_result` instead of letting
    /// them settle as prose: planning phases require the structured object, and
    /// an in-session rewrite is far cheaper than a downstream decode failure.
    pub(super) fn requiring_planning_result(mut self, required: bool) -> Self {
        self.requires_planning_result = required;
        self
    }

    pub(super) fn take(&mut self, path: &Path) -> RefineResult<SignalRead> {
        self.read(path, false)
    }

    pub(super) fn finish(&mut self, path: &Path) -> RefineResult<SignalRead> {
        self.read(path, true)
    }

    fn read(&mut self, path: &Path, writer_finished: bool) -> RefineResult<SignalRead> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.incomplete = None;
                return Ok(SignalRead::Pending);
            }
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read Goal Agent signal {}: {error}",
                    path.display()
                )));
            }
        };
        let value = match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => {
                self.incomplete = None;
                value
            }
            Err(error) => {
                let fingerprint = signal_fingerprint(&bytes);
                let now = std::time::Instant::now();
                match &mut self.incomplete {
                    Some(incomplete) if incomplete.fingerprint == fingerprint => {
                        if writer_finished
                            || now.duration_since(incomplete.first_seen) >= self.write_grace_period
                        {
                            let elapsed = now.duration_since(incomplete.first_seen).as_millis();
                            self.incomplete = None;
                            return Ok(SignalRead::Rejected(format!(
                                "invalid JSON after {} ms: {error}",
                                if writer_finished {
                                    elapsed
                                } else {
                                    self.write_grace_period.as_millis()
                                }
                            )));
                        }
                    }
                    observation => {
                        *observation = Some(IncompleteSignal {
                            fingerprint,
                            first_seen: now,
                        });
                        if writer_finished {
                            self.incomplete = None;
                            return Ok(SignalRead::Rejected(format!(
                                "invalid JSON when the Goal Agent exited: {error}"
                            )));
                        }
                    }
                }
                return Ok(SignalRead::Pending);
            }
        };
        self.incomplete = None;
        let signal: AgentSessionSignal = match serde_path_to_error::deserialize(value) {
            Ok(signal) => signal,
            Err(error) => {
                return Ok(SignalRead::Rejected(format!(
                    "does not match the required schema: {error}"
                )));
            }
        };
        if self.requires_planning_result
            && matches!(signal.state, AgentSessionState::Completed)
            && signal.planning_result.is_none()
        {
            return Ok(SignalRead::Rejected(
                "completion signal omitted the required planning_result object".to_string(),
            ));
        }
        fs::remove_file(path).map_err(|error| {
            RefineError::Io(format!(
                "failed to consume Goal Agent signal {}: {error}",
                path.display()
            ))
        })?;
        Ok(SignalRead::Valid(signal))
    }
}

fn signal_fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn cleanup_session_artifacts(command_path: &Path, signal_path: &Path) {
    for path in [command_path, signal_path] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
}

pub(super) fn goal_agent_protocol_prompt(
    prompt: &str,
    signal_path: &Path,
    implementation_phase: Option<&str>,
) -> String {
    let completion_contract = completion_contract(signal_path, implementation_phase);
    render(
        PromptTemplate::GoalAgentSession,
        &[
            ("goal_prompt", prompt),
            ("completion_contract", &completion_contract),
        ],
    )
}

fn completion_contract(signal_path: &Path, implementation_phase: Option<&str>) -> String {
    let destination = signal_path.display();
    let write_protocol = format!(
        "Write the complete JSON to `{destination}.tmp`, parse-check that temporary file with `jq .` or another JSON parser, and only after it parses atomically replace `{destination}` with `mv` or an equivalent rename. Never write the completion payload directly to `{destination}`."
    );
    match implementation_phase {
        Some("plan") => {
            let planning_result =
                crate::prompts::implementation_planning::plan_result_contract_json();
            format!(
                "Choose applicable Guidance. On completion, produce one JSON object with `state`, a brief `message`, `guidance_applied`, and the required `planning_result`. `guidance_applied` must contain zero-based integer indexes, such as `[0]`, for applicable Guidance candidates; use `[]` when none apply, and never use names. `planning_result` must be a JSON object, never omitted or quoted as a string. Use this complete signal shape: `{{\"state\":\"completed\",\"message\":\"brief planning summary\",\"guidance_applied\":[0],\"planning_result\":{planning_result}}}`. {write_protocol}"
            )
        }
        Some("criticize") => {
            let planning_result =
                crate::prompts::implementation_planning::criticism_result_contract_json();
            format!(
                "Choose applicable Guidance. On completion, produce one JSON object with `state`, a brief `message`, `guidance_applied`, and the required `planning_result`. `guidance_applied` must contain zero-based integer indexes, such as `[0]`, for applicable Guidance candidates; use `[]` when none apply, and never use names. `planning_result` must be a JSON object, never omitted or quoted as a string. Use this complete signal shape, with `findings` empty when nothing material was found: `{{\"state\":\"completed\",\"message\":\"brief criticism summary\",\"guidance_applied\":[0],\"planning_result\":{planning_result}}}`. {write_protocol}"
            )
        }
        Some("revise") => {
            let planning_result =
                crate::prompts::implementation_planning::revision_result_contract_json();
            format!(
                "Choose applicable Guidance. On completion, produce one JSON object with `state`, a brief `message`, `guidance_applied`, and the required `planning_result`. `guidance_applied` must contain zero-based integer indexes, such as `[0]`, for applicable Guidance candidates; use `[]` when none apply, and never use names. `planning_result` must be a JSON object, never omitted or quoted as a string. Use this complete signal shape with one populated canonical `criticism_id` and `resolution` entry per material finding: `{{\"state\":\"completed\",\"message\":\"brief revision summary\",\"guidance_applied\":[0],\"planning_result\":{planning_result}}}`. {write_protocol}"
            )
        }
        _ => {
            let implementation_evidence =
                crate::prompts::implementation_planning::implementation_evidence_contract_json();
            format!(
                "Report changes and exact verification. Choose applicable Guidance. On completion, produce `{{\"state\":\"completed\",\"message\":\"changes and exact verification\",\"guidance_applied\":[0],\"implementation_evidence\":{implementation_evidence}}}`, replacing `[0]` with applicable indexes and each checklist `outcome` with one of `completed|deviated|rejected|blocked`. Guidance makes the field required, though it may be empty. A governed implementation checklist requires evidence for every stable ID without altering the accepted plan. {write_protocol}"
            )
        }
    }
}

pub(super) fn pty_size(cols: u16, rows: u16) -> PtySize {
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

pub(super) fn strip_terminal_control(text: &str) -> String {
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

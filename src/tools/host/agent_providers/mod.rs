use std::env;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessOutputStream, ManagedProcessSpec, ProcessOwner,
    ProcessResourceLimits,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderCapability {
    pub name: String,
    pub display_name: String,
    pub binary: String,
    pub installed: bool,
    pub path: Option<String>,
    pub supports_resume: bool,
    pub supports_direct_api: bool,
    pub supports_cli: bool,
    pub output_format: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderInvocation {
    pub provider: String,
    pub prompt: String,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub process_metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderInvocationResult {
    pub output: String,
    pub provider_session_id: Option<String>,
    pub raw_output: String,
}

pub trait AgentProviderService {
    fn detect(&self) -> RefineResult<Vec<ProviderCapability>>;
    fn configure(&self, provider: &str) -> RefineResult<()>;
    fn authenticate(&self, provider: &str) -> RefineResult<()>;
    fn invoke(&self, invocation: ProviderInvocation) -> RefineResult<String>;
    fn resume(&self, provider: &str, session_id: &str) -> RefineResult<String>;
    fn diagnose(&self, provider: &str) -> RefineResult<Vec<String>>;
}

#[cfg(test)]
pub fn smoke_ai_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Clone, Debug, Default)]
pub struct HostAgentProviderService {
    pub path_override: Option<String>,
    pub runtime_root: Option<PathBuf>,
}

impl HostAgentProviderService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_runtime_root(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: Some(runtime_root.into()),
            ..Self::default()
        }
    }

    fn spec(provider: &str) -> Option<ProviderSpec> {
        match provider {
            "claude" => Some(ProviderSpec::new(
                "claude",
                "Claude Code",
                "claude",
                "claude_json",
                true,
                false,
            )),
            "codex" => Some(ProviderSpec::new(
                "codex",
                "OpenAI Codex",
                "codex",
                "codex_json",
                true,
                false,
            )),
            "gemini" => Some(ProviderSpec::new(
                "gemini", "Gemini", "gemini", "plain", false, false,
            )),
            "copilot" => Some(ProviderSpec::new(
                "copilot",
                "GitHub Copilot",
                "copilot",
                "copilot_json",
                false,
                false,
            )),
            "smoke-ai" => Some(ProviderSpec::new(
                "smoke-ai", "Smoke AI", "smoke-ai", "plain", false, false,
            )),
            _ => None,
        }
    }

    fn specs() -> Vec<ProviderSpec> {
        ["claude", "codex", "gemini", "copilot", "smoke-ai"]
            .into_iter()
            .filter_map(Self::spec)
            .collect()
    }

    fn detect_spec(&self, spec: ProviderSpec) -> ProviderCapability {
        let smoke_ai_binary = (spec.name == "smoke-ai")
            .then(|| self.smoke_ai_binary(&spec))
            .flatten();
        let binary = smoke_ai_binary
            .clone()
            .unwrap_or_else(|| spec.binary.to_string());
        let path = if spec.name == "smoke-ai" && smoke_ai_binary.is_none() {
            None
        } else {
            find_executable(&binary, self.path_override.as_deref())
        };
        ProviderCapability {
            name: spec.name.to_string(),
            display_name: spec.display_name.to_string(),
            binary,
            installed: path.is_some(),
            path: path.map(|path| path.display().to_string()),
            supports_resume: spec.supports_resume,
            supports_direct_api: spec.supports_direct_api,
            supports_cli: true,
            output_format: spec.output_format.to_string(),
        }
    }

    fn smoke_ai_binary(&self, spec: &ProviderSpec) -> Option<String> {
        if self
            .path_override
            .as_deref()
            .and_then(|path| find_executable(spec.binary, Some(path)))
            .is_some()
        {
            return Some(spec.binary.to_string());
        }
        env::var("REFINE_SMOKE_AI_PATH")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    }

    fn resolve_binary_for_provider(&self, provider: &str) -> RefineResult<(ProviderSpec, String)> {
        let spec = Self::spec(provider)
            .ok_or_else(|| RefineError::InvalidInput(format!("unknown provider {provider}")))?;
        let capability = self.detect_spec(spec.clone());
        let Some(path) = capability.path.or_else(|| {
            if capability.installed {
                Some(capability.binary.clone())
            } else {
                None
            }
        }) else {
            return Err(RefineError::Degraded(format!(
                "{} CLI was not found on PATH",
                capability.display_name
            )));
        };
        Ok((spec, path))
    }

    pub fn invoke_detailed(
        &self,
        invocation: ProviderInvocation,
    ) -> RefineResult<ProviderInvocationResult> {
        self.invoke_detailed_with_output(invocation, |_| {})
    }

    pub fn invoke_detailed_with_output<F>(
        &self,
        invocation: ProviderInvocation,
        on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        let (spec, binary) = self.resolve_binary_for_provider(&invocation.provider)?;
        let cwd = invocation.cwd.as_deref().map(Path::new);
        let args = spec.chat_args(
            &binary,
            &invocation.prompt,
            invocation.session_id.as_deref(),
            cwd,
        );
        self.run_provider_command_result_with_output(
            &args,
            cwd,
            spec.output_format,
            invocation.process_metadata,
            on_output,
        )
    }

    pub fn resume_detailed(
        &self,
        provider: &str,
        session_id: &str,
    ) -> RefineResult<ProviderInvocationResult> {
        self.resume_detailed_with_output(provider, session_id, |_| {})
    }

    pub fn resume_detailed_with_output<F>(
        &self,
        provider: &str,
        session_id: &str,
        on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        let (spec, binary) = self.resolve_binary_for_provider(provider)?;
        if !spec.supports_resume {
            return Err(RefineError::InvalidInput(format!(
                "{} does not support provider-session resume",
                spec.display_name
            )));
        }
        let args = spec.chat_args(&binary, "", Some(session_id), None);
        self.run_provider_command_result_with_output(
            &args,
            None,
            spec.output_format,
            Default::default(),
            on_output,
        )
    }

    fn run_provider_command_result_with_output<F>(
        &self,
        args: &[String],
        cwd: Option<&Path>,
        output_format: &str,
        process_metadata: Map<String, Value>,
        mut on_output: F,
    ) -> RefineResult<ProviderInvocationResult>
    where
        F: FnMut(String),
    {
        let Some((binary, rest)) = args.split_first() else {
            return Err(RefineError::InvalidInput(
                "provider command cannot be empty".to_string(),
            ));
        };
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("run/agent-processes"));
        let mut formatter = ProviderActivityFormatter::new(output_format);
        let output = FileProcessSupervisor::new(runtime_root).run_to_completion_with_output(
            ManagedProcessSpec {
                owner: ProcessOwner::Agent,
                command: binary.to_string(),
                args: rest.to_vec(),
                cwd: cwd.map(|path| path.display().to_string()),
                env: Vec::new(),
                stdin: None,
                limits: Some(ProcessResourceLimits {
                    kill_on_parent_exit: true,
                    ..Default::default()
                }),
                authorization_command: Some(args.join(" ")),
                sensitive: false,
                metadata: process_metadata,
            },
            |stream, bytes| {
                for line in formatter.push(stream, bytes) {
                    on_output(line);
                }
            },
        )?;
        for line in formatter.finish() {
            on_output(line);
        }
        let success = output.success();
        let exit_code = output.process.exit_code;
        let stdout = output.stdout;
        let stderr = output.stderr;
        if !success {
            let message = provider_error_message(&stdout, &stderr)
                .or_else(|| last_non_empty_line(&stderr))
                .or_else(|| last_non_empty_line(&stdout))
                .unwrap_or_else(|| {
                    let exit = exit_code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "unknown".to_string());
                    format!("provider command exited {exit}")
                });
            return Err(RefineError::Degraded(message));
        }
        if let Some(message) = provider_error_message(&stdout, &stderr) {
            return Err(RefineError::Degraded(message));
        }
        let final_text = extract_final_text(&stdout, output_format);
        let provider_session_id = extract_provider_session_id(&stdout);
        if final_text.trim().is_empty() {
            Ok(ProviderInvocationResult {
                output: stdout.clone(),
                provider_session_id,
                raw_output: stdout,
            })
        } else {
            Ok(ProviderInvocationResult {
                output: final_text,
                provider_session_id,
                raw_output: stdout,
            })
        }
    }
}

impl AgentProviderService for HostAgentProviderService {
    fn detect(&self) -> RefineResult<Vec<ProviderCapability>> {
        Ok(Self::specs()
            .into_iter()
            .map(|spec| self.detect_spec(spec))
            .collect())
    }

    fn configure(&self, provider: &str) -> RefineResult<()> {
        Self::spec(provider)
            .map(|_| ())
            .ok_or_else(|| RefineError::InvalidInput(format!("unknown provider {provider}")))
    }

    fn authenticate(&self, provider: &str) -> RefineResult<()> {
        let capability = self
            .detect_spec(Self::spec(provider).ok_or_else(|| {
                RefineError::InvalidInput(format!("unknown provider {provider}"))
            })?);
        if capability.installed {
            Ok(())
        } else {
            Err(RefineError::Degraded(format!(
                "{} CLI was not found on PATH",
                capability.display_name
            )))
        }
    }

    fn invoke(&self, invocation: ProviderInvocation) -> RefineResult<String> {
        self.invoke_detailed(invocation).map(|result| result.output)
    }

    fn resume(&self, provider: &str, session_id: &str) -> RefineResult<String> {
        self.resume_detailed(provider, session_id)
            .map(|result| result.output)
    }

    fn diagnose(&self, provider: &str) -> RefineResult<Vec<String>> {
        let capability = self
            .detect_spec(Self::spec(provider).ok_or_else(|| {
                RefineError::InvalidInput(format!("unknown provider {provider}"))
            })?);
        if capability.installed {
            Ok(vec![format!(
                "{} CLI found at {}",
                capability.display_name,
                capability.path.unwrap_or_default()
            )])
        } else {
            Ok(vec![format!(
                "{} CLI not found; install it and run its login command on the host",
                capability.display_name
            )])
        }
    }
}

#[derive(Clone, Debug)]
struct ProviderSpec {
    name: &'static str,
    display_name: &'static str,
    binary: &'static str,
    output_format: &'static str,
    supports_resume: bool,
    supports_direct_api: bool,
}

impl ProviderSpec {
    fn new(
        name: &'static str,
        display_name: &'static str,
        binary: &'static str,
        output_format: &'static str,
        supports_resume: bool,
        supports_direct_api: bool,
    ) -> Self {
        Self {
            name,
            display_name,
            binary,
            output_format,
            supports_resume,
            supports_direct_api,
        }
    }

    fn agent_args(&self, binary_path: &str, prompt: &str, cwd: Option<&Path>) -> Vec<String> {
        match self.name {
            "claude" => vec![
                binary_path.to_string(),
                "--print".to_string(),
                "--output-format=stream-json".to_string(),
                "--verbose".to_string(),
                "--dangerously-skip-permissions".to_string(),
                prompt.to_string(),
            ],
            "codex" => {
                let mut args = vec![
                    binary_path.to_string(),
                    "exec".to_string(),
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "--json".to_string(),
                ];
                if let Some(cwd) = cwd {
                    args.extend(["-C".to_string(), cwd.display().to_string()]);
                }
                args.push(prompt.to_string());
                args
            }
            "gemini" => vec![
                binary_path.to_string(),
                "--yolo".to_string(),
                "-p".to_string(),
                prompt.to_string(),
            ],
            "copilot" => {
                let mut args = vec![
                    binary_path.to_string(),
                    "--allow-all".to_string(),
                    "--output-format".to_string(),
                    "json".to_string(),
                    "--no-color".to_string(),
                    "--no-auto-update".to_string(),
                ];
                if let Some(cwd) = cwd {
                    args.extend(["-C".to_string(), cwd.display().to_string()]);
                }
                args.extend(["-p".to_string(), prompt.to_string()]);
                args
            }
            "smoke-ai" => vec![binary_path.to_string(), prompt.to_string()],
            _ => vec![binary_path.to_string(), prompt.to_string()],
        }
    }

    fn chat_args(
        &self,
        binary_path: &str,
        prompt: &str,
        session_id: Option<&str>,
        cwd: Option<&Path>,
    ) -> Vec<String> {
        match self.name {
            "claude" => {
                let mut args = vec![
                    binary_path.to_string(),
                    "--print".to_string(),
                    "--output-format=stream-json".to_string(),
                    "--verbose".to_string(),
                ];
                if let Some(session_id) = session_id {
                    args.extend(["--resume".to_string(), session_id.to_string()]);
                }
                if !prompt.is_empty() {
                    args.push(prompt.to_string());
                }
                args
            }
            "codex" if session_id.is_some() => {
                let mut args = vec![
                    binary_path.to_string(),
                    "exec".to_string(),
                    "resume".to_string(),
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                    "--json".to_string(),
                    session_id.unwrap_or_default().to_string(),
                ];
                if !prompt.is_empty() {
                    args.push(prompt.to_string());
                }
                args
            }
            "copilot" if session_id.is_some() => {
                let mut args = vec![
                    binary_path.to_string(),
                    "--allow-all".to_string(),
                    "--output-format".to_string(),
                    "json".to_string(),
                    "--no-color".to_string(),
                    "--no-auto-update".to_string(),
                ];
                if let Some(cwd) = cwd {
                    args.extend(["-C".to_string(), cwd.display().to_string()]);
                }
                args.push(format!("--resume={}", session_id.unwrap_or_default()));
                if !prompt.is_empty() {
                    args.extend(["-p".to_string(), prompt.to_string()]);
                }
                args
            }
            _ => self.agent_args(binary_path, prompt, cwd),
        }
    }
}

fn find_executable(binary: &str, path_override: Option<&str>) -> Option<PathBuf> {
    let candidate = Path::new(binary);
    if candidate.components().count() > 1 {
        return executable_file(candidate).then(|| candidate.to_path_buf());
    }
    let path = path_override
        .map(str::to_string)
        .or_else(|| env::var("PATH").ok())
        .unwrap_or_default();
    env::split_paths(&path)
        .chain(user_executable_dirs(path_override))
        .map(|dir| dir.join(binary))
        .find(|path| executable_file(path))
}

fn user_executable_dirs(path_override: Option<&str>) -> Vec<PathBuf> {
    if path_override.is_some() {
        return Vec::new();
    }
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        home.join(".local/bin"),
        home.join(".npm-global/bin"),
        home.join(".cargo/bin"),
    ]
    .into_iter()
    .collect()
}

fn executable_file(path: &Path) -> bool {
    path.is_file()
}

struct ProviderActivityFormatter {
    output_format: String,
    stdout_buffer: String,
    stderr_buffer: String,
}

impl ProviderActivityFormatter {
    fn new(output_format: &str) -> Self {
        Self {
            output_format: output_format.to_string(),
            stdout_buffer: String::new(),
            stderr_buffer: String::new(),
        }
    }

    fn push(&mut self, stream: ManagedProcessOutputStream, bytes: &[u8]) -> Vec<String> {
        let chunk = String::from_utf8_lossy(bytes);
        let buffer = match stream {
            ManagedProcessOutputStream::Stdout => &mut self.stdout_buffer,
            ManagedProcessOutputStream::Stderr => &mut self.stderr_buffer,
        };
        buffer.push_str(&chunk);
        let mut lines = Vec::new();
        while let Some(index) = buffer.find('\n') {
            let mut line = buffer.drain(..=index).collect::<String>();
            line.truncate(line.trim_end_matches(['\r', '\n']).len());
            if let Some(activity) = provider_activity_line(stream, &line, &self.output_format) {
                lines.push(activity);
            }
        }
        lines
    }

    fn finish(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let stdout = std::mem::take(&mut self.stdout_buffer);
        if let Some(activity) = provider_activity_line(
            ManagedProcessOutputStream::Stdout,
            &stdout,
            &self.output_format,
        ) {
            lines.push(activity);
        }
        let stderr = std::mem::take(&mut self.stderr_buffer);
        if let Some(activity) = provider_activity_line(
            ManagedProcessOutputStream::Stderr,
            &stderr,
            &self.output_format,
        ) {
            lines.push(activity);
        }
        lines
    }
}

fn provider_activity_line(
    stream: ManagedProcessOutputStream,
    line: &str,
    output_format: &str,
) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if stream == ManagedProcessOutputStream::Stderr {
        return Some(format!(
            "stderr: {}",
            line.chars().take(1000).collect::<String>()
        ));
    }
    if output_format == "plain" {
        return Some(line.chars().take(1000).collect());
    }
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        return None;
    };
    provider_activity_text_from_json(&event).map(|text| text.chars().take(1000).collect())
}

fn provider_activity_text_from_json(event: &serde_json::Value) -> Option<String> {
    let object = event.as_object()?;
    if let Some(item) = object.get("item").and_then(|value| value.as_object()) {
        let item_type = item.get("type").and_then(|value| value.as_str());
        let text = item
            .get("text")
            .or_else(|| item.get("content"))
            .or_else(|| object.get("text"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if matches!(item_type, Some("agent_message" | "assistant_message")) {
            return text.map(str::to_string);
        }
        if let (Some(item_type), Some(text)) = (item_type, text) {
            return Some(format!("{item_type}: {text}"));
        }
    }
    if object.get("type").and_then(|value| value.as_str()) == Some("assistant.message") {
        return object
            .get("data")
            .and_then(|value| value.get("content"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    if object.get("type").and_then(|value| value.as_str()) == Some("assistant.message_delta") {
        return object
            .get("data")
            .and_then(|value| value.get("deltaContent"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    if object.get("type").and_then(|value| value.as_str()) == Some("assistant") {
        return object
            .get("message")
            .and_then(|value| value.get("content"))
            .and_then(text_from_content)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    for key in ["delta", "text", "message", "result"] {
        if let Some(text) = object
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(text.to_string());
        }
    }
    None
}

fn extract_final_text(stdout: &str, output_format: &str) -> String {
    if output_format == "plain" {
        return stdout.trim().to_string();
    }
    let mut last = String::new();
    let mut deltas = Vec::new();
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(object) = event.as_object() else {
            continue;
        };
        if let Some(item) = object.get("item").and_then(|value| value.as_object()) {
            let item_type = item.get("type").and_then(|value| value.as_str());
            let text = item
                .get("text")
                .or_else(|| item.get("content"))
                .or_else(|| object.get("text"))
                .and_then(|value| value.as_str());
            if matches!(item_type, Some("agent_message" | "assistant_message")) {
                if let Some(text) = text {
                    last = text.to_string();
                }
                continue;
            }
        }
        if object.get("type").and_then(|value| value.as_str()) == Some("assistant.message") {
            if let Some(content) = object
                .get("data")
                .and_then(|value| value.get("content"))
                .and_then(|value| value.as_str())
            {
                last = content.to_string();
            }
            continue;
        }
        if object.get("type").and_then(|value| value.as_str()) == Some("assistant.message_delta") {
            if let Some(delta) = object
                .get("data")
                .and_then(|value| value.get("deltaContent"))
                .and_then(|value| value.as_str())
            {
                deltas.push(delta.to_string());
            }
            continue;
        }
        if object.get("type").and_then(|value| value.as_str()) == Some("assistant")
            && let Some(text) = object
                .get("message")
                .and_then(|value| value.get("content"))
                .and_then(text_from_content)
        {
            last = text;
        }
    }
    if last.is_empty() {
        if deltas.is_empty() {
            stdout.trim().to_string()
        } else {
            deltas.join("")
        }
    } else {
        last
    }
}

fn provider_error_message(stdout: &str, stderr: &str) -> Option<String> {
    for text in [stderr, stdout] {
        for line in text.lines().rev() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let Some(object) = event.as_object() else {
                continue;
            };
            let is_error = object
                .get("is_error")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let has_api_error = object
                .get("api_error_status")
                .map(|value| !value.is_null())
                .unwrap_or(false);
            if !is_error && !has_api_error {
                continue;
            }
            let message = object
                .get("result")
                .or_else(|| object.get("message"))
                .or_else(|| object.get("error"))
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("provider returned an error");
            let status = object
                .get("api_error_status")
                .and_then(|value| value.as_i64())
                .map(|value| value.to_string());
            return Some(match status {
                Some(status) => format!("{message} ({status})"),
                None => message.to_string(),
            });
        }
    }
    None
}

fn extract_provider_session_id(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(session_id) = find_session_id_value(&event) {
            return Some(session_id);
        }
    }
    None
}

fn find_session_id_value(value: &serde_json::Value) -> Option<String> {
    const SESSION_KEYS: &[&str] = &[
        "provider_session_id",
        "session_id",
        "sessionId",
        "conversation_id",
        "conversationId",
    ];
    match value {
        serde_json::Value::Object(object) => {
            for key in SESSION_KEYS {
                if let Some(session_id) = object
                    .get(*key)
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    return Some(session_id.to_string());
                }
            }
            object.values().find_map(find_session_id_value)
        }
        serde_json::Value::Array(values) => values.iter().find_map(find_session_id_value),
        _ => None,
    }
}

fn text_from_content(content: &serde_json::Value) -> Option<String> {
    let parts = content
        .as_array()?
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(|value| value.as_str()) == Some("text") {
                block
                    .get("text")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n").trim().to_string())
    }
}

fn last_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(500).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn host_provider_service_detects_known_provider_binaries() {
        let temp_root = unique_temp_dir("providers");
        let bin_dir = temp_root.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("codex"), "#!/bin/sh\n").unwrap();
        fs::write(bin_dir.join("smoke-ai"), "#!/bin/sh\n").unwrap();

        let service = HostAgentProviderService {
            path_override: Some(bin_dir.display().to_string()),
            ..HostAgentProviderService::default()
        };
        let providers = service.detect().unwrap();
        let codex = providers
            .iter()
            .find(|provider| provider.name == "codex")
            .unwrap();
        assert!(codex.installed);
        assert!(codex.supports_resume);
        assert_eq!(codex.output_format, "codex_json");
        let smoke_ai = providers
            .iter()
            .find(|provider| provider.name == "smoke-ai")
            .unwrap();
        assert!(smoke_ai.installed);
        let claude = providers
            .iter()
            .find(|provider| provider.name == "claude")
            .unwrap();
        assert!(!claude.installed);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn host_provider_service_invokes_smoke_ai_and_extracts_json_final_text() {
        let temp_root = unique_temp_dir("provider-invoke");
        let bin_dir = temp_root.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let smoke = bin_dir.join("smoke-ai");
        fs::write(
            &smoke,
            "#!/bin/sh\nprintf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"smoke ok\"}}'\n",
        )
        .unwrap();
        make_executable(&smoke);

        let service = HostAgentProviderService {
            path_override: Some(bin_dir.display().to_string()),
            runtime_root: Some(temp_root.join("run/8080")),
        };
        let output = service
            .invoke(ProviderInvocation {
                provider: "smoke-ai".to_string(),
                prompt: "hello".to_string(),
                session_id: None,
                cwd: None,
                process_metadata: Default::default(),
            })
            .unwrap();
        assert!(output.contains("agent_message"));
        assert!(temp_root.join("run/8080/processes").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn extract_final_text_handles_codex_and_copilot_jsonl() {
        let codex = r#"{"item":{"type":"agent_message","text":"done"}}"#;
        assert_eq!(extract_final_text(codex, "codex_json"), "done");

        let copilot = concat!(
            "{\"type\":\"assistant.message_delta\",\"data\":{\"deltaContent\":\"hel\"}}\n",
            "{\"type\":\"assistant.message_delta\",\"data\":{\"deltaContent\":\"lo\"}}\n"
        );
        assert_eq!(extract_final_text(copilot, "copilot_json"), "hello");
    }

    #[test]
    fn provider_activity_formatter_extracts_readable_stream_events() {
        let mut formatter = ProviderActivityFormatter::new("codex_json");
        let lines = formatter.push(
            ManagedProcessOutputStream::Stdout,
            b"{\"item\":{\"type\":\"agent_message\",\"text\":\"streamed agent text\"}}\n",
        );
        assert_eq!(lines, vec!["streamed agent text"]);

        let lines = formatter.push(
            ManagedProcessOutputStream::Stdout,
            b"{\"type\":\"assistant.message_delta\",\"data\":{\"deltaContent\":\"delta text\"}}\n",
        );
        assert_eq!(lines, vec!["delta text"]);
    }

    #[test]
    fn provider_error_message_summarizes_codex_api_error() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":401,"result":"Invalid API key - Fix external API key"}"#;
        assert_eq!(
            provider_error_message(stdout, ""),
            Some("Invalid API key - Fix external API key (401)".to_string())
        );
    }

    #[test]
    fn provider_error_message_ignores_success_with_null_api_status() {
        let stdout = r#"{"type":"result","subtype":"success","is_error":false,"api_error_status":null,"result":"Hello"}"#;
        assert_eq!(provider_error_message(stdout, ""), None);
    }

    #[test]
    fn extract_provider_session_id_handles_common_jsonl_shapes() {
        let stdout = concat!(
            "{\"item\":{\"type\":\"agent_message\",\"text\":\"done\"},\"session_id\":\"prov-1\"}\n",
            "{\"data\":{\"conversationId\":\"prov-2\"}}\n"
        );
        assert_eq!(
            extract_provider_session_id(stdout),
            Some("prov-1".to_string())
        );
        assert_eq!(
            extract_provider_session_id("{\"data\":{\"conversationId\":\"prov-2\"}}\n"),
            Some("prov-2".to_string())
        );
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn make_executable(_path: &Path) {}

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}

use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::application::agent_io::prompts::{PromptTemplate, render};
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
};
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::infrastructure::process::supervisor::security::FileSecurityService;
use crate::model::JsonObject;

pub const TARGET_APP_STATE_FILE: &str = "target-app-state.json";
const MANAGE_APP_LOG_PATH: &str = "@refine-state/manage-app.log";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TargetAppOperation {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub started_at: String,
    pub finished_at: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TargetAppSnapshot {
    pub ok: bool,
    pub state: String,
    pub message: String,
    pub last_check_at: String,
    pub last_check_ok: bool,
    pub last_check_message: String,
    pub last_health_at: String,
    pub last_health_ok: bool,
    pub last_health_message: String,
    pub last_error: String,
    pub last_operation_id: String,
    pub last_operation: Option<TargetAppOperation>,
    pub process_id: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TargetAppGeneratedConfig {
    pub start_instructions: String,
    pub stop_instructions: String,
    pub build_instructions: String,
    pub start_command: String,
    pub stop_command: String,
    pub build_command: String,
    pub test_command: String,
    pub status_command: String,
    pub cwd: String,
    pub env: JsonObject,
    pub start_timeout_seconds: u64,
    pub stop_timeout_seconds: u64,
    pub build_timeout_seconds: u64,
    pub test_timeout_seconds: u64,
    pub status_timeout_seconds: u64,
    pub log_path: String,
    pub http_check_url: String,
    pub tcp_check_host: String,
    pub tcp_check_port: String,
    pub process_check_command: String,
    pub notes: String,
}

#[derive(Clone, Debug)]
pub struct FileTargetAppService {
    pub refine_dir: PathBuf,
    pub runtime_root: PathBuf,
    pub target_root: PathBuf,
}

impl FileTargetAppService {}

impl Default for TargetAppSnapshot {
    fn default() -> Self {
        Self {
            ok: true,
            state: "unknown".to_string(),
            message: String::new(),
            last_check_at: String::new(),
            last_check_ok: true,
            last_check_message: String::new(),
            last_health_at: String::new(),
            last_health_ok: true,
            last_health_message: String::new(),
            last_error: String::new(),
            last_operation_id: String::new(),
            last_operation: None,
            process_id: None,
            pid: None,
        }
    }
}

#[derive(Clone, Debug)]
struct TargetCheckResult {
    ok: bool,
    message: String,
}

fn setting(settings: &JsonObject, key: &str) -> String {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string()
}

fn target_app_test_commands(settings: &JsonObject) -> Vec<String> {
    let raw = setting(settings, "target_app_test_commands");
    let mut commands = serde_json::from_str::<Value>(raw.trim())
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            if !item.get("enabled").and_then(Value::as_bool).unwrap_or(true) {
                return None;
            }
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if command.is_empty() {
                None
            } else {
                Some(command)
            }
        })
        .collect::<Vec<_>>();
    if commands.is_empty() {
        let command = setting(settings, "target_app_test_command");
        if !command.trim().is_empty() {
            commands.push(command);
        }
    }
    commands
}

fn apply_package_json_defaults(
    root: &Path,
    config: &mut TargetAppGeneratedConfig,
) -> RefineResult<()> {
    let path = root.join("package.json");
    let bytes = fs::read_to_string(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read package.json {}: {error}",
            path.display()
        ))
    })?;
    let value = serde_json::from_str::<Value>(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse package.json {}: {error}",
            path.display()
        ))
    })?;
    let scripts = value
        .get("scripts")
        .and_then(|scripts| scripts.as_object())
        .cloned()
        .unwrap_or_default();
    let package_manager = package_manager(root);
    if scripts.contains_key("dev") {
        fill_if_empty(
            &mut config.start_command,
            &format!("{package_manager} run dev"),
        );
    } else if scripts.contains_key("start") {
        fill_if_empty(
            &mut config.start_command,
            &format!("{package_manager} start"),
        );
    }
    if scripts.contains_key("build") {
        fill_if_empty(
            &mut config.build_command,
            &format!("{package_manager} run build"),
        );
    }
    if scripts.contains_key("test") {
        fill_if_empty(&mut config.test_command, &format!("{package_manager} test"));
        fill_if_empty(
            &mut config.status_command,
            &format!("{package_manager} test -- --help >/dev/null 2>&1 || true"),
        );
    }
    Ok(())
}

fn apply_makefile_defaults(path: &Path, config: &mut TargetAppGeneratedConfig) -> RefineResult<()> {
    let bytes = fs::read_to_string(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read Makefile {}: {error}",
            path.display()
        ))
    })?;
    let targets = bytes
        .lines()
        .filter_map(|line| line.split_once(':').map(|(target, _)| target.trim()))
        .filter(|target| {
            !target.is_empty()
                && target
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        })
        .collect::<Vec<_>>();
    if targets.contains(&"start") {
        fill_if_empty(&mut config.start_command, "make start");
    }
    if targets.contains(&"stop") {
        fill_if_empty(&mut config.stop_command, "make stop");
    }
    if targets.contains(&"build") {
        fill_if_empty(&mut config.build_command, "make build");
    }
    if targets.contains(&"test") {
        fill_if_empty(&mut config.test_command, "make test");
    }
    if targets.contains(&"status") {
        fill_if_empty(&mut config.status_command, "make status");
    }
    Ok(())
}

fn apply_static_web_server_defaults(
    root: &Path,
    config: &mut TargetAppGeneratedConfig,
    notes: &mut Vec<String>,
) {
    if !config.start_command.trim().is_empty() {
        return;
    }
    let Some(serve_dir) = static_web_serve_dir(root) else {
        return;
    };
    let port = static_web_port(config);
    if config.http_check_url.trim().is_empty() {
        config.http_check_url = format!("http://127.0.0.1:{port}/");
    }
    if config.tcp_check_host.trim().is_empty() {
        config.tcp_check_host = "127.0.0.1".to_string();
    }
    if config.tcp_check_port.trim().is_empty() {
        config.tcp_check_port = port.to_string();
    }
    config.start_command = static_web_start_command(port, serve_dir, &config.http_check_url);
    config.stop_command = static_web_stop_command();
    if config.build_command.trim().is_empty() {
        config.build_command =
            "printf 'No build step configured; static server uses current files.\\n'".to_string();
    }
    config.status_command = format!(
        "curl -fsS {} >/dev/null",
        shell_quote(&config.http_check_url)
    );
    notes.push(format!(
        "Detected static web content and generated a managed local web server on port {port}."
    ));
}

fn config_project_root(target_root: &Path, cwd: &str) -> PathBuf {
    let cwd = cwd.trim();
    if cwd.is_empty() || cwd == "." {
        return target_root.to_path_buf();
    }
    let path = PathBuf::from(cwd);
    if path.is_absolute() {
        path
    } else {
        target_root.join(path)
    }
}

fn static_web_serve_dir(root: &Path) -> Option<&'static str> {
    for (dir, entry) in [
        (".", "index.html"),
        ("public", "public/index.html"),
        ("dist", "dist/index.html"),
        ("build", "build/index.html"),
    ] {
        if root.join(entry).is_file() {
            return Some(dir);
        }
    }
    let has_root_html = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("html"))
                .unwrap_or(false)
        });
    has_root_html.then_some(".")
}

fn static_web_port(config: &TargetAppGeneratedConfig) -> u16 {
    port_from_url(&config.http_check_url)
        .or_else(|| config.tcp_check_port.trim().parse::<u16>().ok())
        .unwrap_or(3000)
}

fn static_web_start_command(port: u16, serve_dir: &str, url: &str) -> String {
    [
        format!("PORT={port};"),
        format!("URL={};", shell_quote(url)),
        format!("SERVE_DIR={};", shell_quote(serve_dir)),
        "RUNTIME_DIR=$(git rev-parse --git-path refine-target-app-runtime);".to_string(),
        "PID_FILE=$RUNTIME_DIR/target-app.pid;".to_string(),
        "SERVER_LOG=$RUNTIME_DIR/target-app-server.log;".to_string(),
        "mkdir -p \"$RUNTIME_DIR\";".to_string(),
        "if curl -fsS \"$URL\" >/dev/null 2>&1; then exit 0; fi;".to_string(),
        "if [ -s \"$PID_FILE\" ] && kill -0 \"$(cat \"$PID_FILE\")\" 2>/dev/null; then :; else"
            .to_string(),
        "rm -f \"$PID_FILE\";".to_string(),
        "if command -v python3 >/dev/null 2>&1; then".to_string(),
        "sh -c \"cd \\\"$SERVE_DIR\\\" && exec python3 -m http.server \\\"$PORT\\\" --bind 127.0.0.1\" > \"$SERVER_LOG\" 2>&1 & echo $! > \"$PID_FILE\";"
            .to_string(),
        "elif command -v npx >/dev/null 2>&1; then".to_string(),
        "sh -c \"exec npx --yes serve \\\"$SERVE_DIR\\\" -l tcp://127.0.0.1:\\\"$PORT\\\" --no-clipboard --no-port-switching\" > \"$SERVER_LOG\" 2>&1 & echo $! > \"$PID_FILE\";"
            .to_string(),
        "else echo \"No static web server runner found (need python3 or npx)\" >&2; exit 1; fi; fi;"
            .to_string(),
        "i=0;".to_string(),
        "while [ \"$i\" -lt 90 ]; do".to_string(),
        "if curl -fsS \"$URL\" >/dev/null 2>&1; then exit 0; fi;".to_string(),
        "i=$((i + 1)); sleep 1; done;".to_string(),
        "echo \"Target app did not become reachable at $URL\" >&2; exit 1".to_string(),
    ]
    .join(" ")
}

fn static_web_stop_command() -> String {
    [
        "RUNTIME_DIR=$(git rev-parse --git-path refine-target-app-runtime);",
        "PID_FILE=$RUNTIME_DIR/target-app.pid;",
        "if [ -s \"$PID_FILE\" ]; then",
        "PID=$(cat \"$PID_FILE\");",
        "if kill -0 \"$PID\" 2>/dev/null; then",
        "kill \"$PID\" 2>/dev/null || true;",
        "i=0;",
        "while [ \"$i\" -lt 30 ]; do",
        "kill -0 \"$PID\" 2>/dev/null || break;",
        "i=$((i + 1)); sleep 1;",
        "done;",
        "kill -0 \"$PID\" 2>/dev/null && kill -9 \"$PID\" 2>/dev/null || true;",
        "fi;",
        "rm -f \"$PID_FILE\";",
        "fi;",
        "exit 0",
    ]
    .join(" ")
}

fn package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else {
        "npm"
    }
}

fn fill_if_empty(value: &mut String, fallback: &str) {
    if value.trim().is_empty() {
        *value = fallback.to_string();
    }
}

trait EmptyStringFallback {
    fn if_empty(self, fallback: &str) -> String;
}

impl EmptyStringFallback for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.trim().is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}

fn convert_lifecycle_commands_to_instructions(config: &mut TargetAppGeneratedConfig) {
    if config.start_instructions.trim().is_empty() && !config.start_command.trim().is_empty() {
        config.start_instructions = command_backed_instruction("start", &config.start_command);
    }
    if config.stop_instructions.trim().is_empty() && !config.stop_command.trim().is_empty() {
        config.stop_instructions = command_backed_instruction("stop", &config.stop_command);
    }
    if config.build_instructions.trim().is_empty() && !config.build_command.trim().is_empty() {
        config.build_instructions = command_backed_instruction("build", &config.build_command);
    }
    config.start_command.clear();
    config.stop_command.clear();
    config.build_command.clear();
}

fn command_backed_instruction(kind: &str, command: &str) -> String {
    let template = match kind {
        "start" => PromptTemplate::TargetAppCommandStart,
        "stop" => PromptTemplate::TargetAppCommandStop,
        "build" => PromptTemplate::TargetAppCommandBuild,
        _ => return command.trim().to_string(),
    };
    render(template, &[("command", command.trim())])
}

fn target_app_lifecycle_prompt(
    kind: &str,
    instructions: &str,
    settings: &JsonObject,
    target_root: &Path,
    cwd: &Path,
) -> String {
    let env_json = setting(settings, "target_app_env_json");
    let health_url = first_nonempty(&[
        setting(settings, "target_app_http_check_url"),
        setting(settings, "target_app_health_url"),
        setting(settings, "target_app_url"),
    ]);
    let tcp_host = setting(settings, "target_app_tcp_check_host");
    let tcp_port = setting(settings, "target_app_tcp_check_port");
    let status_command = setting(settings, "target_app_status_command");
    let process_command = setting(settings, "target_app_process_check_command");
    let target_root = target_root.display().to_string();
    let cwd = cwd.display().to_string();
    let environment = if env_json.trim().is_empty() {
        "{}"
    } else {
        env_json.trim()
    };
    render(
        PromptTemplate::TargetAppLifecycle,
        &[
            ("kind", kind),
            ("target_root", &target_root),
            ("cwd", &cwd),
            ("environment", environment),
            ("health_url", &health_url),
            ("tcp_host", &tcp_host),
            ("tcp_port", &tcp_port),
            ("status_command", &status_command),
            ("process_command", &process_command),
            ("instructions", instructions.trim()),
        ],
    )
}

fn append_note(notes: &mut String, note: &str) {
    if notes.trim().is_empty() {
        *notes = note.to_string();
    } else {
        notes.push(' ');
        notes.push_str(note);
    }
}

fn clear_generated_wrapper_entrypoints(config: &mut TargetAppGeneratedConfig) -> bool {
    let mut cleared = false;
    if is_manage_app_wrapper_entrypoint(&config.start_command, "start") {
        config.start_command.clear();
        cleared = true;
    }
    if is_manage_app_wrapper_entrypoint(&config.stop_command, "stop") {
        config.stop_command.clear();
        cleared = true;
    }
    if is_manage_app_wrapper_entrypoint(&config.build_command, "build") {
        config.build_command.clear();
        cleared = true;
    }
    if is_manage_app_wrapper_entrypoint(&config.test_command, "test") {
        config.test_command.clear();
        cleared = true;
    }
    if is_manage_app_wrapper_entrypoint(&config.status_command, "status") {
        config.status_command.clear();
        cleared = true;
    }
    cleared
}

fn is_manage_app_wrapper_entrypoint(command: &str, action: &str) -> bool {
    let command = command.trim();
    command == manage_app_wrapper_entrypoint(action)
        || command == format!("./.refine/manage-app.sh {action}")
        || command == format!(".refine/manage-app.sh {action}")
        || command == format!("sh ./.refine/manage-app.sh {action}")
        || command == format!("sh .refine/manage-app.sh {action}")
}

fn manage_app_wrapper_entrypoint(action: &str) -> String {
    format!("sh \"$(git rev-parse --git-common-dir)/refine-live-state/manage-app.sh\" {action}")
}

fn first_nonempty(values: &[String]) -> String {
    values
        .iter()
        .find(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_default()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn manage_app_wrapper_script(config: &TargetAppGeneratedConfig) -> String {
    let inner_cwd = if config.cwd.trim().is_empty() {
        "."
    } else {
        config.cwd.trim()
    };
    let mut lines = vec![
        "#!/usr/bin/env sh".to_string(),
        "set -u".to_string(),
        String::new(),
        "# Generated by Refine. Edit this file if your target app needs custom lifecycle handling."
            .to_string(),
        format!("APP_CWD={}", shell_quote(inner_cwd)),
        format!("LOG_PATH={}", shell_quote(config.log_path.trim())),
        format!("START_COMMAND={}", shell_quote(config.start_command.trim())),
        format!("STOP_COMMAND={}", shell_quote(config.stop_command.trim())),
        format!("BUILD_COMMAND={}", shell_quote(config.build_command.trim())),
        format!("TEST_COMMAND={}", shell_quote(config.test_command.trim())),
        format!(
            "STATUS_COMMAND={}",
            shell_quote(config.status_command.trim())
        ),
    ];

    if !config.notes.trim().is_empty() {
        lines.push(format!("# Analysis notes: {}", config.notes.trim()));
    }
    for (key, value) in &config.env {
        if shell_env_key(key) {
            lines.push(format!(
                "export {}={}",
                key,
                shell_quote(&shell_env_value(value))
            ));
        }
    }

    lines.extend([
        String::new(),
        "ROOT=$(git rev-parse --show-toplevel)".to_string(),
        "WRAPPER_DIR=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)".to_string(),
        "case \"$LOG_PATH\" in".to_string(),
        "  @refine-state/*) LOG_FILE=$WRAPPER_DIR/${LOG_PATH#@refine-state/} ;;".to_string(),
        "  /*) LOG_FILE=$LOG_PATH ;;".to_string(),
        "  *) LOG_FILE=$ROOT/$LOG_PATH ;;".to_string(),
        "esac".to_string(),
        "mkdir -p -- \"$(dirname -- \"$LOG_FILE\")\"".to_string(),
        String::new(),
        "timestamp() { date '+%Y-%m-%dT%H:%M:%S%z'; }".to_string(),
        "log() { printf '%s [%s] %s\\n' \"$(timestamp)\" \"$ACTION\" \"$*\" >> \"$LOG_FILE\"; }"
            .to_string(),
        String::new(),
        "run_cmd() {".to_string(),
        "  cmd=$1".to_string(),
        "  if [ -z \"$cmd\" ]; then".to_string(),
        "    if [ \"$ACTION\" = stop ]; then".to_string(),
        "      log 'no command configured; treating stop as complete'".to_string(),
        "      exit 0".to_string(),
        "    fi".to_string(),
        "    log 'no command configured'".to_string(),
        "    exit 1".to_string(),
        "  fi".to_string(),
        "  case \"$APP_CWD\" in".to_string(),
        "    /*) RUN_DIR=$APP_CWD ;;".to_string(),
        "    *) RUN_DIR=$ROOT/$APP_CWD ;;".to_string(),
        "  esac".to_string(),
        "  log \"cwd=$RUN_DIR\"".to_string(),
        "  log \"command=$cmd\"".to_string(),
        "  if [ ! -d \"$RUN_DIR\" ]; then".to_string(),
        "    log 'cwd does not exist'".to_string(),
        "    exit 1".to_string(),
        "  fi".to_string(),
        "  (".to_string(),
        "    cd -- \"$RUN_DIR\" || exit 1".to_string(),
        "    sh -lc \"$cmd\"".to_string(),
        "  ) >> \"$LOG_FILE\" 2>&1".to_string(),
        "  code=$?".to_string(),
        "  log \"exit=$code\"".to_string(),
        "  exit \"$code\"".to_string(),
        "}".to_string(),
        String::new(),
        "ACTION=${1:-status}".to_string(),
        "case \"$ACTION\" in".to_string(),
        "  start) run_cmd \"$START_COMMAND\" ;;".to_string(),
        "  stop) run_cmd \"$STOP_COMMAND\" ;;".to_string(),
        "  build) run_cmd \"$BUILD_COMMAND\" ;;".to_string(),
        "  test) run_cmd \"$TEST_COMMAND\" ;;".to_string(),
        "  status) run_cmd \"$STATUS_COMMAND\" ;;".to_string(),
        "  *)".to_string(),
        "    printf 'usage: %s start|stop|build|test|status\\n' \"$0\" >&2".to_string(),
        "    exit 64".to_string(),
        "    ;;".to_string(),
        "esac".to_string(),
    ]);
    lines.push(String::new());
    lines.join("\n")
}

fn shell_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn shell_env_value(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            if value.is_number() || value.is_boolean() {
                Some(value.to_string())
            } else {
                None
            }
        })
        .unwrap_or_default()
}

fn make_executable(path: &Path) -> RefineResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to stat target-app wrapper {}: {error}",
                    path.display()
                ))
            })?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).map_err(|error| {
            RefineError::Io(format!(
                "failed to mark target-app wrapper executable {}: {error}",
                path.display()
            ))
        })?;
    }
    Ok(())
}

fn port_from_url(url: &str) -> Option<u16> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host_port = after_scheme.split('/').next().unwrap_or("");
    let port = host_port.rsplit_once(':')?.1;
    port.parse::<u16>().ok()
}

fn command_env(settings: &JsonObject) -> RefineResult<Vec<(String, String)>> {
    let raw = setting(settings, "target_app_env_json");
    let value = serde_json::from_str::<Value>(raw.trim()).map_err(|_| {
        RefineError::InvalidInput("target_app_env_json must be a JSON object".to_string())
    })?;
    let Some(object) = value.as_object() else {
        return Err(RefineError::InvalidInput(
            "target_app_env_json must be a JSON object".to_string(),
        ));
    };
    Ok(object
        .iter()
        .filter_map(|(key, value)| {
            value
                .as_str()
                .map(|text| (key.clone(), text.to_string()))
                .or_else(|| {
                    if value.is_number() || value.is_boolean() {
                        Some((key.clone(), value.to_string()))
                    } else {
                        None
                    }
                })
        })
        .collect())
}

fn shell_program_args(command: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), command.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        (
            "sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
        )
    }
}

fn operation_message(operation: &TargetAppOperation) -> String {
    if operation.exit_code == Some(0) {
        if operation.stdout.trim().is_empty() {
            format!("{} completed", operation.kind)
        } else {
            operation.stdout.clone()
        }
    } else if !operation.stderr.trim().is_empty() {
        operation.stderr.clone()
    } else if !operation.stdout.trim().is_empty() {
        operation.stdout.clone()
    } else {
        format!("{} failed", operation.kind)
    }
}

fn tcp_reachable(host: &str, port: u16) -> bool {
    let Ok(mut addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok())
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn new_operation_id(prefix: &str) -> String {
    format!("{}-{}", prefix, Utc::now().timestamp_millis())
}

mod commands;
mod generation;
mod lifecycle;
mod state;
#[cfg(test)]
mod tests;

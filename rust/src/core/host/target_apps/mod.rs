use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::host::process_supervision::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
};
use crate::core::supervisor::config::{ConfigService, FileSettingsService};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::security::FileSecurityService;
use crate::model::JsonObject;

pub const TARGET_APP_STATE_FILE: &str = "target-app-state.json";

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
    pub start_command: String,
    pub stop_command: String,
    pub rebuild_command: String,
    pub status_command: String,
    pub cwd: String,
    pub env: JsonObject,
    pub start_timeout_seconds: u64,
    pub stop_timeout_seconds: u64,
    pub rebuild_timeout_seconds: u64,
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
    pub durable_root: PathBuf,
    pub runtime_root: PathBuf,
    pub source_root: PathBuf,
}

impl FileTargetAppService {
    pub fn new(
        durable_root: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        source_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            durable_root: durable_root.into(),
            runtime_root: runtime_root.into(),
            source_root: source_root.into(),
        }
    }

    pub fn status(&self) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let mut snapshot = self.load_snapshot()?;
        let check = self.run_configured_checks(&settings)?;
        snapshot.ok = check.ok;
        snapshot.state = if check.ok {
            "running".to_string()
        } else if snapshot.state == "running" {
            "degraded".to_string()
        } else {
            "stopped".to_string()
        };
        snapshot.message = check.message.clone();
        snapshot.last_check_at = now_timestamp();
        snapshot.last_check_ok = check.ok;
        snapshot.last_check_message = check.message.clone();
        snapshot.last_health_at = snapshot.last_check_at.clone();
        snapshot.last_health_ok = check.ok;
        snapshot.last_health_message = check.message;
        snapshot.last_error = if check.ok {
            String::new()
        } else {
            snapshot.last_check_message.clone()
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn health(&self) -> RefineResult<TargetAppSnapshot> {
        self.status()
    }

    pub fn snapshot(&self) -> RefineResult<TargetAppSnapshot> {
        self.load_snapshot()
    }

    pub fn start(&self) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let command = setting(&settings, "target_app_start_command");
        if command.trim().is_empty() {
            let mut snapshot = self.load_snapshot()?;
            snapshot.ok = true;
            snapshot.message = "No target-app start command is configured.".to_string();
            snapshot.state = "unknown".to_string();
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        let (shell, args) = shell_program_args(&command);
        let security =
            FileSecurityService::from_project_settings(&self.runtime_root, &self.durable_root)?;
        let process = FileProcessSupervisor::with_allowed_commands(
            &self.runtime_root,
            security.allowed_commands.iter().cloned(),
        )
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::TargetApp,
            command: shell,
            args,
            cwd: Some(self.command_cwd(&settings).display().to_string()),
            env: command_env(&settings)?,
            stdin: None,
            limits: None,
            authorization_command: Some(command.clone()),
            sensitive: false,
        })?;
        let operation = TargetAppOperation {
            id: new_operation_id("target-start"),
            kind: "start".to_string(),
            state: "running".to_string(),
            started_at: now_timestamp(),
            finished_at: String::new(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        };
        let snapshot = TargetAppSnapshot {
            ok: true,
            state: "running".to_string(),
            message: "Target application started.".to_string(),
            last_check_at: String::new(),
            last_check_ok: true,
            last_check_message: String::new(),
            last_health_at: String::new(),
            last_health_ok: true,
            last_health_message: String::new(),
            last_error: String::new(),
            last_operation_id: operation.id.clone(),
            last_operation: Some(operation),
            process_id: Some(process.id),
            pid: process.pid,
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn stop(&self) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let command = setting(&settings, "target_app_stop_command");
        let operation = if command.trim().is_empty() {
            TargetAppOperation {
                id: new_operation_id("target-stop"),
                kind: "stop".to_string(),
                state: "complete".to_string(),
                started_at: now_timestamp(),
                finished_at: now_timestamp(),
                exit_code: Some(0),
                stdout: String::new(),
                stderr: "No target-app stop command is configured.".to_string(),
            }
        } else {
            self.run_command("stop", &command, &settings)?
        };
        self.mark_target_processes_stopped()?;
        let ok = operation.exit_code == Some(0);
        let snapshot = TargetAppSnapshot {
            ok,
            state: if ok { "stopped" } else { "failed" }.to_string(),
            message: operation_message(&operation),
            last_check_at: String::new(),
            last_check_ok: ok,
            last_check_message: operation_message(&operation),
            last_health_at: String::new(),
            last_health_ok: ok,
            last_health_message: operation_message(&operation),
            last_error: if ok {
                String::new()
            } else {
                operation_message(&operation)
            },
            last_operation_id: operation.id.clone(),
            last_operation: Some(operation),
            process_id: None,
            pid: None,
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn rebuild(&self) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let command = setting(&settings, "target_app_rebuild_command");
        if command.trim().is_empty() {
            let mut snapshot = self.load_snapshot()?;
            snapshot.ok = false;
            snapshot.state = "failed".to_string();
            snapshot.message = "No target-app rebuild command is configured.".to_string();
            snapshot.last_error = snapshot.message.clone();
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        let operation = self.run_command("rebuild", &command, &settings)?;
        let ok = operation.exit_code == Some(0);
        let snapshot = TargetAppSnapshot {
            ok,
            state: if ok { "stopped" } else { "failed" }.to_string(),
            message: operation_message(&operation),
            last_check_at: String::new(),
            last_check_ok: ok,
            last_check_message: operation_message(&operation),
            last_health_at: String::new(),
            last_health_ok: ok,
            last_health_message: operation_message(&operation),
            last_error: if ok {
                String::new()
            } else {
                operation_message(&operation)
            },
            last_operation_id: operation.id.clone(),
            last_operation: Some(operation),
            process_id: None,
            pid: None,
        };
        self.save_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn generate_config(&self) -> RefineResult<TargetAppGeneratedConfig> {
        let settings = self.settings()?;
        let mut config = TargetAppGeneratedConfig {
            start_command: setting(&settings, "target_app_start_command"),
            stop_command: setting(&settings, "target_app_stop_command"),
            rebuild_command: setting(&settings, "target_app_rebuild_command"),
            status_command: setting(&settings, "target_app_status_command"),
            cwd: setting(&settings, "target_app_cwd"),
            env: serde_json::Map::new(),
            start_timeout_seconds: setting(&settings, "target_app_start_timeout_seconds")
                .parse()
                .unwrap_or(120),
            stop_timeout_seconds: setting(&settings, "target_app_stop_timeout_seconds")
                .parse()
                .unwrap_or(60),
            rebuild_timeout_seconds: setting(&settings, "target_app_rebuild_timeout_seconds")
                .parse()
                .unwrap_or(300),
            status_timeout_seconds: setting(&settings, "target_app_status_timeout_seconds")
                .parse()
                .unwrap_or(10),
            log_path: setting(&settings, "target_app_log_path"),
            http_check_url: first_nonempty(&[
                setting(&settings, "target_app_http_check_url"),
                setting(&settings, "target_app_health_url"),
                setting(&settings, "target_app_url"),
            ]),
            tcp_check_host: setting(&settings, "target_app_tcp_check_host"),
            tcp_check_port: setting(&settings, "target_app_tcp_check_port"),
            process_check_command: setting(&settings, "target_app_process_check_command"),
            notes: String::new(),
        };
        config.env = serde_json::from_str::<Value>(&setting(&settings, "target_app_env_json"))
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();

        let project_root = self.command_cwd(&settings);
        let mut notes = Vec::new();
        if project_root.join("package.json").exists() {
            apply_package_json_defaults(&project_root, &mut config)?;
            notes.push("Detected package.json and generated npm-compatible commands.".to_string());
        } else if project_root.join("Cargo.toml").exists() {
            fill_if_empty(&mut config.start_command, "cargo run");
            fill_if_empty(&mut config.rebuild_command, "cargo build");
            fill_if_empty(&mut config.status_command, "cargo check --quiet");
            notes.push("Detected Cargo.toml and generated cargo commands.".to_string());
        } else if project_root.join("Makefile").exists() || project_root.join("makefile").exists() {
            let makefile = if project_root.join("Makefile").exists() {
                project_root.join("Makefile")
            } else {
                project_root.join("makefile")
            };
            apply_makefile_defaults(&makefile, &mut config)?;
            notes.push("Detected Makefile targets and generated make commands.".to_string());
        } else {
            notes.push("No package.json, Cargo.toml, or Makefile was detected; preserved existing target-app settings.".to_string());
        }

        if config.status_command.trim().is_empty() && !config.http_check_url.trim().is_empty() {
            config.status_command = format!(
                "curl -fsS {} >/dev/null",
                shell_quote(&config.http_check_url)
            );
        }
        if config.tcp_check_port.trim().is_empty() {
            if let Some(port) = port_from_url(&config.http_check_url) {
                config.tcp_check_host = "127.0.0.1".to_string();
                config.tcp_check_port = port.to_string();
            }
        }
        if config.stop_command.trim().is_empty() && !config.tcp_check_port.trim().is_empty() {
            config.stop_command = format!(
                "sh -c 'lsof -ti tcp:{} | xargs -r kill'",
                config.tcp_check_port
            );
            notes.push("Generated stop command targets the configured TCP port.".to_string());
        }
        config.notes = notes.join(" ");
        Ok(config)
    }

    fn run_command(
        &self,
        kind: &str,
        command: &str,
        settings: &JsonObject,
    ) -> RefineResult<TargetAppOperation> {
        let started_at = now_timestamp();
        FileSecurityService::from_project_settings(&self.runtime_root, &self.durable_root)?
            .authorize_host_command("target_app", command)?;
        let (shell, args) = shell_program_args(command);
        let output = FileProcessSupervisor::new(&self.runtime_root).run_to_completion(
            ManagedProcessSpec {
                owner: ProcessOwner::TargetApp,
                command: shell,
                args,
                cwd: Some(self.command_cwd(settings).display().to_string()),
                env: command_env(settings)?,
                stdin: None,
                limits: None,
                authorization_command: Some(command.to_string()),
                sensitive: false,
            },
        )?;
        Ok(TargetAppOperation {
            id: new_operation_id(&format!("target-{kind}")),
            kind: kind.to_string(),
            state: if output.success() {
                "complete".to_string()
            } else {
                "failed".to_string()
            },
            started_at,
            finished_at: now_timestamp(),
            exit_code: output.process.exit_code,
            stdout: output.stdout.trim().to_string(),
            stderr: output.stderr.trim().to_string(),
        })
    }

    fn run_configured_checks(&self, settings: &JsonObject) -> RefineResult<TargetCheckResult> {
        let mut checks = Vec::new();
        let status_command = setting(settings, "target_app_status_command");
        if !status_command.trim().is_empty() {
            let operation = self.run_command("status", &status_command, settings)?;
            checks.push(TargetCheckResult {
                ok: operation.exit_code == Some(0),
                message: operation_message(&operation),
            });
        }
        let process_command = setting(settings, "target_app_process_check_command");
        if !process_command.trim().is_empty() {
            let operation = self.run_command("process-check", &process_command, settings)?;
            checks.push(TargetCheckResult {
                ok: operation.exit_code == Some(0),
                message: operation_message(&operation),
            });
        }
        let tcp_host = setting(settings, "target_app_tcp_check_host");
        let tcp_port = setting(settings, "target_app_tcp_check_port");
        if !tcp_host.trim().is_empty() && !tcp_port.trim().is_empty() {
            let port = tcp_port.parse::<u16>().map_err(|_| {
                RefineError::InvalidInput("target_app_tcp_check_port must be a port".to_string())
            })?;
            let ok = tcp_reachable(&tcp_host, port);
            checks.push(TargetCheckResult {
                ok,
                message: if ok {
                    format!("TCP check {tcp_host}:{port} succeeded")
                } else {
                    format!("TCP check {tcp_host}:{port} failed")
                },
            });
        }
        if checks.is_empty() {
            return Ok(TargetCheckResult {
                ok: true,
                message: "No target-app status checks are configured.".to_string(),
            });
        }
        let failed: Vec<_> = checks.iter().filter(|check| !check.ok).collect();
        if failed.is_empty() {
            Ok(TargetCheckResult {
                ok: true,
                message: checks
                    .into_iter()
                    .map(|check| check.message)
                    .collect::<Vec<_>>()
                    .join("; "),
            })
        } else {
            Ok(TargetCheckResult {
                ok: false,
                message: failed
                    .into_iter()
                    .map(|check| check.message.clone())
                    .collect::<Vec<_>>()
                    .join("; "),
            })
        }
    }

    fn settings(&self) -> RefineResult<JsonObject> {
        FileSettingsService::new(&self.durable_root).load()
    }

    fn state_path(&self) -> PathBuf {
        self.runtime_root.join(TARGET_APP_STATE_FILE)
    }

    fn load_snapshot(&self) -> RefineResult<TargetAppSnapshot> {
        let path = self.state_path();
        if !path.exists() {
            return Ok(TargetAppSnapshot::default());
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read target-app state {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse target-app state {}: {error}",
                path.display()
            ))
        })
    }

    fn save_snapshot(&self, snapshot: &TargetAppSnapshot) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(snapshot).map_err(|error| {
            RefineError::Serialization(format!("failed to encode target-app state: {error}"))
        })?;
        fs::write(self.state_path(), encoded)
            .map_err(|error| RefineError::Io(format!("failed to write target-app state: {error}")))
    }

    fn command_cwd(&self, settings: &JsonObject) -> PathBuf {
        let cwd = setting(settings, "target_app_cwd");
        if cwd.trim().is_empty() {
            return self.source_root.clone();
        }
        let path = PathBuf::from(cwd);
        if path.is_absolute() {
            path
        } else {
            self.source_root.join(path)
        }
    }

    fn mark_target_processes_stopped(&self) -> RefineResult<()> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        for process in supervisor
            .list()?
            .into_iter()
            .filter(|process| process.owner == ProcessOwner::TargetApp)
        {
            let _ = supervisor.signal(&process.id, "stop");
        }
        Ok(())
    }
}

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
            &mut config.rebuild_command,
            &format!("{package_manager} run build"),
        );
    }
    if scripts.contains_key("test") {
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
    if targets.contains(&"rebuild") {
        fill_if_empty(&mut config.rebuild_command, "make rebuild");
    } else if targets.contains(&"build") {
        fill_if_empty(&mut config.rebuild_command, "make build");
    }
    if targets.contains(&"status") {
        fill_if_empty(&mut config.status_command, "make status");
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn target_app_service_runs_status_and_rebuild_commands() {
        let temp_root = unique_temp_dir("target-app-service");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let source_root = temp_root.join("app");
        fs::create_dir_all(&durable_root).unwrap();
        fs::create_dir_all(&source_root).unwrap();
        FileSettingsService::new(&durable_root)
            .update(&json!({
                "target_app_status_command": "test -f status-ok",
                "target_app_rebuild_command": "touch rebuilt && echo rebuilt",
                "target_app_cwd": source_root.to_str().unwrap()
            }))
            .unwrap();
        fs::write(source_root.join("status-ok"), "").unwrap();
        let service = FileTargetAppService::new(&durable_root, &runtime_root, &source_root);

        let status = service.status().unwrap();
        assert_eq!(status.state, "running");
        assert!(status.last_check_ok);

        let rebuilt = service.rebuild().unwrap();
        assert!(rebuilt.ok);
        assert!(source_root.join("rebuilt").exists());
        assert!(runtime_root.join(TARGET_APP_STATE_FILE).exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_service_spawns_start_command_and_registers_process() {
        let temp_root = unique_temp_dir("target-app-start");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let source_root = temp_root.join("app");
        fs::create_dir_all(&durable_root).unwrap();
        fs::create_dir_all(&source_root).unwrap();
        FileSettingsService::new(&durable_root)
            .update(&json!({
                "target_app_start_command": "printf target-started; sleep 2",
                "target_app_cwd": source_root.to_str().unwrap()
            }))
            .unwrap();
        let service = FileTargetAppService::new(&durable_root, &runtime_root, &source_root);

        let started = service.start().unwrap();
        assert_eq!(started.state, "running");
        assert!(started.pid.is_some());
        assert_eq!(
            FileProcessSupervisor::new(&runtime_root)
                .list()
                .unwrap()
                .len(),
            1
        );
        std::thread::sleep(Duration::from_millis(50));
        let supervisor = FileProcessSupervisor::new(&runtime_root);
        let process_id = started.process_id.as_deref().unwrap();
        assert!(
            supervisor
                .stream(process_id)
                .unwrap()
                .contains("target-started")
        );
        service.stop().unwrap();

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_service_generates_package_json_config() {
        let temp_root = unique_temp_dir("target-app-generate");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let source_root = temp_root.join("app");
        fs::create_dir_all(&durable_root).unwrap();
        fs::create_dir_all(&source_root).unwrap();
        fs::write(
            source_root.join("package.json"),
            r#"{"scripts":{"dev":"vite","build":"vite build"}}"#,
        )
        .unwrap();
        fs::write(source_root.join("pnpm-lock.yaml"), "").unwrap();
        FileSettingsService::new(&durable_root)
            .update(&json!({
                "target_app_url": "http://127.0.0.1:5173",
                "target_app_cwd": source_root.to_str().unwrap()
            }))
            .unwrap();

        let generated = FileTargetAppService::new(&durable_root, &runtime_root, &source_root)
            .generate_config()
            .unwrap();
        assert_eq!(generated.start_command, "pnpm run dev");
        assert_eq!(generated.rebuild_command, "pnpm run build");
        assert_eq!(generated.tcp_check_port, "5173");
        assert!(generated.notes.contains("package.json"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}

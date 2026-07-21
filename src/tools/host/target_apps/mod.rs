use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::JsonObject;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::security::FileSecurityService;
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};

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

impl FileTargetAppService {
    pub fn new(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: runtime_root.into(),
            target_root: target_root.into(),
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
        let instructions = setting(&settings, "target_app_start_instructions");
        let command = setting(&settings, "target_app_start_command");
        if !instructions.trim().is_empty() {
            let operation =
                self.run_agent_lifecycle("start", &instructions, &settings, Default::default());
            let ok = operation.exit_code == Some(0);
            let snapshot = TargetAppSnapshot {
                ok,
                state: if ok { "running" } else { "failed" }.to_string(),
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
            return Ok(snapshot);
        }
        if command.trim().is_empty() {
            let mut snapshot = self.load_snapshot()?;
            snapshot.ok = true;
            snapshot.message = "No target-app start instructions are configured.".to_string();
            snapshot.state = "unknown".to_string();
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        let (shell, args) = shell_program_args(&command);
        let security =
            FileSecurityService::from_project_settings(&self.runtime_root, &self.refine_dir)?;
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
            metadata: Default::default(),
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
        let instructions = setting(&settings, "target_app_stop_instructions");
        let command = setting(&settings, "target_app_stop_command");
        let operation = if !instructions.trim().is_empty() {
            self.run_agent_lifecycle("stop", &instructions, &settings, Default::default())
        } else if command.trim().is_empty() {
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
            self.run_command("stop", &command, &settings, Default::default())?
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

    pub fn build(&self) -> RefineResult<TargetAppSnapshot> {
        self.build_with_metadata(Default::default())
    }

    pub fn build_with_metadata(
        &self,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let instructions = first_nonempty(&[
            setting(&settings, "target_app_build_instructions"),
            setting(&settings, "target_app_rebuild_instructions"),
        ]);
        let command = setting(&settings, "target_app_build_command");
        if !instructions.trim().is_empty() {
            let operation =
                self.run_agent_lifecycle("build", &instructions, &settings, process_metadata);
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
            return Ok(snapshot);
        }
        if command.trim().is_empty() {
            let mut snapshot = self.load_snapshot()?;
            snapshot.ok = true;
            snapshot.state = "stopped".to_string();
            snapshot.message = "No target-app build instructions are configured.".to_string();
            snapshot.last_check_ok = true;
            snapshot.last_check_message = snapshot.message.clone();
            snapshot.last_health_ok = true;
            snapshot.last_health_message = snapshot.message.clone();
            snapshot.last_error = String::new();
            snapshot.last_operation_id = String::new();
            snapshot.last_operation = None;
            snapshot.process_id = None;
            snapshot.pid = None;
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }
        let operation = self.run_command("build", &command, &settings, process_metadata)?;
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

    pub fn test(&self) -> RefineResult<TargetAppSnapshot> {
        self.test_with_metadata(Default::default())
    }

    pub fn test_with_metadata(
        &self,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<TargetAppSnapshot> {
        let settings = self.settings()?;
        let commands = target_app_test_commands(&settings);
        if commands.is_empty() {
            let mut snapshot = self.load_snapshot()?;
            snapshot.ok = false;
            snapshot.state = "failed".to_string();
            snapshot.message = "No enabled target-app test command is configured.".to_string();
            snapshot.last_error = snapshot.message.clone();
            self.save_snapshot(&snapshot)?;
            return Ok(snapshot);
        }

        let mut last_operation = None;
        let mut messages = Vec::new();
        let mut ok = true;
        for command in commands {
            let operation =
                self.run_quality_command("test", &command, &settings, process_metadata.clone())?;
            let operation_ok = operation.exit_code == Some(0);
            let message = operation_message(&operation);
            messages.push(format!("{command}: {message}"));
            if !operation_ok {
                ok = false;
                last_operation = Some(operation);
                break;
            }
            last_operation = Some(operation);
        }
        let operation = last_operation.expect("non-empty commands must produce an operation");
        let message = messages.join("\n");
        let snapshot = TargetAppSnapshot {
            ok,
            state: if ok { "stopped" } else { "failed" }.to_string(),
            message: message.clone(),
            last_check_at: String::new(),
            last_check_ok: ok,
            last_check_message: message.clone(),
            last_health_at: String::new(),
            last_health_ok: ok,
            last_health_message: message.clone(),
            last_error: if ok { String::new() } else { message.clone() },
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
            start_instructions: setting(&settings, "target_app_start_instructions"),
            stop_instructions: setting(&settings, "target_app_stop_instructions"),
            build_instructions: first_nonempty(&[
                setting(&settings, "target_app_build_instructions"),
                setting(&settings, "target_app_rebuild_instructions"),
            ]),
            start_command: setting(&settings, "target_app_start_command"),
            stop_command: setting(&settings, "target_app_stop_command"),
            build_command: setting(&settings, "target_app_build_command"),
            test_command: setting(&settings, "target_app_test_command"),
            status_command: setting(&settings, "target_app_status_command"),
            cwd: setting(&settings, "target_app_cwd"),
            env: serde_json::Map::new(),
            start_timeout_seconds: setting(&settings, "target_app_start_timeout_seconds")
                .parse()
                .unwrap_or(120),
            stop_timeout_seconds: setting(&settings, "target_app_stop_timeout_seconds")
                .parse()
                .unwrap_or(60),
            build_timeout_seconds: setting(&settings, "target_app_build_timeout_seconds")
                .parse()
                .unwrap_or(300),
            test_timeout_seconds: setting(&settings, "target_app_test_timeout_seconds")
                .parse()
                .unwrap_or(600),
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

        let mut notes = Vec::new();
        if clear_generated_wrapper_entrypoints(&mut config) {
            notes.push(
                "Ignored existing manage-app wrapper entrypoints while regenerating lifecycle instructions."
                    .to_string(),
            );
        }
        let project_root = self.command_cwd(&settings);
        if project_root.join("package.json").exists() {
            apply_package_json_defaults(&project_root, &mut config)?;
            notes.push(
                "Detected package.json and generated npm-compatible lifecycle instructions."
                    .to_string(),
            );
        } else if project_root.join("Cargo.toml").exists() {
            fill_if_empty(&mut config.start_command, "cargo run");
            fill_if_empty(&mut config.build_command, "cargo build");
            fill_if_empty(&mut config.test_command, "cargo test");
            fill_if_empty(&mut config.status_command, "cargo check --quiet");
            notes.push(
                "Detected Cargo.toml and generated cargo lifecycle instructions.".to_string(),
            );
        } else if project_root.join("Makefile").exists() || project_root.join("makefile").exists() {
            let makefile = if project_root.join("Makefile").exists() {
                project_root.join("Makefile")
            } else {
                project_root.join("makefile")
            };
            apply_makefile_defaults(&makefile, &mut config)?;
            notes.push(
                "Detected Makefile targets and generated make lifecycle instructions.".to_string(),
            );
        } else {
            notes.push("No package.json, Cargo.toml, or Makefile was detected; preserved existing target-app settings.".to_string());
        }

        if config.status_command.trim().is_empty() && !config.http_check_url.trim().is_empty() {
            config.status_command = format!(
                "curl -fsS {} >/dev/null",
                shell_quote(&config.http_check_url)
            );
        }
        if config.tcp_check_port.trim().is_empty()
            && let Some(port) = port_from_url(&config.http_check_url)
        {
            config.tcp_check_host = "127.0.0.1".to_string();
            config.tcp_check_port = port.to_string();
        }
        if config.stop_command.trim().is_empty() && !config.tcp_check_port.trim().is_empty() {
            config.stop_command = format!(
                "sh -c 'lsof -ti tcp:{} | xargs -r kill'",
                config.tcp_check_port
            );
            notes.push("Generated stop instruction targets the configured TCP port.".to_string());
        }
        apply_static_web_server_defaults(&project_root, &mut config, &mut notes);
        convert_lifecycle_commands_to_instructions(&mut config);
        config.notes = notes.join(" ");
        Ok(config)
    }

    pub fn write_manage_app_wrapper(
        &self,
        config: &mut TargetAppGeneratedConfig,
    ) -> RefineResult<()> {
        let wrapper_dir = self.refine_dir.clone();
        fs::create_dir_all(&wrapper_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create target-app wrapper directory {}: {error}",
                wrapper_dir.display()
            ))
        })?;

        if config.log_path.trim().is_empty() {
            config.log_path = MANAGE_APP_LOG_PATH.to_string();
        }
        let mut notes = Vec::new();
        if clear_generated_wrapper_entrypoints(config) {
            notes.push(
                "Ignored generated manage-app wrapper entrypoints before writing the wrapper."
                    .to_string(),
            );
        }
        let project_root = config_project_root(&self.target_root, &config.cwd);
        apply_static_web_server_defaults(&project_root, config, &mut notes);
        for note in notes {
            append_note(&mut config.notes, &note);
        }

        let wrapper_path = wrapper_dir.join("manage-app.sh");
        let script = manage_app_wrapper_script(config);
        fs::write(&wrapper_path, script).map_err(|error| {
            RefineError::Io(format!(
                "failed to write target-app wrapper {}: {error}",
                wrapper_path.display()
            ))
        })?;
        make_executable(&wrapper_path)?;

        config.start_command = manage_app_wrapper_entrypoint("start");
        config.stop_command = manage_app_wrapper_entrypoint("stop");
        config.build_command = manage_app_wrapper_entrypoint("build");
        config.test_command = manage_app_wrapper_entrypoint("test");
        config.status_command = manage_app_wrapper_entrypoint("status");
        config.cwd = ".".to_string();
        append_note(
            &mut config.notes,
            "Wrote the managed target-app wrapper outside the application worktree and pointed target-app commands at it.",
        );
        Ok(())
    }

    fn run_command(
        &self,
        kind: &str,
        command: &str,
        settings: &JsonObject,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<TargetAppOperation> {
        self.run_owned_command(
            kind,
            command,
            settings,
            process_metadata,
            ProcessOwner::TargetApp,
            "target_app",
        )
    }

    fn run_quality_command(
        &self,
        kind: &str,
        command: &str,
        settings: &JsonObject,
        process_metadata: Map<String, Value>,
    ) -> RefineResult<TargetAppOperation> {
        self.run_owned_command(
            kind,
            command,
            settings,
            process_metadata,
            ProcessOwner::Quality,
            "quality",
        )
    }

    fn run_agent_lifecycle(
        &self,
        kind: &str,
        instructions: &str,
        settings: &JsonObject,
        mut process_metadata: Map<String, Value>,
    ) -> TargetAppOperation {
        let started_at = now_timestamp();
        process_metadata.insert(
            "target_app_action".to_string(),
            Value::String(kind.to_string()),
        );
        process_metadata.insert(
            "target_root".to_string(),
            Value::String(self.target_root.display().to_string()),
        );
        let provider = setting(settings, "agent_cli")
            .trim()
            .to_string()
            .if_empty("claude");
        let cwd = self.command_cwd(settings);
        let prompt =
            target_app_lifecycle_prompt(kind, instructions, settings, &self.target_root, &cwd);
        let result = HostAgentProviderService::with_runtime_root(self.runtime_root.join("agents"))
            .invoke(ProviderInvocation {
                provider,
                prompt,
                session_id: None,
                cwd: Some(cwd.display().to_string()),
                process_metadata,
            });
        match result {
            Ok(output) => TargetAppOperation {
                id: new_operation_id(&format!("target-{kind}")),
                kind: kind.to_string(),
                state: "complete".to_string(),
                started_at,
                finished_at: now_timestamp(),
                exit_code: Some(0),
                stdout: output.trim().to_string(),
                stderr: String::new(),
            },
            Err(error) => TargetAppOperation {
                id: new_operation_id(&format!("target-{kind}")),
                kind: kind.to_string(),
                state: "failed".to_string(),
                started_at,
                finished_at: now_timestamp(),
                exit_code: Some(1),
                stdout: String::new(),
                stderr: error.to_string(),
            },
        }
    }

    fn run_owned_command(
        &self,
        kind: &str,
        command: &str,
        settings: &JsonObject,
        process_metadata: Map<String, Value>,
        owner: ProcessOwner,
        authorization_category: &str,
    ) -> RefineResult<TargetAppOperation> {
        let started_at = now_timestamp();
        FileSecurityService::from_project_settings(&self.runtime_root, &self.refine_dir)?
            .authorize_host_command(authorization_category, command)?;
        let (shell, args) = shell_program_args(command);
        let output = FileProcessSupervisor::new(&self.runtime_root).run_to_completion(
            ManagedProcessSpec {
                owner,
                command: shell,
                args,
                cwd: Some(self.command_cwd(settings).display().to_string()),
                env: command_env(settings)?,
                stdin: None,
                limits: None,
                authorization_command: Some(command.to_string()),
                sensitive: false,
                metadata: process_metadata,
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
            let operation =
                self.run_command("status", &status_command, settings, Default::default())?;
            checks.push(TargetCheckResult {
                ok: operation.exit_code == Some(0),
                message: operation_message(&operation),
            });
        }
        let process_command = setting(settings, "target_app_process_check_command");
        if !process_command.trim().is_empty() {
            let operation = self.run_command(
                "process-check",
                &process_command,
                settings,
                Default::default(),
            )?;
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
        FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root).load()
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
        let state_path = self.state_path();
        let temp_path = self.snapshot_temp_path();
        fs::write(&temp_path, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write target-app state temp file {}: {error}",
                temp_path.display()
            ))
        })?;
        fs::rename(&temp_path, &state_path).map_err(|error| {
            let _ = fs::remove_file(&temp_path);
            RefineError::Io(format!(
                "failed to replace target-app state {}: {error}",
                state_path.display()
            ))
        })
    }

    fn snapshot_temp_path(&self) -> PathBuf {
        let nanos = Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or_else(|| Utc::now().timestamp_micros() * 1000);
        self.runtime_root.join(format!(
            ".{TARGET_APP_STATE_FILE}.{}.{}.tmp",
            std::process::id(),
            nanos
        ))
    }

    fn command_cwd(&self, settings: &JsonObject) -> PathBuf {
        let cwd = setting(settings, "target_app_cwd");
        if cwd.trim().is_empty() {
            return self.target_root.clone();
        }
        let path = PathBuf::from(cwd);
        if path.is_absolute() {
            path
        } else {
            self.target_root.join(path)
        }
    }

    fn mark_target_processes_stopped(&self) -> RefineResult<()> {
        let supervisor = FileProcessSupervisor::new(&self.runtime_root);
        for process in supervisor
            .recover_owner(ProcessOwner::TargetApp)?
            .into_iter()
            .filter(|process| {
                process.owner == ProcessOwner::TargetApp && process.state != "stopped"
            })
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
    match kind {
        "start" => format!(
            "Start the target app. Use `{}` as the initial approach, but inspect the project and adapt if dependencies, ports, environment, or long-running process handling require it. Leave the app running in the background when appropriate, verify configured health checks when possible, and report the exact command and evidence.",
            command.trim()
        ),
        "stop" => format!(
            "Stop the target app. Use `{}` as the initial approach, but inspect the project and adapt if the process was started differently. Confirm the app is no longer reachable when possible and report the evidence.",
            command.trim()
        ),
        "build" => format!(
            "Build or rebuild the target app. Use `{}` as the initial approach, but inspect failures, install or repair project-local dependencies when safe, rerun the build as needed, and report exact blockers if the build cannot be completed.",
            command.trim()
        ),
        _ => command.trim().to_string(),
    }
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
    format!(
        "You are operating the target application for Refine.\n\nAction: {kind}\nTarget root: {}\nWorking directory: {}\nEnvironment overrides JSON: {}\nHealth URL: {}\nTCP check: {} {}\nStatus command hint: {}\nProcess check hint: {}\n\nInstructions:\n{}\n\nUse the host tools available in the working directory. Prefer durable, project-appropriate fixes over a brittle one-liner. If you start a long-running process, make sure this turn can finish after the app is started. If the action cannot be completed, explain the blocker and the evidence.",
        target_root.display(),
        cwd.display(),
        if env_json.trim().is_empty() {
            "{}"
        } else {
            env_json.trim()
        },
        health_url,
        tcp_host,
        tcp_port,
        status_command,
        process_command,
        instructions.trim()
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
    format!("sh \"$(git rev-parse --show-toplevel)-refine-live-state/manage-app.sh\" {action}")
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::TcpListener;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn target_app_service_runs_status_and_build_commands() {
        let temp_root = unique_temp_dir("target-app-service");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let target_root = temp_root.join("app");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "target_app_status_command": "test -f status-ok",
                "target_app_build_command": "touch built && echo built",
                "target_app_test_commands": [
                    {"command": "printf skipped > disabled-test", "enabled": false},
                    {"command": "printf first-ok > tested && echo first-ok", "enabled": true},
                    {"command": "printf second-ok > tested-two && echo second-ok", "enabled": true}
                ],
                "target_app_cwd": target_root.to_str().unwrap(),
                "allowed_commands": "test, touch, printf"
            }))
            .unwrap();
        fs::write(target_root.join("status-ok"), "").unwrap();
        let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

        let status = service.status().unwrap();
        assert_eq!(status.state, "running");
        assert!(status.last_check_ok);

        let built = service.build().unwrap();
        assert!(built.ok);
        assert!(target_root.join("built").exists());

        let tested = service.test().unwrap();
        assert!(tested.ok);
        assert_eq!(tested.last_operation.as_ref().unwrap().kind, "test");
        assert_eq!(tested.last_operation.as_ref().unwrap().stdout, "second-ok");
        assert!(target_root.join("tested").exists());
        assert!(target_root.join("tested-two").exists());
        assert!(!target_root.join("disabled-test").exists());
        let audit = fs::read_to_string(runtime_root.join("security-audit.jsonl")).unwrap();
        assert!(audit.contains("\"actor\":\"quality\""));
        assert!(runtime_root.join(TARGET_APP_STATE_FILE).exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_service_treats_missing_build_command_as_success() {
        let temp_root = unique_temp_dir("target-app-no-build");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let target_root = temp_root.join("app");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({"target_app_cwd": target_root.to_str().unwrap()}))
            .unwrap();
        let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

        let built = service.build().unwrap();
        assert!(built.ok);
        assert_eq!(built.state, "stopped");
        assert_eq!(
            built.message,
            "No target-app build instructions are configured."
        );
        assert!(built.last_check_ok);
        assert!(built.last_health_ok);
        assert_eq!(built.last_error, "");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_service_spawns_start_command_and_registers_process() {
        let temp_root = unique_temp_dir("target-app-start");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let target_root = temp_root.join("app");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "target_app_start_command": "printf target-started; sleep 2",
                "target_app_cwd": target_root.to_str().unwrap()
            }))
            .unwrap();
        let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

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
    fn target_app_service_runs_lifecycle_instructions_with_agent_provider() {
        let temp_root = unique_temp_dir("target-app-agent-lifecycle");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let target_root = temp_root.join("app");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        let smoke_ai = temp_root.join("smoke-ai");
        fs::write(&smoke_ai, "#!/bin/sh\nprintf 'agent lifecycle ok\\n'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&smoke_ai, permissions).unwrap();
        }
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "agent_cli": "smoke-ai",
                "target_app_start_instructions": "Start the target app and verify it.",
                "target_app_stop_instructions": "Stop the target app and verify it.",
                "target_app_build_instructions": "Build the target app and report evidence.",
                "target_app_cwd": target_root.to_str().unwrap()
            }))
            .unwrap();
        let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
        unsafe {
            std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
        }
        let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

        let started = service.start().unwrap();
        assert_eq!(started.state, "running");
        assert_eq!(
            started.last_operation.as_ref().unwrap().stdout,
            "agent lifecycle ok"
        );
        assert!(started.process_id.is_none());

        let built = service.build().unwrap();
        assert!(built.ok);
        assert_eq!(built.last_operation.as_ref().unwrap().kind, "build");
        assert_eq!(
            built.last_operation.as_ref().unwrap().stdout,
            "agent lifecycle ok"
        );

        let stopped = service.stop().unwrap();
        assert_eq!(stopped.state, "stopped");
        assert_eq!(
            stopped.last_operation.as_ref().unwrap().stdout,
            "agent lifecycle ok"
        );

        unsafe {
            match previous {
                Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
                None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
            }
        }
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_snapshot_write_replaces_longer_state() {
        let temp_root = unique_temp_dir("target-app-state");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let target_root = temp_root.join("app");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);
        let mut long_snapshot = TargetAppSnapshot {
            state: "running".to_string(),
            message: "Target application started.".to_string(),
            last_operation_id: "target-start-1".to_string(),
            last_operation: Some(TargetAppOperation {
                id: "target-start-1".to_string(),
                kind: "start".to_string(),
                state: "running".to_string(),
                started_at: now_timestamp(),
                finished_at: String::new(),
                exit_code: None,
                stdout: "long target app stdout".repeat(8),
                stderr: String::new(),
            }),
            process_id: Some("proc-target-app-state".to_string()),
            pid: Some(12345),
            ..TargetAppSnapshot::default()
        };
        service.save_snapshot(&long_snapshot).unwrap();
        long_snapshot.last_operation = None;
        long_snapshot.last_operation_id = String::new();
        long_snapshot.process_id = None;
        long_snapshot.pid = None;
        long_snapshot.message = "short".to_string();
        service.save_snapshot(&long_snapshot).unwrap();

        let raw = fs::read_to_string(service.state_path()).unwrap();
        assert!(!raw.contains("long target app stdout"));
        assert!(!raw.contains("proc-target-app-state"));
        let loaded = service.load_snapshot().unwrap();
        assert_eq!(loaded.message, "short");
        assert!(loaded.last_operation.is_none());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_service_generates_package_json_config() {
        let temp_root = unique_temp_dir("target-app-generate");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let target_root = temp_root.join("app");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        fs::write(
            target_root.join("package.json"),
            r#"{"scripts":{"dev":"vite","build":"vite build","test":"vitest run"}}"#,
        )
        .unwrap();
        fs::write(target_root.join("pnpm-lock.yaml"), "").unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "target_app_url": "http://127.0.0.1:5173",
                "target_app_cwd": target_root.to_str().unwrap()
            }))
            .unwrap();

        let generated = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root)
            .generate_config()
            .unwrap();
        assert_eq!(generated.start_command, "");
        assert_eq!(generated.build_command, "");
        assert!(generated.start_instructions.contains("pnpm run dev"));
        assert!(generated.build_instructions.contains("pnpm run build"));
        assert_eq!(generated.test_command, "pnpm test");
        assert_eq!(generated.tcp_check_port, "5173");
        assert!(generated.notes.contains("package.json"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_service_generates_static_web_server_for_package_without_start_script() {
        let temp_root = unique_temp_dir("target-app-static-package");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let target_root = temp_root.join("app");
        let port = free_test_port();
        fs::create_dir_all(&refine_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        fs::write(
            target_root.join("package.json"),
            r#"{"scripts":{"test":"node --test"}}"#,
        )
        .unwrap();
        fs::write(target_root.join("index.html"), "<h1>Static app</h1>").unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "target_app_url": format!("http://127.0.0.1:{port}/"),
                "target_app_cwd": target_root.to_str().unwrap()
            }))
            .unwrap();

        let generated = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root)
            .generate_config()
            .unwrap();
        assert!(generated.start_command.is_empty());
        assert!(generated.stop_command.is_empty());
        assert!(generated.build_command.is_empty());
        assert!(
            generated
                .start_instructions
                .contains("python3 -m http.server")
        );
        assert!(generated.stop_instructions.contains("target-app.pid"));
        assert!(
            generated
                .build_instructions
                .contains("No build step configured")
        );
        assert_eq!(
            generated.status_command,
            format!("curl -fsS 'http://127.0.0.1:{port}/' >/dev/null")
        );
        assert_eq!(generated.tcp_check_host, "127.0.0.1");
        assert_eq!(generated.tcp_check_port, port.to_string());
        assert!(generated.notes.contains("static web content"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_generation_does_not_embed_existing_manage_app_entrypoints() {
        let temp_root = unique_temp_dir("target-app-wrapper-regeneration");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        let target_root = temp_root.join("app");
        let port = free_test_port();
        fs::create_dir_all(&refine_dir).unwrap();
        fs::create_dir_all(&target_root).unwrap();
        fs::write(
            target_root.join("package.json"),
            r#"{"scripts":{"test":"node --test"}}"#,
        )
        .unwrap();
        fs::write(target_root.join("index.html"), "<h1>Static app</h1>").unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({
                "target_app_url": format!("http://127.0.0.1:{port}/"),
                "target_app_cwd": target_root.to_str().unwrap(),
                "target_app_start_command": "./.refine/manage-app.sh start",
                "target_app_stop_command": "./.refine/manage-app.sh stop",
                "target_app_build_command": "./.refine/manage-app.sh build",
                "target_app_test_command": "./.refine/manage-app.sh test",
                "target_app_status_command": "./.refine/manage-app.sh status"
            }))
            .unwrap();

        let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);
        let generated = service.generate_config().unwrap();

        assert!(generated.start_command.is_empty());
        assert!(generated.stop_command.is_empty());
        assert!(generated.build_command.is_empty());
        assert_eq!(generated.test_command, "npm test");
        assert_eq!(
            generated.status_command,
            format!("curl -fsS 'http://127.0.0.1:{port}/' >/dev/null")
        );
        assert!(
            generated
                .start_instructions
                .contains("python3 -m http.server")
        );
        assert!(generated.stop_instructions.contains("target-app.pid"));
        assert!(
            generated
                .notes
                .contains("Ignored existing manage-app wrapper entrypoints")
        );
        assert!(!target_root.join(".refine/manage-app.sh").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_service_writes_manage_app_wrapper() {
        let temp_root = unique_temp_dir("target-app-wrapper");
        let target_root = temp_root.join("app");
        let runtime_root = temp_root.join("run/8080");
        let inner_root = target_root.join("client");
        fs::create_dir_all(&inner_root).unwrap();
        git_init(&target_root);
        let refine_dir =
            crate::tools::host::project_layout::refine_dir_for_target_root(&target_root).unwrap();
        fs::create_dir_all(&refine_dir).unwrap();

        let mut config = TargetAppGeneratedConfig {
            start_instructions: String::new(),
            stop_instructions: String::new(),
            build_instructions: String::new(),
            start_command: "printf \"$WRAP_VALUE\" > ../started".to_string(),
            stop_command: String::new(),
            build_command: "printf built > ../built".to_string(),
            test_command: "printf tested > ../tested".to_string(),
            status_command: "printf status-ok".to_string(),
            cwd: "client".to_string(),
            env: serde_json::Map::from_iter([(
                "WRAP_VALUE".to_string(),
                Value::String("wrapped".to_string()),
            )]),
            start_timeout_seconds: 120,
            stop_timeout_seconds: 60,
            build_timeout_seconds: 300,
            test_timeout_seconds: 600,
            status_timeout_seconds: 10,
            log_path: String::new(),
            http_check_url: String::new(),
            tcp_check_host: String::new(),
            tcp_check_port: String::new(),
            process_check_command: String::new(),
            notes: "provider analysis".to_string(),
        };
        let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

        service.write_manage_app_wrapper(&mut config).unwrap();

        assert_eq!(config.start_command, manage_app_wrapper_entrypoint("start"));
        assert_eq!(config.stop_command, manage_app_wrapper_entrypoint("stop"));
        assert_eq!(config.build_command, manage_app_wrapper_entrypoint("build"));
        assert_eq!(config.test_command, manage_app_wrapper_entrypoint("test"));
        assert_eq!(
            config.status_command,
            manage_app_wrapper_entrypoint("status")
        );
        assert_eq!(config.cwd, ".");
        assert_eq!(config.log_path, MANAGE_APP_LOG_PATH);

        let wrapper_path = refine_dir.join("manage-app.sh");
        let script = fs::read_to_string(&wrapper_path).unwrap();
        assert!(script.contains("APP_CWD='client'"));
        assert!(script.contains("START_COMMAND='printf \"$WRAP_VALUE\" > ../started'"));
        assert!(script.contains("TEST_COMMAND='printf tested > ../tested'"));
        assert!(script.contains("# Analysis notes: provider analysis"));
        assert!(script.contains("export WRAP_VALUE='wrapped'"));

        let output = std::process::Command::new(&wrapper_path)
            .arg("start")
            .current_dir(&target_root)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            fs::read_to_string(target_root.join("started")).unwrap(),
            "wrapped"
        );
        let log = fs::read_to_string(refine_dir.join("manage-app.log")).unwrap();
        assert!(log.contains("[start] cwd="));
        assert!(log.contains("[start] exit=0"));
        assert!(!target_root.join(".refine").exists());

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn target_app_wrapper_turns_partial_ai_web_config_into_managed_server() {
        let temp_root = unique_temp_dir("target-app-wrapper-static");
        let target_root = temp_root.join("app");
        let runtime_root = temp_root.join("run/8080");
        let port = free_test_port();
        fs::create_dir_all(&target_root).unwrap();
        git_init(&target_root);
        let refine_dir =
            crate::tools::host::project_layout::refine_dir_for_target_root(&target_root).unwrap();
        fs::create_dir_all(&refine_dir).unwrap();
        fs::write(target_root.join("index.html"), "<h1>AI static app</h1>").unwrap();

        let mut config = TargetAppGeneratedConfig {
            start_instructions: String::new(),
            stop_instructions: String::new(),
            build_instructions: String::new(),
            start_command: String::new(),
            stop_command: String::new(),
            build_command: String::new(),
            test_command: "npm test".to_string(),
            status_command: "npm test -- --help >/dev/null 2>&1 || true".to_string(),
            cwd: ".".to_string(),
            env: serde_json::Map::new(),
            start_timeout_seconds: 120,
            stop_timeout_seconds: 60,
            build_timeout_seconds: 300,
            test_timeout_seconds: 600,
            status_timeout_seconds: 10,
            log_path: String::new(),
            http_check_url: format!("http://127.0.0.1:{port}/"),
            tcp_check_host: String::new(),
            tcp_check_port: String::new(),
            process_check_command: String::new(),
            notes: "provider returned only a test status command".to_string(),
        };
        let service = FileTargetAppService::new(&refine_dir, &runtime_root, &target_root);

        service.write_manage_app_wrapper(&mut config).unwrap();

        assert_eq!(config.start_command, manage_app_wrapper_entrypoint("start"));
        assert_eq!(config.stop_command, manage_app_wrapper_entrypoint("stop"));
        assert_eq!(config.build_command, manage_app_wrapper_entrypoint("build"));
        assert_eq!(config.test_command, manage_app_wrapper_entrypoint("test"));
        assert_eq!(
            config.status_command,
            manage_app_wrapper_entrypoint("status")
        );
        assert!(config.notes.contains("static web content"));

        let wrapper_path = refine_dir.join("manage-app.sh");
        let script = fs::read_to_string(&wrapper_path).unwrap();
        assert!(!script.contains("START_COMMAND=''"));
        assert!(!script.contains("STOP_COMMAND=''"));
        assert!(script.contains(&format!("PORT={port};")));
        assert!(script.contains(&format!("http://127.0.0.1:{port}/")));
        assert!(script.contains("python3 -m http.server"));
        assert!(script.contains("STATUS_COMMAND='curl -fsS"));

        let start = std::process::Command::new(&wrapper_path)
            .arg("start")
            .current_dir(&target_root)
            .output()
            .unwrap();
        assert!(
            start.status.success(),
            "start failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&start.stdout),
            String::from_utf8_lossy(&start.stderr)
        );
        assert!(!target_root.join(".refine").exists());
        let status = std::process::Command::new(&wrapper_path)
            .arg("status")
            .current_dir(&target_root)
            .output()
            .unwrap();
        let stop = std::process::Command::new(&wrapper_path)
            .arg("stop")
            .current_dir(&target_root)
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "status failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr)
        );
        assert!(
            stop.status.success(),
            "stop failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&stop.stdout),
            String::from_utf8_lossy(&stop.stderr)
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }

    fn git_init(root: &Path) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn free_test_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
}

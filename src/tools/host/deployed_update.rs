use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    BackgroundDaemonConfig, DaemonRuntimeService, DaemonStatus, FileDaemonLifecycleService,
    http_probe,
};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::daemon_lifecycle::{
    DaemonLifecycleAction, FileHostDaemonLifecycleService, execute_daemon_lifecycle,
};
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::product::project_registry::FileProjectRegistryService;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeployedUpdateSummary {
    pub ok: bool,
    pub checkout_path: String,
    pub runtime_root: String,
    pub stopped_ports: Vec<u16>,
    pub target_version: Option<String>,
    pub binary_path: String,
    pub build_result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_agent: Option<UpdateAgentRunSummary>,
    pub restarted_ports: Vec<u16>,
    pub failures: Vec<DeployedUpdateFailure>,
    pub manual_recovery_command: Option<String>,
    pub rollback_possible: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeployedUpdateFailure {
    pub stage: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateAgentRunSummary {
    pub provider: String,
    pub prompt: String,
    pub output: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateAgentInvocation {
    pub provider: String,
    pub prompt: String,
    pub cwd: PathBuf,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateAgentOutcome {
    pub succeeded: bool,
    pub target_version: Option<String>,
    pub binary_path: Option<PathBuf>,
    pub output: String,
}

pub trait DeployedUpdateHost {
    fn running_ports(&mut self) -> RefineResult<Vec<u16>>;
    fn stop_port(&mut self, port: u16) -> RefineResult<()>;
    fn port_stopped(&mut self, port: u16) -> RefineResult<bool>;
    fn run_update_agent(
        &mut self,
        invocation: &UpdateAgentInvocation,
    ) -> RefineResult<UpdateAgentOutcome>;
    fn verify_binary_mode(&mut self, checkout: &Path, binary_path: &Path) -> RefineResult<()>;
    fn restart_port(&mut self, port: u16) -> RefineResult<DaemonStatus>;
}

#[derive(Clone, Debug)]
pub struct DeployedUpdateOptions {
    pub checkout_path: PathBuf,
    pub runtime_root: PathBuf,
    pub assume_yes: bool,
    pub provider: String,
}

impl DeployedUpdateOptions {
    pub fn new(checkout_path: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            checkout_path: checkout_path.into(),
            runtime_root: runtime_root.into(),
            assume_yes: false,
            provider: default_update_provider(),
        }
    }

    pub fn with_assume_yes(mut self, assume_yes: bool) -> Self {
        self.assume_yes = assume_yes;
        self
    }

    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.provider = provider.into();
        self
    }
}

pub fn default_update_provider() -> String {
    "claude".to_string()
}

/// Pick the provider that performs a delegated update: an explicit `--provider`
/// wins, then the active target app's configured provider, then the first
/// installed provider CLI in alphabetical order.
pub fn resolve_agent_provider(
    runtime_root: &Path,
    explicit: Option<String>,
) -> RefineResult<String> {
    if let Some(provider) = explicit
        .map(|provider| provider.trim().to_string())
        .filter(|provider| !provider.is_empty())
    {
        return Ok(provider);
    }
    if let Some(provider) = target_app_configured_provider(runtime_root) {
        return Ok(provider);
    }
    first_installed_provider()
}

fn target_app_configured_provider(runtime_root: &Path) -> Option<String> {
    let registry = FileProjectRegistryService::new(runtime_root, None)
        .load()
        .ok()?;
    let target_root = PathBuf::from(registry.active_app?);
    let refine_dir = refine_dir_for_target_root(&target_root).ok()?;
    let settings = FileSettingsService::with_active_root(refine_dir, target_root)
        .load()
        .ok()?;
    settings
        .get("agent_cli")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(str::to_string)
}

fn first_installed_provider() -> RefineResult<String> {
    let mut installed = HostAgentProviderService::new()
        .detect()?
        .into_iter()
        .filter(|capability| capability.installed && capability.name != "smoke-ai")
        .map(|capability| capability.name)
        .collect::<Vec<_>>();
    installed.sort();
    installed.into_iter().next().ok_or_else(|| {
        RefineError::NotFound(
            "no agent provider CLI found on PATH; install one (e.g. claude, codex, copilot, gemini) or pass --provider".to_string(),
        )
    })
}

pub fn run_deployed_update<H: DeployedUpdateHost>(
    host: &mut H,
    options: DeployedUpdateOptions,
) -> DeployedUpdateSummary {
    let checkout_path = options.checkout_path;
    let runtime_root = options.runtime_root;
    let assume_yes = options.assume_yes;
    let provider = options.provider;
    let binary_path = checkout_path.join("bin/refine");
    let mut summary = DeployedUpdateSummary {
        ok: false,
        checkout_path: checkout_path.display().to_string(),
        runtime_root: runtime_root.display().to_string(),
        binary_path: binary_path.display().to_string(),
        build_result: "not_started".to_string(),
        manual_recovery_command: Some(manual_update_recovery_command(
            &checkout_path,
            &provider,
            assume_yes,
        )),
        rollback_possible: false,
        ..Default::default()
    };

    let running_ports = match host.running_ports() {
        Ok(ports) => ports,
        Err(error) => {
            push_failure(&mut summary, "discover_ports", error);
            return summary;
        }
    };

    for port in &running_ports {
        if let Err(error) = host.stop_port(*port) {
            push_failure(&mut summary, "stop_ports", error);
            return summary;
        }
        summary.stopped_ports.push(*port);
    }

    for port in &running_ports {
        match host.port_stopped(*port) {
            Ok(true) => {}
            Ok(false) => {
                summary.failures.push(DeployedUpdateFailure {
                    stage: "verify_stopped".to_string(),
                    message: format!("Refine daemon on port {port} is still reachable"),
                });
                return summary;
            }
            Err(error) => {
                push_failure(&mut summary, "verify_stopped", error);
                return summary;
            }
        }
    }

    let invocation = update_agent_invocation(&checkout_path, &provider, assume_yes);
    let agent = match host.run_update_agent(&invocation) {
        Ok(outcome) => outcome,
        Err(error) => {
            summary.build_result = "failed".to_string();
            push_failure(&mut summary, "update_agent", error);
            return summary;
        }
    };
    summary.update_agent = Some(UpdateAgentRunSummary {
        provider: invocation.provider.clone(),
        prompt: invocation.prompt.clone(),
        output: agent.output.clone(),
    });
    if !agent.succeeded {
        summary.build_result = "failed".to_string();
        summary.failures.push(DeployedUpdateFailure {
            stage: "update_agent".to_string(),
            message: update_agent_failure_message(&agent),
        });
        return summary;
    }
    summary.build_result = "succeeded".to_string();
    summary.target_version = agent
        .target_version
        .or_else(|| cargo_package_version(&checkout_path));
    if let Some(path) = agent.binary_path {
        summary.binary_path = path.display().to_string();
    }

    if let Err(error) = host.verify_binary_mode(&checkout_path, Path::new(&summary.binary_path)) {
        push_failure(&mut summary, "verify_binary_mode", error);
        summary.rollback_possible = true;
        return summary;
    }

    for port in running_ports {
        match host.restart_port(port) {
            Ok(_) => summary.restarted_ports.push(port),
            Err(error) => {
                push_failure(&mut summary, "restart_ports", error);
                summary.rollback_possible = true;
            }
        }
    }

    summary.ok = summary.failures.is_empty();
    if summary.ok {
        summary.manual_recovery_command = None;
    }
    summary
}

pub const UPDATE_RUNBOOK_PATH: &str = "docs/runbooks/install.md";

pub fn update_agent_prompt(checkout_path: &Path, assume_yes: bool) -> String {
    let defaults = if assume_yes {
        " Proceed with the documented defaults instead of asking questions."
    } else {
        ""
    };
    format!(
        "Update the deployed Refine checkout at {checkout} by following the \"Update Refine\" \
         section of {runbook} in that checkout. Refine daemons are already stopped and the \
         caller manages the daemon lifecycle: do not run `./r system update` and do not start, \
         stop, or restart Refine. When you finish, the freshly built release binary must be \
         installed at bin/refine and .refine-deployed must mark the checkout as deployed. If a \
         step fails, stop and report the exact blocker.{defaults}",
        checkout = checkout_path.display(),
        runbook = UPDATE_RUNBOOK_PATH,
    )
}

pub fn update_agent_invocation(
    checkout_path: &Path,
    provider: &str,
    assume_yes: bool,
) -> UpdateAgentInvocation {
    UpdateAgentInvocation {
        provider: provider.to_string(),
        prompt: update_agent_prompt(checkout_path, assume_yes),
        cwd: checkout_path.to_path_buf(),
    }
}

fn manual_update_recovery_command(
    checkout_path: &Path,
    provider: &str,
    assume_yes: bool,
) -> String {
    format!(
        "cd {} && ./r agent invoke {} --provider {}",
        shell_quote(&checkout_path.display().to_string()),
        shell_quote(&update_agent_prompt(checkout_path, assume_yes)),
        shell_quote(provider)
    )
}

fn update_agent_failure_message(agent: &UpdateAgentOutcome) -> String {
    let detail = agent.output.trim();
    if detail.is_empty() {
        "update agent failed without output".to_string()
    } else {
        format!("update agent failed: {detail}")
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

#[derive(Clone, Debug)]
pub struct FileDeployedUpdateHost {
    pub runtime_root: PathBuf,
}

impl FileDeployedUpdateHost {
    pub fn new(runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
        }
    }

    fn runtime_lifecycle(&self) -> FileDaemonLifecycleService {
        FileDaemonLifecycleService::new(RuntimeRoot {
            root: self.runtime_root.clone(),
        })
    }

    fn lifecycle(&self) -> FileHostDaemonLifecycleService {
        FileHostDaemonLifecycleService::new(
            RuntimeRoot {
                root: self.runtime_root.clone(),
            },
            env!("CARGO_PKG_VERSION"),
        )
    }
}

impl DeployedUpdateHost for FileDeployedUpdateHost {
    fn running_ports(&mut self) -> RefineResult<Vec<u16>> {
        let statuses = self.runtime_lifecycle().known_statuses()?;
        Ok(statuses
            .into_iter()
            .filter(|status| status.daemon_healthy && status.web_available)
            .map(|status| status.port)
            .collect())
    }

    fn stop_port(&mut self, port: u16) -> RefineResult<()> {
        execute_daemon_lifecycle(
            &self.lifecycle(),
            DaemonLifecycleAction::Stop,
            BackgroundDaemonConfig {
                port,
                ..Default::default()
            },
        )
        .map(|_| ())
    }

    fn port_stopped(&mut self, port: u16) -> RefineResult<bool> {
        let status = self.runtime_lifecycle().status(port)?;
        Ok(!status.daemon_healthy && http_probe(port).is_err())
    }

    fn run_update_agent(
        &mut self,
        invocation: &UpdateAgentInvocation,
    ) -> RefineResult<UpdateAgentOutcome> {
        if !invocation.cwd.join(UPDATE_RUNBOOK_PATH).is_file() {
            return Err(RefineError::NotFound(format!(
                "update runbook not found: {}",
                invocation.cwd.join(UPDATE_RUNBOOK_PATH).display()
            )));
        }
        let output = HostAgentProviderService::new().invoke(ProviderInvocation {
            provider: invocation.provider.clone(),
            prompt: invocation.prompt.clone(),
            session_id: None,
            cwd: Some(invocation.cwd.display().to_string()),
            process_metadata: Default::default(),
        })?;
        Ok(UpdateAgentOutcome {
            succeeded: true,
            target_version: cargo_package_version(&invocation.cwd),
            binary_path: Some(invocation.cwd.join("bin/refine")),
            output,
        })
    }

    fn verify_binary_mode(&mut self, checkout: &Path, binary_path: &Path) -> RefineResult<()> {
        let output = Command::new(checkout.join("r"))
            .current_dir(checkout)
            .env("REFINE_R_DRY_RUN", "1")
            .output()
            .map_err(|error| RefineError::Io(format!("failed to verify ./r mode: {error}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            return Err(RefineError::Conflict(format!(
                "./r mode check failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        if !stdout.contains("mode=binary") {
            return Err(RefineError::Conflict(
                "./r did not select deployed binary mode after update".to_string(),
            ));
        }
        if !binary_path.exists() {
            return Err(RefineError::NotFound(format!(
                "deployed binary missing after update: {}",
                binary_path.display()
            )));
        }
        Ok(())
    }

    fn restart_port(&mut self, port: u16) -> RefineResult<DaemonStatus> {
        execute_daemon_lifecycle(
            &self.lifecycle(),
            DaemonLifecycleAction::Start,
            BackgroundDaemonConfig {
                port,
                ..Default::default()
            },
        )
    }
}

pub fn discover_refine_checkout() -> RefineResult<PathBuf> {
    let cwd = std::env::current_dir().map_err(|error| {
        RefineError::Io(format!("failed to resolve current directory: {error}"))
    })?;
    if let Some(path) = find_checkout_from(&cwd) {
        return Ok(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(path) = exe.parent().and_then(find_checkout_from)
    {
        return Ok(path);
    }
    Err(RefineError::NotFound(
        "could not find a Refine checkout; run ./r system update from the Refine checkout"
            .to_string(),
    ))
}

pub fn active_refine_paths() -> RefineResult<(PathBuf, PathBuf)> {
    let executable = std::env::current_exe().map_err(|error| {
        RefineError::Io(format!(
            "failed to resolve active Refine executable: {error}"
        ))
    })?;
    let executable = executable.canonicalize().unwrap_or(executable);
    let checkout = executable
        .ancestors()
        .find(|path| is_refine_checkout(path))
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(discover_refine_checkout)?;
    Ok((executable, checkout))
}

pub fn is_refine_checkout(path: &Path) -> bool {
    path.join(".git").exists()
        && path.join("Cargo.toml").is_file()
        && path.join("src/main.rs").is_file()
        && path.join(UPDATE_RUNBOOK_PATH).is_file()
        && path.join("r").is_file()
}

fn find_checkout_from(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(path) = current {
        if is_refine_checkout(path) {
            return Some(path.to_path_buf());
        }
        current = path.parent();
    }
    None
}

fn cargo_package_version(checkout: &Path) -> Option<String> {
    let text = std::fs::read_to_string(checkout.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        if in_package && trimmed.starts_with('[') {
            return None;
        }
        if in_package
            && let Some(value) = trimmed.strip_prefix("version")
            && let Some(value) = value.split('=').nth(1)
        {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn push_failure(summary: &mut DeployedUpdateSummary, stage: &str, error: RefineError) {
    summary.failures.push(DeployedUpdateFailure {
        stage: stage.to_string(),
        message: error.to_string(),
    });
}

#[cfg(test)]
mod tests;

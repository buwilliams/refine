use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::core::host::agent_providers::{AgentProviderService, HostAgentProviderService};
use crate::core::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::core::host::installation::{
    FileInstallationService, InstallTarget, InstallationService,
};
use crate::core::host::process_supervision::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner,
};
use crate::core::observability::activity::FileActivityService;
use crate::core::product::project_registry::FileProjectRegistryService;
use crate::core::supervisor::errors::RefineResult;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DoctorReport {
    pub daemon: Vec<String>,
    pub install: Vec<String>,
    pub os_backend: Vec<String>,
    pub target_app: Vec<String>,
    pub git: Vec<String>,
    pub provider: Vec<String>,
    pub browser: Vec<String>,
    pub docker: Vec<String>,
    pub storage: Vec<String>,
}

pub trait DiagnosticsService {
    fn doctor(&self) -> RefineResult<DoctorReport>;
}

#[derive(Clone, Debug)]
pub struct FileDiagnosticsService {
    pub durable_root: Option<PathBuf>,
    pub runtime_root: PathBuf,
    pub repo_root: PathBuf,
}

impl FileDiagnosticsService {
    pub fn new(
        durable_root: Option<PathBuf>,
        runtime_root: impl Into<PathBuf>,
        repo_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            durable_root,
            runtime_root: runtime_root.into(),
            repo_root: repo_root.into(),
        }
    }
}

impl DiagnosticsService for FileDiagnosticsService {
    fn doctor(&self) -> RefineResult<DoctorReport> {
        let project_status =
            FileProjectRegistryService::new(&self.runtime_root, self.durable_root.clone())
                .status()?;
        let process_summary = FileProcessSupervisor::new(&self.runtime_root).list()?;
        let providers = HostAgentProviderService::new().detect().unwrap_or_default();
        let installed_providers = providers
            .iter()
            .filter(|provider| provider.installed)
            .map(|provider| provider.name.clone())
            .collect::<Vec<_>>();
        let install_status =
            FileInstallationService::new(&self.runtime_root, env!("CARGO_PKG_VERSION"))
                .status()
                .map(|status| {
                    format!(
                        "installed={} target={} version={}",
                        status.installed,
                        install_target_label(&status.target),
                        status.version.unwrap_or_else(|| "unknown".to_string())
                    )
                })
                .unwrap_or_else(|error| format!("install status unavailable: {error}"));
        let git_status =
            FileGitWorktreeService::with_runtime_root(&self.repo_root, &self.runtime_root)
                .inspect("")
                .map(|status| {
                    if status.dirty_user_changes {
                        format!(
                            "git worktree has user changes on {}",
                            status.branch.unwrap_or_else(|| "detached".to_string())
                        )
                    } else {
                        format!(
                            "git worktree clean on {}",
                            status.branch.unwrap_or_else(|| "detached".to_string())
                        )
                    }
                })
                .unwrap_or_else(|error| format!("git inspection unavailable: {error}"));
        let activity_count = self
            .durable_root
            .as_ref()
            .and_then(|root| FileActivityService::new(root).count().ok())
            .unwrap_or(0);
        let browser_status = command_status(
            &self.runtime_root,
            "playwright",
            &["--version"],
            "playwright CLI available",
            "playwright CLI not found; browser QA remains task-scoped",
        );
        let docker_status = command_status(
            &self.runtime_root,
            "docker",
            &["--version"],
            "docker CLI available",
            "docker CLI not found; Docker-dependent target workflows will be blocked",
        );
        let storage_root = self
            .durable_root
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string());

        Ok(DoctorReport {
            daemon: vec![
                "native diagnostics service reachable".to_string(),
                format!("runtime_root={}", self.runtime_root.display()),
                format!("managed_processes={}", process_summary.len()),
            ],
            install: vec![install_status],
            os_backend: vec![
                format!("os={}", std::env::consts::OS),
                os_backend_hint().to_string(),
            ],
            target_app: vec![
                if project_status.attached {
                    format!(
                        "attached={}",
                        project_status
                            .client_repo
                            .unwrap_or_else(|| "unknown".to_string())
                    )
                } else {
                    "no project attached".to_string()
                },
                "target-app commands and registered processes are supervised by the native daemon"
                    .to_string(),
            ],
            git: vec![git_status],
            provider: vec![
                format!("detected_providers={}", providers.len()),
                format!("installed_providers={}", installed_providers.join(",")),
            ],
            browser: vec![browser_status],
            docker: vec![docker_status],
            storage: vec![
                format!("durable_root={storage_root}"),
                format!(
                    "durable_root_exists={}",
                    durable_root_exists(&self.durable_root)
                ),
                format!("runtime_root_exists={}", self.runtime_root.exists()),
                format!("activity_entries={activity_count}"),
            ],
        })
    }
}

fn command_status(
    runtime_root: &std::path::Path,
    command: &str,
    args: &[&str],
    available: &str,
    missing: &str,
) -> String {
    let output = FileProcessSupervisor::new(runtime_root).run_to_completion(ManagedProcessSpec {
        owner: ProcessOwner::Maintenance,
        command: command.to_string(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: None,
        env: Vec::new(),
        stdin: None,
        limits: None,
        authorization_command: Some(format!("{} {}", command, args.join(" "))),
        sensitive: false,
    });
    match output {
        Ok(output) if output.success() => {
            let version = output
                .stdout
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if version.is_empty() {
                available.to_string()
            } else {
                format!("{available}: {version}")
            }
        }
        Ok(output) => format!(
            "{missing}: exited {}",
            output
                .process
                .exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ),
        Err(_) => missing.to_string(),
    }
}

fn os_backend_hint() -> &'static str {
    match std::env::consts::OS {
        "linux" => "service_backend=systemd_or_process",
        "macos" => "service_backend=launchd_or_login_item",
        "windows" => "service_backend=service_or_user_session",
        _ => "service_backend=process",
    }
}

fn install_target_label(target: &InstallTarget) -> &'static str {
    match target {
        InstallTarget::MacOsAppBundle => "macos_app_bundle",
        InstallTarget::WindowsInstaller => "windows_installer",
        InstallTarget::LinuxCliWeb => "linux_cli_web",
    }
}

fn durable_root_exists(durable_root: &Option<PathBuf>) -> bool {
    durable_root
        .as_ref()
        .map(|path| path.exists())
        .unwrap_or(false)
}

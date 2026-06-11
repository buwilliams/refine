use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    BackgroundDaemonConfig, DaemonLifecycleService, DaemonStatus, FileDaemonLifecycleService,
    http_probe,
};
use crate::process::supervisor::runtime::RuntimeRoot;

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
    pub installer: Option<InstallerRunSummary>,
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
pub struct InstallerRunSummary {
    pub command: Vec<String>,
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallerInvocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InstallerOutcome {
    pub succeeded: bool,
    pub status: Option<i32>,
    pub target_version: Option<String>,
    pub binary_path: Option<PathBuf>,
    pub stdout: String,
    pub stderr: String,
}

pub trait DeployedUpdateHost {
    fn running_ports(&mut self) -> RefineResult<Vec<u16>>;
    fn stop_port(&mut self, port: u16) -> RefineResult<()>;
    fn port_stopped(&mut self, port: u16) -> RefineResult<bool>;
    fn run_installer(&mut self, invocation: &InstallerInvocation)
    -> RefineResult<InstallerOutcome>;
    fn verify_binary_mode(&mut self, checkout: &Path, binary_path: &Path) -> RefineResult<()>;
    fn restart_port(&mut self, port: u16) -> RefineResult<DaemonStatus>;
}

#[derive(Clone, Debug)]
pub struct DeployedUpdateOptions {
    pub checkout_path: PathBuf,
    pub runtime_root: PathBuf,
    pub assume_yes: bool,
}

impl DeployedUpdateOptions {
    pub fn new(checkout_path: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            checkout_path: checkout_path.into(),
            runtime_root: runtime_root.into(),
            assume_yes: false,
        }
    }

    pub fn with_assume_yes(mut self, assume_yes: bool) -> Self {
        self.assume_yes = assume_yes;
        self
    }
}

pub fn run_deployed_update<H: DeployedUpdateHost>(
    host: &mut H,
    options: DeployedUpdateOptions,
) -> DeployedUpdateSummary {
    let checkout_path = options.checkout_path;
    let runtime_root = options.runtime_root;
    let assume_yes = options.assume_yes;
    let binary_path = checkout_path.join("bin/refine");
    let mut summary = DeployedUpdateSummary {
        ok: false,
        checkout_path: checkout_path.display().to_string(),
        runtime_root: runtime_root.display().to_string(),
        binary_path: binary_path.display().to_string(),
        build_result: "not_started".to_string(),
        manual_recovery_command: Some(manual_update_recovery_command(
            &checkout_path,
            &runtime_root,
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

    let invocation = installer_invocation(&checkout_path, &runtime_root, assume_yes);
    let installer = match host.run_installer(&invocation) {
        Ok(outcome) => outcome,
        Err(error) => {
            summary.build_result = "failed".to_string();
            push_failure(&mut summary, "installer", error);
            return summary;
        }
    };
    summary.installer = Some(InstallerRunSummary {
        command: invocation.command_line(),
        status: installer.status,
        stdout: installer.stdout.clone(),
        stderr: installer.stderr.clone(),
    });
    if !installer.succeeded {
        summary.build_result = "failed".to_string();
        summary.failures.push(DeployedUpdateFailure {
            stage: "installer".to_string(),
            message: installer_failure_message(&installer),
        });
        return summary;
    }
    summary.build_result = "succeeded".to_string();
    summary.target_version = installer
        .target_version
        .or_else(|| cargo_package_version(&checkout_path));
    if let Some(path) = installer.binary_path {
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

pub fn installer_invocation(
    checkout_path: &Path,
    runtime_root: &Path,
    assume_yes: bool,
) -> InstallerInvocation {
    let mut args = vec!["--upgrade".to_string()];
    if assume_yes {
        args.insert(0, "--yes".to_string());
    }
    let mut env = vec![
        ("REFINE_INSTALL_UPGRADE".to_string(), "1".to_string()),
        ("REFINE_INSTALL_UPDATE_ONLY".to_string(), "1".to_string()),
        (
            "REFINE_INSTALL_CHECKOUT_DEFAULT".to_string(),
            checkout_path.display().to_string(),
        ),
        (
            "REFINE_INSTALL_RUNTIME_ROOT".to_string(),
            runtime_root.display().to_string(),
        ),
    ];
    if assume_yes {
        env.push((
            "REFINE_INSTALL_ASSUME_DEFAULTS".to_string(),
            "1".to_string(),
        ));
    }
    InstallerInvocation {
        program: checkout_path.join("scripts/install.sh"),
        args,
        cwd: checkout_path.to_path_buf(),
        env,
    }
}

impl InstallerInvocation {
    fn command_line(&self) -> Vec<String> {
        let mut command = Vec::with_capacity(1 + self.args.len());
        command.push(self.program.display().to_string());
        command.extend(self.args.clone());
        command
    }
}

fn manual_update_recovery_command(
    checkout_path: &Path,
    runtime_root: &Path,
    assume_yes: bool,
) -> String {
    let yes_flag = if assume_yes { " --yes" } else { "" };
    format!(
        "cd {} && REFINE_INSTALL_UPDATE_ONLY=1 REFINE_INSTALL_RUNTIME_ROOT={} REFINE_INSTALL_CHECKOUT_DEFAULT={} scripts/install.sh{} --upgrade",
        shell_quote(&checkout_path.display().to_string()),
        shell_quote(&runtime_root.display().to_string()),
        shell_quote(&checkout_path.display().to_string()),
        yes_flag
    )
}

fn installer_failure_message(installer: &InstallerOutcome) -> String {
    let mut message = format!(
        "installer failed with status {}",
        installer
            .status
            .map(|status| status.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    );
    let detail = if installer.stderr.trim().is_empty() {
        installer.stdout.trim()
    } else {
        installer.stderr.trim()
    };
    if !detail.is_empty() {
        message.push_str(": ");
        message.push_str(detail);
    }
    message
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

    fn lifecycle(&self) -> FileDaemonLifecycleService {
        FileDaemonLifecycleService::new(RuntimeRoot {
            root: self.runtime_root.clone(),
        })
    }
}

impl DeployedUpdateHost for FileDeployedUpdateHost {
    fn running_ports(&mut self) -> RefineResult<Vec<u16>> {
        let statuses = self.lifecycle().known_statuses()?;
        Ok(statuses
            .into_iter()
            .filter(|status| status.daemon_healthy && status.web_available)
            .map(|status| status.port)
            .collect())
    }

    fn stop_port(&mut self, port: u16) -> RefineResult<()> {
        self.lifecycle().stop(port).map(|_| ())
    }

    fn port_stopped(&mut self, port: u16) -> RefineResult<bool> {
        let status = self.lifecycle().status(port)?;
        Ok(!status.daemon_healthy && http_probe(port).is_err())
    }

    fn run_installer(
        &mut self,
        invocation: &InstallerInvocation,
    ) -> RefineResult<InstallerOutcome> {
        if !invocation.program.is_file() {
            return Err(RefineError::NotFound(format!(
                "installer not found: {}",
                invocation.program.display()
            )));
        }
        let output = Command::new(&invocation.program)
            .args(&invocation.args)
            .current_dir(&invocation.cwd)
            .envs(invocation.env.iter().map(|(key, value)| (key, value)))
            .output()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to launch installer {}: {error}",
                    invocation.program.display()
                ))
            })?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok(InstallerOutcome {
            succeeded: output.status.success(),
            status: output.status.code(),
            target_version: cargo_package_version(&invocation.cwd),
            binary_path: Some(invocation.cwd.join("bin/refine")),
            stdout,
            stderr,
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
        self.lifecycle()
            .start_background_daemon(BackgroundDaemonConfig {
                port,
                ..Default::default()
            })
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

fn find_checkout_from(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(path) = current {
        if path.join("Cargo.toml").is_file()
            && path.join("src/main.rs").is_file()
            && path.join("scripts/install.sh").is_file()
            && path.join("r").is_file()
        {
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
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[derive(Default)]
    struct FakeUpdateHost {
        running_ports: Vec<u16>,
        stopped_ports: BTreeSet<u16>,
        restarted_ports: Vec<u16>,
        installer_seen_all_stopped: bool,
        fail_installer: bool,
        installer_succeeded: bool,
        installer_status: Option<i32>,
        installer_stderr: String,
        fail_verify: bool,
        fail_restart: bool,
        invocations: Vec<InstallerInvocation>,
        verify_stopped: bool,
        events: Vec<String>,
    }

    impl DeployedUpdateHost for FakeUpdateHost {
        fn running_ports(&mut self) -> RefineResult<Vec<u16>> {
            Ok(self.running_ports.clone())
        }

        fn stop_port(&mut self, port: u16) -> RefineResult<()> {
            self.events.push(format!("stop:{port}"));
            self.stopped_ports.insert(port);
            Ok(())
        }

        fn port_stopped(&mut self, port: u16) -> RefineResult<bool> {
            Ok(self.verify_stopped && self.stopped_ports.contains(&port))
        }

        fn run_installer(
            &mut self,
            invocation: &InstallerInvocation,
        ) -> RefineResult<InstallerOutcome> {
            self.installer_seen_all_stopped = self
                .running_ports
                .iter()
                .all(|port| self.stopped_ports.contains(port));
            self.invocations.push(invocation.clone());
            self.events.push("installer".to_string());
            if self.fail_installer {
                return Err(RefineError::Conflict("installer failed".to_string()));
            }
            Ok(InstallerOutcome {
                succeeded: self.installer_succeeded,
                status: self.installer_status,
                target_version: Some("9.8.7".to_string()),
                binary_path: Some(invocation.cwd.join("bin/refine")),
                stdout: String::new(),
                stderr: self.installer_stderr.clone(),
            })
        }

        fn verify_binary_mode(
            &mut self,
            _checkout: &Path,
            _binary_path: &Path,
        ) -> RefineResult<()> {
            if self.fail_verify {
                Err(RefineError::Conflict(
                    "binary mode check failed".to_string(),
                ))
            } else {
                Ok(())
            }
        }

        fn restart_port(&mut self, port: u16) -> RefineResult<DaemonStatus> {
            if self.fail_restart {
                return Err(RefineError::Conflict("restart failed".to_string()));
            }
            self.events.push(format!("restart:{port}"));
            self.restarted_ports.push(port);
            Ok(DaemonStatus {
                port,
                daemon_healthy: true,
                web_available: true,
                worker_state: "idle".to_string(),
                target_app_state: "unknown".to_string(),
                launch_mode: "binary".to_string(),
                executable_path: Some("/tmp/refine/bin/refine".to_string()),
                active_operations: Vec::new(),
                degraded_integrations: Vec::new(),
            })
        }
    }

    #[test]
    fn deployed_update_stops_ports_before_invoking_installer_and_restarts_them() {
        let mut host = FakeUpdateHost {
            running_ports: vec![8080, 9090],
            verify_stopped: true,
            installer_succeeded: true,
            installer_status: Some(0),
            ..Default::default()
        };
        let summary = run_deployed_update(
            &mut host,
            DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run").with_assume_yes(true),
        );

        assert!(summary.ok);
        assert!(host.installer_seen_all_stopped);
        assert_eq!(summary.stopped_ports, vec![8080, 9090]);
        assert_eq!(summary.restarted_ports, vec![8080, 9090]);
        assert_eq!(
            host.events,
            vec![
                "stop:8080",
                "stop:9090",
                "installer",
                "restart:8080",
                "restart:9090"
            ]
        );
        assert_eq!(summary.target_version.as_deref(), Some("9.8.7"));
        assert_eq!(host.invocations.len(), 1);
        assert_eq!(host.invocations[0].args, vec!["--yes", "--upgrade"]);
        assert!(host.invocations[0].env.contains(&(
            "REFINE_INSTALL_ASSUME_DEFAULTS".to_string(),
            "1".to_string()
        )));
        assert_eq!(
            summary.installer.as_ref().unwrap().command,
            vec!["/tmp/refine/scripts/install.sh", "--yes", "--upgrade"]
        );
        assert!(
            host.invocations[0]
                .env
                .contains(&("REFINE_INSTALL_UPDATE_ONLY".to_string(), "1".to_string()))
        );
    }

    #[test]
    fn deployed_update_leaves_installer_interactive_without_assume_yes() {
        let mut host = FakeUpdateHost {
            running_ports: Vec::new(),
            verify_stopped: true,
            installer_succeeded: true,
            installer_status: Some(0),
            ..Default::default()
        };
        let summary = run_deployed_update(
            &mut host,
            DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
        );

        assert!(summary.ok);
        assert_eq!(host.invocations.len(), 1);
        assert_eq!(host.invocations[0].args, vec!["--upgrade"]);
        assert!(!host.invocations[0].env.contains(&(
            "REFINE_INSTALL_ASSUME_DEFAULTS".to_string(),
            "1".to_string()
        )));
        assert_eq!(
            summary.installer.as_ref().unwrap().command,
            vec!["/tmp/refine/scripts/install.sh", "--upgrade"]
        );
    }

    #[test]
    fn deployed_update_does_not_run_installer_until_ports_verify_stopped() {
        let mut host = FakeUpdateHost {
            running_ports: vec![8080],
            verify_stopped: false,
            installer_succeeded: true,
            installer_status: Some(0),
            ..Default::default()
        };
        let summary = run_deployed_update(
            &mut host,
            DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
        );

        assert!(!summary.ok);
        assert!(host.invocations.is_empty());
        assert_eq!(summary.failures[0].stage, "verify_stopped");
    }

    #[test]
    fn deployed_update_does_not_restart_or_report_success_when_installer_fails() {
        let mut host = FakeUpdateHost {
            running_ports: vec![8080],
            fail_installer: true,
            verify_stopped: true,
            installer_succeeded: true,
            installer_status: Some(0),
            ..Default::default()
        };
        let summary = run_deployed_update(
            &mut host,
            DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
        );

        assert!(!summary.ok);
        assert_eq!(summary.stopped_ports, vec![8080]);
        assert!(summary.restarted_ports.is_empty());
        assert_eq!(summary.failures[0].stage, "installer");
        assert!(
            summary
                .manual_recovery_command
                .as_deref()
                .unwrap()
                .contains("scripts/install.sh --upgrade")
        );
    }

    #[test]
    fn deployed_update_reports_failed_installer_status_and_output() {
        let mut host = FakeUpdateHost {
            running_ports: vec![8080],
            verify_stopped: true,
            installer_succeeded: false,
            installer_status: Some(42),
            installer_stderr: "build failed".to_string(),
            ..Default::default()
        };
        let summary = run_deployed_update(
            &mut host,
            DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
        );

        assert!(!summary.ok);
        assert_eq!(summary.failures[0].stage, "installer");
        assert!(summary.failures[0].message.contains("42"));
        assert!(summary.failures[0].message.contains("build failed"));
        assert_eq!(summary.installer.as_ref().unwrap().status, Some(42));
        assert!(summary.restarted_ports.is_empty());
    }

    #[test]
    fn deployed_update_reports_after_installer_failures_without_false_success() {
        let mut host = FakeUpdateHost {
            running_ports: vec![8080],
            fail_verify: true,
            verify_stopped: true,
            installer_succeeded: true,
            installer_status: Some(0),
            ..Default::default()
        };
        let summary = run_deployed_update(
            &mut host,
            DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
        );

        assert!(!summary.ok);
        assert_eq!(summary.failures[0].stage, "verify_binary_mode");
        assert!(summary.rollback_possible);
        assert!(summary.restarted_ports.is_empty());
    }

    #[test]
    fn deployed_update_reports_restart_failures_after_binary_replacement() {
        let mut host = FakeUpdateHost {
            running_ports: vec![8080],
            fail_restart: true,
            verify_stopped: true,
            installer_succeeded: true,
            installer_status: Some(0),
            ..Default::default()
        };
        let summary = run_deployed_update(
            &mut host,
            DeployedUpdateOptions::new("/tmp/refine", "/tmp/refine/run"),
        );

        assert!(!summary.ok);
        assert_eq!(summary.failures[0].stage, "restart_ports");
        assert!(summary.rollback_possible);
        assert!(summary.restarted_ports.is_empty());
    }
}

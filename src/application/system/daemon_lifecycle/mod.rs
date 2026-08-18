#[cfg(test)]
use std::time::Duration;

use crate::application::system::installation::{FileInstallationService, InstalledServiceAction};
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::lifecycle::{
    BackgroundDaemonConfig, DaemonReachability, DaemonStatus, FileDaemonLifecycleService,
    ReadinessProgress, http_reachability_probe,
};
use crate::infrastructure::process::supervisor::runtime::{RuntimePathInputs, RuntimeRoot};

#[cfg(test)]
mod authority_tests;
mod direct;
mod handoff;
mod identity;
mod managed;
#[cfg(test)]
mod managed_tests;
mod operations;
#[cfg(test)]
mod operations_tests;

use direct::stop_direct_daemon;
pub(crate) use handoff::{
    HandoffLaunchReceipt, HandoffObservation, HostRestartSafeHandoffLauncher, RestartSafeHandoff,
    RestartSafeHandoffLauncher, handoff_argument_fingerprint, handoff_cwd, handoff_mechanism,
    handoff_mechanism_identity,
};
pub(crate) use identity::live_daemon_executable;
use managed::{run_service_managed_daemon, stop_service_managed_daemon};
#[cfg(test)]
pub(crate) use managed::{run_service_managed_daemon_with, stop_service_managed_daemon_with};
pub use operations::{
    DAEMON_LIFECYCLE_OPERATIONS_DIR, DaemonLifecycleOperation, FileDaemonLifecycleOperationService,
};

/// The authoritative host daemon lifecycle capability.
///
/// Surfaces provide target/runtime context and request an action. This service
/// alone selects installed service-manager behavior or direct managed-process
/// fallback, including confirmation, durable evidence, and recovery semantics.
pub trait HostDaemonLifecycleService {
    fn start(&self, config: BackgroundDaemonConfig) -> RefineResult<DaemonStatus>;
    fn stop(&self, port: u16) -> RefineResult<DaemonStatus>;
    fn restart(&self, config: BackgroundDaemonConfig) -> RefineResult<DaemonStatus>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonLifecycleAction {
    Start,
    Stop,
    Restart,
}

pub fn execute_daemon_lifecycle(
    service: &impl HostDaemonLifecycleService,
    action: DaemonLifecycleAction,
    config: BackgroundDaemonConfig,
) -> RefineResult<DaemonStatus> {
    match action {
        DaemonLifecycleAction::Start => service.start(config),
        DaemonLifecycleAction::Stop => service.stop(config.port),
        DaemonLifecycleAction::Restart => service.restart(config),
    }
}

/// Stop and confirm the exact daemon before removing its installation.
///
/// The installation remains intact when shutdown fails, so an operator can
/// inspect or retry the registered service instead of being left with a live,
/// unmanaged daemon and a false uninstall success.
pub fn uninstall_daemon_installation(
    lifecycle: &impl HostDaemonLifecycleService,
    installation: &FileInstallationService,
    port: u16,
) -> RefineResult<()> {
    lifecycle.stop(port)?;
    installation.uninstall_after_daemon_stopped()
}

#[derive(Clone, Debug)]
pub struct FileHostDaemonLifecycleService {
    runtime_root: RuntimeRoot,
    current_version: String,
    path_inputs: RuntimePathInputs,
}

impl FileHostDaemonLifecycleService {
    pub fn new(runtime_root: RuntimeRoot, current_version: impl Into<String>) -> Self {
        Self {
            runtime_root,
            current_version: current_version.into(),
            path_inputs: RuntimePathInputs::from_env(),
        }
    }

    pub fn with_path_inputs(
        runtime_root: RuntimeRoot,
        current_version: impl Into<String>,
        path_inputs: RuntimePathInputs,
    ) -> Self {
        Self {
            runtime_root,
            current_version: current_version.into(),
            path_inputs,
        }
    }

    fn runtime_lifecycle(&self) -> FileDaemonLifecycleService {
        FileDaemonLifecycleService::new(self.runtime_root.clone())
    }

    fn installation(&self, port: u16) -> FileInstallationService {
        FileInstallationService::with_path_inputs_for_port(
            &self.runtime_root.root,
            &self.current_version,
            port,
            self.path_inputs.clone(),
        )
    }

    fn validate_managed_start_config(
        &self,
        config: &BackgroundDaemonConfig,
        service_manager: &str,
    ) -> RefineResult<()> {
        if config.bind_address != std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
            || config.cache_dir.is_some()
            || config.static_root.is_some()
        {
            return Err(RefineError::InvalidInput(format!(
                "the activated {service_manager} installation owns daemon configuration; use the installed defaults or pass --foreground for an explicit in-process start"
            )));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn exercise_with(
        &self,
        config: BackgroundDaemonConfig,
        action: InstalledServiceAction,
        timeout: Duration,
        poll_interval: Duration,
        mut control: impl FnMut(&FileInstallationService, InstalledServiceAction) -> RefineResult<()>,
        probe: impl FnMut(u16) -> DaemonReachability,
    ) -> RefineResult<DaemonStatus> {
        let port = config.port;
        let lifecycle = self.runtime_lifecycle();
        let installation = self.installation(port);
        let service_manager = installation
            .installed_service_manager_for(action)?
            .ok_or_else(|| {
                RefineError::Conflict("test requires an installed service".to_string())
            })?;
        match action {
            InstalledServiceAction::Stop => stop_service_managed_daemon_with(
                &lifecycle,
                port,
                &service_manager,
                timeout,
                poll_interval,
                || control(&installation, action),
                probe,
            ),
            InstalledServiceAction::Start | InstalledServiceAction::Restart => {
                self.validate_managed_start_config(&config, &service_manager)?;
                run_service_managed_daemon_with(
                    &lifecycle,
                    port,
                    &service_manager,
                    action,
                    timeout,
                    poll_interval,
                    || control(&installation, action),
                    probe,
                )
            }
        }
    }
}

impl HostDaemonLifecycleService for FileHostDaemonLifecycleService {
    fn start(&self, config: BackgroundDaemonConfig) -> RefineResult<DaemonStatus> {
        let port = config.port;
        let lifecycle = self.runtime_lifecycle();
        let installation = self.installation(port);
        if let Some(service_manager) = installation.installed_service_manager()? {
            self.validate_managed_start_config(&config, &service_manager)?;
            return run_service_managed_daemon(
                &lifecycle,
                port,
                &service_manager,
                InstalledServiceAction::Start,
                || {
                    installation
                        .control_installed_service(InstalledServiceAction::Start)?
                        .ok_or_else(|| {
                            RefineError::Conflict(format!(
                                "activated {service_manager} installation disappeared while starting"
                            ))
                        })
                        .map(|_| ())
                },
            );
        }
        if http_reachability_probe(port) == DaemonReachability::Reachable {
            return lifecycle.mark_observed_ready(port);
        }
        lifecycle.start_background_daemon(config)
    }

    fn stop(&self, port: u16) -> RefineResult<DaemonStatus> {
        let lifecycle = self.runtime_lifecycle();
        let installation = self.installation(port);
        if let Some(service_manager) =
            installation.installed_service_manager_for(InstalledServiceAction::Stop)?
        {
            return stop_service_managed_daemon(&lifecycle, port, &service_manager, || {
                installation
                    .control_installed_service(InstalledServiceAction::Stop)?
                    .ok_or_else(|| {
                        RefineError::Conflict(format!(
                            "registered {service_manager} installation disappeared while stopping"
                        ))
                    })
                    .map(|_| ())
            });
        }
        stop_direct_daemon(&lifecycle, port)
    }

    fn restart(&self, config: BackgroundDaemonConfig) -> RefineResult<DaemonStatus> {
        let port = config.port;
        let lifecycle = self.runtime_lifecycle();
        let installation = self.installation(port);
        if let Some(service_manager) = installation.installed_service_manager()? {
            self.validate_managed_start_config(&config, &service_manager)?;
            return run_service_managed_daemon(
                &lifecycle,
                port,
                &service_manager,
                InstalledServiceAction::Restart,
                || {
                    installation
                        .control_installed_service(InstalledServiceAction::Restart)?
                        .ok_or_else(|| {
                            RefineError::Conflict(format!(
                                "activated {service_manager} installation disappeared while restarting"
                            ))
                        })
                        .map(|_| ())
                },
            );
        }
        stop_direct_daemon(&lifecycle, port)?;
        lifecycle.start_background_daemon(config)
    }
}

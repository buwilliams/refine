use super::os_backend::ServiceCommandOutput;
use super::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstalledServiceAction {
    Start,
    Stop,
    Restart,
}

impl InstalledServiceAction {
    fn as_systemd_verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServiceManagerControl {
    pub service_manager: String,
    pub action: InstalledServiceAction,
    pub commands: Vec<String>,
}

impl FileInstallationService {
    pub fn installed_service_manager(&self) -> RefineResult<Option<String>> {
        self.installed_service_manager_for(InstalledServiceAction::Start)
    }

    pub fn installed_service_manager_for(
        &self,
        action: InstalledServiceAction,
    ) -> RefineResult<Option<String>> {
        Ok(self
            .service_manager_backend(action)?
            .map(|backend| backend.service_manager))
    }

    pub fn control_installed_service(
        &self,
        action: InstalledServiceAction,
    ) -> RefineResult<Option<ServiceManagerControl>> {
        self.control_installed_service_with(action, &mut |command| {
            self.execute_service_command(command)
        })
    }

    pub(super) fn control_installed_service_with(
        &self,
        action: InstalledServiceAction,
        run: &mut impl FnMut(&ServiceCommand) -> Result<ServiceCommandOutput, String>,
    ) -> RefineResult<Option<ServiceManagerControl>> {
        let Some(backend) = self.service_manager_backend(action)? else {
            return Ok(None);
        };
        let commands = match backend.target {
            InstallTarget::LinuxCliWeb => {
                self.control_systemd_service_with(&backend, action, run)?
            }
            InstallTarget::MacOsAppBundle => {
                self.control_launchd_service_with(&backend, action, run)?
            }
            InstallTarget::WindowsInstaller => return Ok(None),
        };
        Ok(Some(ServiceManagerControl {
            service_manager: backend.service_manager,
            action,
            commands,
        }))
    }

    fn service_manager_backend(
        &self,
        _action: InstalledServiceAction,
    ) -> RefineResult<Option<InstallBackendRegistration>> {
        let status = self.status()?;
        Ok(status.backend.filter(|backend| {
            status.installed
                && backend.registered
                && backend.activated
                && matches!(
                    backend.target,
                    InstallTarget::LinuxCliWeb | InstallTarget::MacOsAppBundle
                )
        }))
    }

    fn control_systemd_service_with(
        &self,
        backend: &InstallBackendRegistration,
        action: InstalledServiceAction,
        run: &mut impl FnMut(&ServiceCommand) -> Result<ServiceCommandOutput, String>,
    ) -> RefineResult<Vec<String>> {
        let command = ServiceCommand::new(
            "systemctl",
            vec![
                "--user".to_string(),
                action.as_systemd_verb().to_string(),
                systemd_unit_name(backend),
            ],
        );
        run_checked(run, &command).map_err(|error| {
            RefineError::Degraded(format!(
                "failed to {} installed Refine systemd service with `{}`: {error}",
                action.as_systemd_verb(),
                command.display()
            ))
        })?;
        Ok(vec![command.display()])
    }

    fn control_launchd_service_with(
        &self,
        backend: &InstallBackendRegistration,
        action: InstalledServiceAction,
        run: &mut impl FnMut(&ServiceCommand) -> Result<ServiceCommandOutput, String>,
    ) -> RefineResult<Vec<String>> {
        let target = launchd_service_target(backend);
        let query = ServiceCommand::new("launchctl", vec!["print".to_string(), target.clone()]);
        let mut commands = vec![query.display()];
        let query_result = run(&query)
            .and_then(|output| classify_launchd_query(&output))
            .map_err(|error| format!("`{}` failed: {error}", query.display()));

        if action == InstalledServiceAction::Stop {
            return self.stop_launchd_service(run, backend, target, commands, query_result);
        }

        let loaded = query_result.map_err(|error| {
            RefineError::Degraded(format!(
                "failed to query installed Refine launchd service before {}: {error}",
                action.as_systemd_verb()
            ))
        })?;
        let mut lifecycle_commands =
            self.legacy_launchd_migration_commands(backend, run, &mut commands)?;
        lifecycle_commands.extend(match loaded {
            LaunchdQuery::Loaded => vec![ServiceCommand::new(
                "launchctl",
                match action {
                    InstalledServiceAction::Start => {
                        vec!["kickstart".to_string(), target]
                    }
                    InstalledServiceAction::Restart => {
                        vec!["kickstart".to_string(), "-k".to_string(), target]
                    }
                    InstalledServiceAction::Stop => unreachable!(),
                },
            )],
            LaunchdQuery::NotLoaded => {
                let plist = backend.service_metadata_path.clone().ok_or_else(|| {
                    RefineError::Conflict(
                        "installed Refine launchd service has no plist path".to_string(),
                    )
                })?;
                vec![
                    ServiceCommand::new("launchctl", vec!["enable".to_string(), target]),
                    ServiceCommand::new(
                        "launchctl",
                        vec!["bootstrap".to_string(), launchctl_gui_domain(), plist],
                    ),
                ]
            }
        });

        for command in lifecycle_commands {
            commands.push(command.display());
            run_checked(run, &command).map_err(|error| {
                RefineError::Degraded(format!(
                    "failed to {} installed Refine launchd service with `{}`: {error}",
                    action.as_systemd_verb(),
                    command.display()
                ))
            })?;
        }
        Ok(commands)
    }

    fn stop_launchd_service(
        &self,
        run: &mut impl FnMut(&ServiceCommand) -> Result<ServiceCommandOutput, String>,
        backend: &InstallBackendRegistration,
        target: String,
        mut commands: Vec<String>,
        query_result: Result<LaunchdQuery, String>,
    ) -> RefineResult<Vec<String>> {
        let mut failures = Vec::new();
        stop_launchd_target(
            run,
            &target,
            query_result,
            true,
            &mut commands,
            &mut failures,
        );

        if let Some(legacy_label) = backend.legacy_service_label.as_deref() {
            let legacy_target = format!("{}/{}", launchctl_gui_domain(), legacy_label);
            let legacy_query = ServiceCommand::new(
                "launchctl",
                vec!["print".to_string(), legacy_target.clone()],
            );
            commands.push(legacy_query.display());
            match run(&legacy_query) {
                Ok(output) => match classify_launchd_query(&output) {
                    Ok(LaunchdQuery::Loaded)
                        if launchd_registration_matches_backend(&output, backend) =>
                    {
                        stop_launchd_target(
                            run,
                            &legacy_target,
                            Ok(LaunchdQuery::Loaded),
                            false,
                            &mut commands,
                            &mut failures,
                        );
                    }
                    Ok(_) => {}
                    Err(error) => failures.push(format!(
                        "legacy service query failed: `{}` failed: {error}",
                        legacy_query.display()
                    )),
                },
                Err(error) => failures.push(format!(
                    "legacy service query failed: `{}` failed: {error}",
                    legacy_query.display()
                )),
            }
        }

        if failures.is_empty() {
            Ok(commands)
        } else {
            Err(RefineError::Degraded(format!(
                "failed to stop installed Refine launchd service: {}",
                failures.join("; ")
            )))
        }
    }

    fn legacy_launchd_migration_commands(
        &self,
        backend: &InstallBackendRegistration,
        run: &mut impl FnMut(&ServiceCommand) -> Result<ServiceCommandOutput, String>,
        commands: &mut Vec<String>,
    ) -> RefineResult<Vec<ServiceCommand>> {
        let Some(legacy_label) = backend.legacy_service_label.as_deref() else {
            return Ok(Vec::new());
        };
        let legacy_target = format!("{}/{}", launchctl_gui_domain(), legacy_label);
        let legacy_query = ServiceCommand::new(
            "launchctl",
            vec!["print".to_string(), legacy_target.clone()],
        );
        commands.push(legacy_query.display());
        let output = run(&legacy_query).map_err(|error| {
            RefineError::Degraded(format!(
                "failed to query legacy Refine launchd service before migration with `{}`: {error}",
                legacy_query.display()
            ))
        })?;
        match classify_launchd_query(&output).map_err(|error| {
            RefineError::Degraded(format!(
                "failed to query legacy Refine launchd service before migration with `{}`: {error}",
                legacy_query.display()
            ))
        })? {
            LaunchdQuery::NotLoaded => Ok(Vec::new()),
            LaunchdQuery::Loaded if !launchd_registration_matches_backend(&output, backend) => {
                Ok(Vec::new())
            }
            LaunchdQuery::Loaded => Ok(vec![
                ServiceCommand::new(
                    "launchctl",
                    vec!["disable".to_string(), legacy_target.clone()],
                ),
                ServiceCommand::new("launchctl", vec!["bootout".to_string(), legacy_target]),
            ]),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LaunchdQuery {
    Loaded,
    NotLoaded,
}

fn classify_launchd_query(output: &ServiceCommandOutput) -> Result<LaunchdQuery, String> {
    if output.succeeded() {
        return Ok(LaunchdQuery::Loaded);
    }
    let detail = output.failure_detail();
    let normalized = detail.to_ascii_lowercase();
    if normalized.contains("could not find service") && normalized.contains("in domain for") {
        Ok(LaunchdQuery::NotLoaded)
    } else {
        Err(detail)
    }
}

fn run_checked(
    run: &mut impl FnMut(&ServiceCommand) -> Result<ServiceCommandOutput, String>,
    command: &ServiceCommand,
) -> Result<(), String> {
    run(command)?.require_success()
}

fn stop_launchd_target(
    run: &mut impl FnMut(&ServiceCommand) -> Result<ServiceCommandOutput, String>,
    target: &str,
    query_result: Result<LaunchdQuery, String>,
    disable_when_not_loaded: bool,
    commands: &mut Vec<String>,
    failures: &mut Vec<String>,
) {
    if let Err(error) = &query_result {
        failures.push(format!("service query failed: {error}"));
    }

    if disable_when_not_loaded || query_result != Ok(LaunchdQuery::NotLoaded) {
        let disable =
            ServiceCommand::new("launchctl", vec!["disable".to_string(), target.to_string()]);
        commands.push(disable.display());
        if let Err(error) = run_checked(run, &disable) {
            failures.push(format!("`{}` failed: {error}", disable.display()));
        }
    }

    if query_result != Ok(LaunchdQuery::NotLoaded) {
        let bootout =
            ServiceCommand::new("launchctl", vec!["bootout".to_string(), target.to_string()]);
        commands.push(bootout.display());
        if let Err(error) = run_checked(run, &bootout) {
            failures.push(format!("`{}` failed: {error}", bootout.display()));
        }
    }
}

fn launchd_registration_matches_backend(
    output: &ServiceCommandOutput,
    backend: &InstallBackendRegistration,
) -> bool {
    backend
        .service_metadata_path
        .as_deref()
        .is_some_and(|path| {
            launchd_output_reports_path(&output.stdout, path)
                || launchd_output_reports_path(&output.stderr, path)
        })
}

fn launchd_output_reports_path(output: &str, expected_path: &str) -> bool {
    output.lines().any(|line| {
        line.split_once('=')
            .map(|(_, value)| value.trim().trim_matches('"') == expected_path)
            .unwrap_or(false)
    })
}

pub(super) fn systemd_unit_name(backend: &InstallBackendRegistration) -> String {
    backend
        .service_metadata_path
        .as_deref()
        .and_then(|path| PathBuf::from(path).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.to_str().map(str::to_string))
        .unwrap_or_else(|| "refine.service".to_string())
}

pub(super) fn launchd_label(backend: &InstallBackendRegistration) -> String {
    backend
        .port
        .map(|port| format!("com.refine.daemon.{port}"))
        .unwrap_or_else(|| "com.refine.daemon".to_string())
}

fn launchd_service_target(backend: &InstallBackendRegistration) -> String {
    format!("{}/{}", launchctl_gui_domain(), launchd_label(backend))
}

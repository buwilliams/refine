use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::System {
            action: SystemAction::ApiGroups,
        } => {
            let groups: Vec<_> = API_GROUPS
                .iter()
                .map(|group| json!({"prefix": group.prefix, "capability": group.capability}))
                .collect();
            println!("{}", serde_json::to_string_pretty(&groups).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Install {
                    port,
                    target,
                    runtime_root,
                    version,
                },
        } => {
            let runtime_root = resolve_system_runtime_root(runtime_root)?;
            let status = FileInstallationService::for_port(runtime_root, version, port)
                .install(target.into_target())?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Performance {
                    operation,
                    failures,
                    limit,
                    json,
                },
        } => {
            let mut path = format!("/performance?limit={}&offset=0", limit.clamp(1, 1000));
            if let Some(operation) = operation.as_deref().filter(|value| !value.is_empty()) {
                path.push_str(&format!("&operation={operation}"));
            }
            if failures {
                path.push_str("&success=false");
            }
            let report = daemon_json("GET", &path, None)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report).unwrap());
                return Ok(());
            }
            print_performance_report(&report);
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Repair {
                    port,
                    runtime_root,
                    version,
                },
        } => {
            let runtime_root = resolve_system_runtime_root(runtime_root)?;
            let status = FileInstallationService::for_port(runtime_root, version, port).repair()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Update {
                    yes,
                    provider,
                    port,
                    runtime_root,
                },
        } => {
            let paths = RefineCheckoutPaths::discover()?;
            let runtime_root = resolve_system_runtime_root_for_paths(runtime_root, &paths)?;
            let checkout_path = paths.checkout;
            let provider = resolve_agent_provider(&runtime_root, provider)?;
            let mut host =
                FileDeployedUpdateHost::with_controlling_port(runtime_root.clone(), port);
            let summary = run_deployed_update(
                &mut host,
                DeployedUpdateOptions::new(checkout_path, runtime_root)
                    .with_assume_yes(yes)
                    .with_provider(provider)
                    .with_controlling_port(port),
            );
            print_json(&serde_json::to_value(&summary).unwrap());
            if !summary.ok {
                return Err(RefineError::Degraded(
                    "system update failed; see JSON summary above".to_string(),
                ));
            }
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::ReleasePlan {
                    bump,
                    repo_root,
                    runtime_root,
                },
        } => {
            let service = FileReleaseService::new(
                absolute_cli_path(repo_root)?,
                resolve_system_runtime_root(runtime_root)?,
            );
            let plan = service.plan(ReleaseBump::parse(&bump)?)?;
            print_json(&serde_json::to_value(plan).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::ReleasePrepare {
                    bump,
                    repo_root,
                    runtime_root,
                },
        } => {
            let service = FileReleaseService::new(
                absolute_cli_path(repo_root)?,
                resolve_system_runtime_root(runtime_root)?,
            );
            let operation = service.prepare_blocking(ReleaseBump::parse(&bump)?)?;
            print_json(&serde_json::to_value(operation).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::ReleasePublish {
                    preparation_id,
                    confirm,
                    repo_root,
                    runtime_root,
                },
        } => {
            let service = FileReleaseService::new(
                absolute_cli_path(repo_root)?,
                resolve_system_runtime_root(runtime_root)?,
            );
            let operation = service.publish_blocking(&preparation_id, confirm)?;
            print_json(&serde_json::to_value(operation).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::SourceStatus {
                    checkout,
                    fetch,
                    port,
                    runtime_root,
                },
        } => {
            let paths = RefineCheckoutPaths::discover()?;
            let runtime_root = resolve_system_runtime_root_for_paths(runtime_root, &paths)?;
            let checkout = paths
                .require_same_source_checkout(checkout.as_deref().unwrap_or(&paths.checkout))?;
            let service = FileSourcePromotionService::new(
                checkout,
                RuntimeRoot { root: runtime_root }.port_root(port),
                port,
            );
            let status = service.inspect(fetch)?;
            print_json(&serde_json::to_value(status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::SourcePromote {
                    checkout,
                    port,
                    runtime_root,
                },
        } => {
            let paths = RefineCheckoutPaths::discover()?;
            let runtime_root = resolve_system_runtime_root_for_paths(runtime_root, &paths)?;
            let checkout = paths
                .require_same_source_checkout(checkout.as_deref().unwrap_or(&paths.checkout))?;
            let service = FileSourcePromotionService::new(
                checkout,
                RuntimeRoot { root: runtime_root }.port_root(port),
                port,
            );
            let operation = service.queue()?;
            print_json(&json!({"operation": operation}));
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::SourcePromoteHelper {
                    checkout,
                    port_runtime_root,
                    port,
                    operation_id,
                    attempt_id,
                    claim_nonce,
                },
        } => {
            let (checkout, port_runtime_root) =
                validate_source_helper_paths(checkout, port_runtime_root, port)?;
            FileSourcePromotionService::new(checkout, port_runtime_root, port)
                .run_helper(&operation_id, attempt_id.as_deref(), claim_nonce.as_deref())
                .map(|_| ())
        }
        Commands::System {
            action:
                SystemAction::SourceUpgradeCapability {
                    checkout,
                    port_runtime_root,
                    port,
                    operation_id,
                    action,
                },
        } => {
            let (checkout, port_runtime_root) =
                validate_source_helper_paths(checkout, port_runtime_root, port)?;
            let result = FileSourcePromotionService::new(checkout, port_runtime_root, port)
                .run_agent_capability(&operation_id, &action)?;
            print_json(&result);
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::SourceCheckWorker {
                    checkout,
                    port_runtime_root,
                    port,
                    operation_id,
                },
        } => {
            let (checkout, port_runtime_root) =
                validate_source_helper_paths(checkout, port_runtime_root, port)?;
            FileSourcePromotionService::new(checkout, port_runtime_root, port)
                .run_update_check(&operation_id)
                .map(|_| ())
        }
        Commands::System {
            action:
                SystemAction::DaemonLifecycleHelper {
                    action,
                    port,
                    runtime_root,
                    operation_id,
                },
        } => {
            let action = match action.as_str() {
                "stop" => DaemonLifecycleAction::Stop,
                "restart" => DaemonLifecycleAction::Restart,
                other => {
                    return Err(RefineError::InvalidInput(format!(
                        "unsupported daemon lifecycle helper action {other}"
                    )));
                }
            };
            let runtime_root = resolve_system_runtime_root(runtime_root)?;
            crate::tools::host::daemon_lifecycle::FileDaemonLifecycleOperationService::new(
                RuntimeRoot { root: runtime_root },
                env!("CARGO_PKG_VERSION"),
            )
            .run_helper(
                &operation_id,
                action,
                BackgroundDaemonConfig {
                    port,
                    ..Default::default()
                },
            )
            .map(|_| ())
        }
        Commands::System {
            action:
                SystemAction::RunnerWorker {
                    kind,
                    port_runtime_root,
                    project_registry_root,
                    target_root,
                    operation_id,
                },
        } => {
            #[cfg(not(test))]
            let paths = RefineCheckoutPaths::discover()?;
            let port = port_runtime_root
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<u16>().ok())
                .ok_or_else(|| {
                    RefineError::InvalidInput(format!(
                        "runner port runtime {} does not end in a valid port",
                        port_runtime_root.display()
                    ))
                })?;
            #[cfg(test)]
            let _ = port;
            #[cfg(not(test))]
            let port_runtime_root = paths.require_port_runtime(&port_runtime_root, port)?;
            let project_registry_root = match project_registry_root {
                #[cfg(not(test))]
                Some(root) if root == paths.runtime_root => Some(root),
                #[cfg(not(test))]
                Some(root) => {
                    return Err(RefineError::Conflict(format!(
                        "runner project registry root {} does not match checkout-owned runtime {}",
                        root.display(),
                        paths.runtime_root.display()
                    )));
                }
                #[cfg(test)]
                Some(root) => Some(root),
                None => None,
            };
            run_worker(
                &kind,
                port_runtime_root,
                project_registry_root,
                target_root.map(absolute_cli_path).transpose()?,
                operation_id,
            )
        }
        Commands::System {
            action:
                SystemAction::Rollback {
                    port,
                    runtime_root,
                    version,
                },
        } => {
            let runtime_root = resolve_system_runtime_root(runtime_root)?;
            let status =
                FileInstallationService::for_port(runtime_root, version, port).rollback()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Uninstall {
                    port,
                    runtime_root,
                    version,
                },
        } => {
            let runtime_root = resolve_system_runtime_root(runtime_root)?;
            let lifecycle = FileHostDaemonLifecycleService::new(
                RuntimeRoot {
                    root: runtime_root.clone(),
                },
                env!("CARGO_PKG_VERSION"),
            );
            let installation = FileInstallationService::for_port(runtime_root, version, port);
            uninstall_daemon_installation(&lifecycle, &installation, port)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"uninstalled": true})).unwrap()
            );
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Doctor {
                    target_root,
                    runtime_root,
                    repo_root,
                },
        } => {
            let runtime_root = resolve_system_runtime_root(runtime_root)?;
            let repo_root = absolute_cli_path(repo_root)?;
            let report =
                FileDiagnosticsService::new(target_root, runtime_root, repo_root).doctor()?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Start {
                    port,
                    bind_address,
                    cache_dir,
                    static_root,
                    runtime_root,
                    once,
                    foreground,
                },
        } => run_system_start(
            port,
            bind_address,
            cache_dir,
            static_root,
            runtime_root,
            once,
            foreground,
        ),
        Commands::System {
            action: SystemAction::Stop { port, runtime_root },
        } => {
            let runtime_root = resolve_system_runtime_root(runtime_root)?;
            let lifecycle = FileHostDaemonLifecycleService::new(
                RuntimeRoot { root: runtime_root },
                env!("CARGO_PKG_VERSION"),
            );
            let status = execute_daemon_lifecycle(
                &lifecycle,
                DaemonLifecycleAction::Stop,
                BackgroundDaemonConfig {
                    port,
                    ..Default::default()
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Restart { port, runtime_root },
        } => {
            let runtime_root = resolve_system_runtime_root(runtime_root)?;
            let lifecycle = FileHostDaemonLifecycleService::new(
                RuntimeRoot { root: runtime_root },
                env!("CARGO_PKG_VERSION"),
            );
            let status = execute_daemon_lifecycle(
                &lifecycle,
                DaemonLifecycleAction::Restart,
                BackgroundDaemonConfig {
                    port,
                    ..Default::default()
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Status {
                    port: _,
                    runtime_root,
                },
        } => {
            print_json(&system_status_response(resolve_system_runtime_root(
                runtime_root,
            )?)?);
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Ps {
                    port,
                    runtime_root,
                    stop,
                    signal,
                },
        } => {
            print_json(&system_ps_response(
                resolve_system_runtime_root(runtime_root)?,
                port,
                stop.as_deref(),
                &signal,
            )?);
            Ok(())
        }
        _ => unreachable!("command family was routed incorrectly"),
    }
}

fn validate_source_helper_paths(
    checkout: PathBuf,
    port_runtime_root: PathBuf,
    port: u16,
) -> RefineResult<(PathBuf, PathBuf)> {
    let paths = RefineCheckoutPaths::discover()?;
    let checkout = paths.require_same_source_checkout(&checkout)?;
    let port_runtime_root = paths.require_port_runtime(&port_runtime_root, port)?;
    Ok((checkout, port_runtime_root))
}

pub(super) fn selected_process_ports(
    runtime: &RuntimeRoot,
    port: Option<u16>,
) -> RefineResult<Vec<u16>> {
    if let Some(port) = port {
        return Ok(vec![port]);
    }
    let ports = FileDaemonLifecycleService::new(runtime.clone())
        .known_statuses()?
        .into_iter()
        .map(|status| status.port)
        .collect::<Vec<_>>();
    if ports.is_empty() {
        Ok(vec![8082])
    } else {
        Ok(ports)
    }
}

pub(super) fn stop_system_process(
    runtime: &RuntimeRoot,
    port: Option<u16>,
    process_id: &str,
    signal: &str,
) -> RefineResult<serde_json::Value> {
    let ports = selected_process_ports(runtime, port)?;
    let mut misses = Vec::new();
    for port in ports {
        let port_root = runtime.port_root(port);
        let service = FileProcessControlService::new(&port_root);
        match service.stop(process_id, signal) {
            Ok(mut result) => {
                if let Some(object) = result.as_object_mut() {
                    object.insert("port".to_string(), json!(port));
                    object.insert(
                        "runtime_root".to_string(),
                        json!(port_root.display().to_string()),
                    );
                }
                return Ok(result);
            }
            Err(RefineError::NotFound(message)) => misses.push(message),
            Err(error) => return Err(error),
        }
    }
    Err(RefineError::NotFound(format!(
        "Process {process_id} was not found{}",
        if misses.is_empty() {
            String::new()
        } else {
            format!(" ({})", misses.join("; "))
        }
    )))
}

pub(super) fn port_status_with_processes(
    runtime: &RuntimeRoot,
    status: &DaemonStatus,
) -> serde_json::Value {
    let port_root = runtime.port_root(status.port);
    let process_summary = FileProcessStatusService::new(&port_root).summary();
    let mut value = serde_json::to_value(status).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "runtime_root".to_string(),
            json!(port_root.display().to_string()),
        );
        match process_summary {
            Ok(summary) => {
                let process_count = summary
                    .get("processes")
                    .and_then(|value| value.as_array())
                    .map(|processes| processes.len())
                    .unwrap_or(0);
                object.insert("process_count".to_string(), json!(process_count));
                object.insert("running_process_count".to_string(), json!(process_count));
                object.insert(
                    "processes".to_string(),
                    summary
                        .get("processes")
                        .and_then(|value| value.as_array())
                        .map(|processes| {
                            processes
                                .iter()
                                .map(minimal_status_process)
                                .collect::<Vec<_>>()
                        })
                        .map(Value::Array)
                        .unwrap_or_else(|| json!([])),
                );
            }
            Err(error) => {
                object.insert("process_count".to_string(), json!(0));
                object.insert("processes".to_string(), json!([]));
                object.insert("process_error".to_string(), json!(error.to_string()));
            }
        }
    }
    value
}

pub(super) fn minimal_status_process(process: &Value) -> Value {
    json!({
        "pid": process.get("pid").cloned().unwrap_or(Value::Null),
        "status": process.get("status").cloned().unwrap_or(Value::Null),
        "label": process.get("label").cloned().unwrap_or(Value::Null),
    })
}

/// Render the performance report as a readable table: this command exists to
/// answer "how is Refine performing right now" from a terminal.
fn print_performance_report(report: &Value) {
    let summary = report
        .get("summary")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = report
        .get("total_event_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    println!("Events in memory since daemon start: {total}");
    println!();
    if summary.is_empty() {
        println!("No performance events recorded yet.");
        return;
    }
    println!(
        "{:<24} {:>7} {:>9} {:>9} {:>9} {:>9}",
        "operation", "count", "failures", "avg ms", "p95 ms", "max ms"
    );
    for row in &summary {
        println!(
            "{:<24} {:>7} {:>9} {:>9.1} {:>9.1} {:>9.1}",
            row.get("operation").and_then(Value::as_str).unwrap_or("?"),
            row.get("count").and_then(Value::as_u64).unwrap_or(0),
            row.get("failures").and_then(Value::as_u64).unwrap_or(0),
            row.get("avg_ms").and_then(Value::as_f64).unwrap_or(0.0),
            row.get("p95_ms").and_then(Value::as_f64).unwrap_or(0.0),
            row.get("max_ms").and_then(Value::as_f64).unwrap_or(0.0),
        );
    }
    let mut recent = report
        .get("recent")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    recent.sort_by(|a, b| {
        let elapsed = |event: &Value| event.get("elapsed_ms").and_then(Value::as_f64).unwrap_or(0.0);
        elapsed(b).partial_cmp(&elapsed(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    if !recent.is_empty() {
        println!();
        println!("Slowest recent events:");
        for event in recent.iter().take(10) {
            let details = event.get("details").cloned().unwrap_or(Value::Null);
            let target = details
                .get("path")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| {
                    event
                        .get("operation")
                        .and_then(Value::as_str)
                        .unwrap_or("?")
                        .to_string()
                });
            println!(
                "{:>9.1}ms  {}  {}  {}",
                event.get("elapsed_ms").and_then(Value::as_f64).unwrap_or(0.0),
                if event.get("success").and_then(Value::as_bool).unwrap_or(true) {
                    "ok "
                } else {
                    "ERR"
                },
                event.get("occurred_at").and_then(Value::as_str).unwrap_or(""),
                target,
            );
        }
    }
}

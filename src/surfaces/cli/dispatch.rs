use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};

use clap::Parser;
use serde_json::{Value, json};

use crate::model::workflow::GapStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    BackgroundDaemonConfig, DaemonLifecycleService, DaemonStatus, FileDaemonLifecycleService,
    current_launch_executable, current_launch_mode, http_probe,
};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::surfaces::web_server::{
    API_CONTRACT_VERSION, API_GROUPS, InProcessWebServer, LocalHttpDaemon,
};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::cluster::{ClusterService, FileClusterService, NodeRemoteUpdate};
use crate::tools::host::deployed_update::{
    DeployedUpdateOptions, FileDeployedUpdateHost, discover_refine_checkout, run_deployed_update,
};
use crate::tools::host::fleet::FileFleetService;
use crate::tools::host::fleet::worker::{WorkerInitOptions, initialize_worker};
use crate::tools::host::installation::{FileInstallationService, InstallationService};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::tools::observability::processes::FileProcessStatusService;
use crate::tools::observability::support_bundle::{FileSupportBundleService, SupportBundleService};
use crate::tools::product::imports::FileImportService;
use crate::tools::product::next_actions::FileNextActionsService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_registry::{FileProjectRegistryService, ProjectRegistryService};
use crate::tools::product::project_state::{
    FileProjectStateStore, ProjectStateStore, ProjectionQuery, ProjectionSnapshot,
};
use crate::tools::product::work_items::FileWorkItemService;

use super::actions::*;
use super::helpers::*;

pub fn run() -> RefineResult<()> {
    let cli = Cli::parse();
    dispatch(cli)
}

pub fn dispatch(cli: Cli) -> RefineResult<()> {
    #[cfg(not(test))]
    if let Some(path) = explicit_target_root_path(&cli.command) {
        return Err(RefineError::InvalidInput(format!(
            "direct --target-root CLI dispatch is not supported in normal operation; use the daemon API for target state mutations instead ({})",
            path.display()
        )));
    }

    #[cfg(not(test))]
    let cli = match cli.command {
        Commands::Project { action } => return dispatch_project_daemon(action),
        Commands::Gap { action } => return dispatch_gap_daemon(action),
        Commands::Feature { action } => return dispatch_feature_daemon(action),
        Commands::Workflow { action } => return dispatch_workflow_daemon(action),
        Commands::Node { action } => return dispatch_node_daemon(action),
        Commands::Cluster { action } => return dispatch_cluster_daemon(action),
        Commands::Log { action } => return dispatch_log_daemon(action),
        Commands::Agent { action } => return dispatch_agent_daemon(action),
        Commands::System {
            action: SystemAction::Doctor { .. },
        } => {
            let response = daemon_json("GET", "/diagnostics", None)?;
            print_json(&response);
            return Ok(());
        }
        other => Cli { command: other },
    };

    match cli.command {
        Commands::Website {
            port,
            bind_address,
            static_root,
            once,
        } => run_website(port, bind_address, static_root, once),
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
            let status = FileInstallationService::for_port(runtime_root, version, port)
                .install(target.into_target())?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
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
            let status = FileInstallationService::for_port(runtime_root, version, port).repair()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Update { yes, runtime_root },
        } => {
            let runtime_root = absolute_cli_path(runtime_root)?;
            let checkout_path = discover_refine_checkout()?;
            let mut host = FileDeployedUpdateHost::new(runtime_root.clone());
            let summary = run_deployed_update(
                &mut host,
                DeployedUpdateOptions::new(checkout_path, runtime_root).with_assume_yes(yes),
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
                SystemAction::Rollback {
                    port,
                    runtime_root,
                    version,
                },
        } => {
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
            FileInstallationService::for_port(runtime_root, version, port).uninstall()?;
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
            let report =
                FileDiagnosticsService::new(target_root, runtime_root, repo_root).doctor()?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::List {
                target_root: Some(target_root),
            },
        } => {
            let nodes = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root))
                .list_response()?;
            println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Show {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let node =
                FileNodeRegistryService::new(refine_dir_for_target_root(&target_root)).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Create {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let node = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root))
                .create(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Activate {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let nodes = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root))
                .activate(&id)?;
            println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Archive {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let node = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root))
                .archive(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Rename {
                    id,
                    name,
                    target_root: Some(target_root),
                },
        } => {
            let node = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root))
                .rename(&id, &name)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Settings {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let settings = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root))
                .settings(&id)?;
            println!("{}", serde_json::to_string_pretty(&settings).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Transfer {
                    id,
                    item_id,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .transfer_item_to_node(&id, &item_id)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::List {
                    target_root: Some(target_root),
                },
        } => {
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .list_response()?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Show {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let node =
                FileClusterService::new(refine_dir_for_target_root(&target_root)).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::AddNode {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let cluster =
                FileClusterService::new(refine_dir_for_target_root(&target_root)).add_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::EditNode {
                    id,
                    display_name,
                    ssh_host,
                    ssh_user,
                    ssh_identity_path,
                    ssh_port,
                    refine_checkout,
                    target_app_path,
                    refine_port,
                    provider,
                    provisioning,
                    enabled,
                    target_root: Some(target_root),
                },
        } => {
            let provisioning = provisioning
                .as_deref()
                .map(parse_provisioning_object)
                .transpose()?;
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .upsert_node(
                    &id,
                    NodeRemoteUpdate {
                        display_name,
                        ssh_host,
                        ssh_user,
                        ssh_identity_path,
                        ssh_port: ssh_port.map(u64::from),
                        refine_checkout,
                        target_app_path,
                        refine_port: refine_port.map(u64::from),
                        provider,
                        provisioning,
                        enabled,
                    },
                )?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::EnableNode {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .set_enabled(&id, true)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::DisableNode {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .set_enabled(&id, false)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::RemoveNode {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .remove_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Bootstrap {
                    id,
                    dry_run,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .bootstrap_node_response(&id, dry_run)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Providers {
                    target_root: Some(target_root),
                },
        } => {
            let providers = FileFleetService::new(refine_dir_for_target_root(&target_root))
                .providers_response()?;
            println!("{}", serde_json::to_string_pretty(&providers).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Provision {
                    id,
                    provider,
                    dry_run,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileFleetService::new(refine_dir_for_target_root(&target_root))
                .provision_response(&id, provider.as_deref(), dry_run)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Deprovision {
                    id,
                    dry_run,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileFleetService::new(refine_dir_for_target_root(&target_root))
                .deprovision_response(&id, dry_run)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::ProvisionStatus {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileFleetService::new(refine_dir_for_target_root(&target_root))
                .provision_status_response(&id)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Distribute {
                    to,
                    converge,
                    dry_run,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .distribute_response(to.as_deref(), converge, dry_run)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Sync {
                    target_root: Some(target_root),
                },
        } => {
            let sync = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .sync_response()?;
            println!("{}", serde_json::to_string_pretty(&sync).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Run {
                    id,
                    command,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .run_remote(&id, &command)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Transfer {
                    id,
                    item_id,
                    target_root: Some(target_root),
                },
        } => {
            let service = FileClusterService::new(refine_dir_for_target_root(&target_root));
            service.transfer(&item_id, &id)?;
            let result = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .transfer_item_to_node(&id, &item_id)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Maintenance {
                    target_root: Some(target_root),
                },
        } => {
            let maintenance = FileClusterService::new(refine_dir_for_target_root(&target_root))
                .maintenance_response()?;
            println!("{}", serde_json::to_string_pretty(&maintenance).unwrap());
            Ok(())
        }
        Commands::Next {
            target_root: Some(target_root),
        } => {
            let next = FileNextActionsService::new(refine_dir_for_target_root(&target_root))
                .next_response()?;
            println!("{}", serde_json::to_string_pretty(&next).unwrap());
            Ok(())
        }
        Commands::Next { target_root: None } => {
            let next = daemon_json("GET", "/guidance/next", None)?;
            print_json(&next);
            Ok(())
        }
        Commands::Commands => {
            print_json(&super::catalog::commands_catalog());
            Ok(())
        }
        Commands::Log {
            action: LogAction::List { target_root, limit },
        } => {
            if skipped_target_root(&target_root) {
                let response = daemon_json("GET", &format!("/activity?limit={limit}"), None)?;
                print_json(&json!({
                    "entries": response.get("activity").cloned().unwrap_or_default()
                }));
                return Ok(());
            }
            let entries =
                FileActivityService::new(refine_dir_for_target_root(&target_root)).recent(limit)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action: LogAction::Tail { target_root, limit },
        } => {
            if skipped_target_root(&target_root) {
                let response = daemon_json("GET", &format!("/activity?limit={limit}"), None)?;
                print_json(&json!({
                    "entries": response.get("activity").cloned().unwrap_or_default(),
                    "tail": true
                }));
                return Ok(());
            }
            let entries =
                FileActivityService::new(refine_dir_for_target_root(&target_root)).recent(limit)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries, "tail": true})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action: LogAction::Show { id, target_root },
        } => {
            if skipped_target_root(&target_root) {
                let response = daemon_json("GET", "/activity?limit=1000", None)?;
                let Some(entry) = response
                    .get("activity")
                    .and_then(|value| value.as_array())
                    .and_then(|entries| {
                        entries.iter().find(|entry| {
                            entry.get("id").and_then(|value| value.as_str()) == Some(id.as_str())
                        })
                    })
                    .cloned()
                else {
                    return Err(RefineError::NotFound(format!(
                        "Log entry {id} was not found"
                    )));
                };
                print_json(&json!({ "entry": entry }));
                return Ok(());
            }
            let service = FileActivityService::new(refine_dir_for_target_root(&target_root));
            let limit = service.count()?.max(1);
            let Some(entry) = service
                .query(limit, 0, None, None, None, None, None)?
                .into_iter()
                .find(|entry| entry.id == id)
            else {
                return Err(crate::process::supervisor::errors::RefineError::NotFound(
                    format!("Log entry {id} was not found"),
                ));
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entry": entry})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::Query {
                    q,
                    target_root,
                    limit,
                    offset,
                    gap_id,
                    severity,
                    category,
                    actor,
                },
        } => {
            if skipped_target_root(&target_root) {
                let mut query = vec![
                    format!("limit={limit}"),
                    format!("offset={offset}"),
                    format!("q={}", query_component(&q)),
                ];
                if let Some(value) = gap_id {
                    query.push(format!("gap_id={}", query_component(&value)));
                }
                if let Some(value) = severity {
                    query.push(format!("severity={}", query_component(&value)));
                }
                if let Some(value) = category {
                    query.push(format!("category={}", query_component(&value)));
                }
                if let Some(value) = actor {
                    query.push(format!("actor={}", query_component(&value)));
                }
                let response = daemon_json("GET", &format!("/activity?{}", query.join("&")), None)?;
                print_json(&json!({
                    "entries": response.get("activity").cloned().unwrap_or_default()
                }));
                return Ok(());
            }
            let entries = FileActivityService::new(refine_dir_for_target_root(&target_root))
                .query(
                    limit,
                    offset,
                    gap_id.as_deref(),
                    severity.as_deref(),
                    category.as_deref(),
                    actor.as_deref(),
                    Some(&q),
                )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::Export {
                    target_root: Some(target_root),
                },
        } => {
            let service = FileActivityService::new(refine_dir_for_target_root(&target_root));
            let limit = service.count()?;
            let entries = if limit == 0 {
                Vec::new()
            } else {
                service.query(limit, 0, None, None, None, None, None)?
            };
            let exported = entries.len();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries, "exported": exported}))
                    .unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action: LogAction::Export { target_root: None },
        } => {
            let response = daemon_json("GET", "/activity?limit=1000", None)?;
            let entries = response.get("activity").cloned().unwrap_or_default();
            let exported = entries.as_array().map(Vec::len).unwrap_or_default();
            print_json(&json!({"entries": entries, "exported": exported}));
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::Bundle {
                    target_root,
                    runtime_root,
                    repo_root,
                    redact_secrets,
                },
        } => {
            if skipped_target_root(&target_root) {
                let response = daemon_json(
                    "POST",
                    "/diagnostics/support-bundle",
                    Some(json!({ "redact_secrets": redact_secrets })),
                )?;
                print_json(&response);
                return Ok(());
            }
            let bundle = FileSupportBundleService::new(
                refine_dir_for_target_root(&target_root),
                runtime_root,
                repo_root,
            )
            .export(redact_secrets)?;
            println!("{}", serde_json::to_string_pretty(&bundle).unwrap());
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Detect,
        } => {
            let providers = HostAgentProviderService::new().detect()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"providers": providers})).unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Configure { provider },
        } => {
            HostAgentProviderService::new().configure(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "configured": true})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Auth { provider },
        } => {
            HostAgentProviderService::new().authenticate(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "authenticated": true})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Diagnose { provider },
        } => {
            let diagnostics = HostAgentProviderService::new().diagnose(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "diagnostics": diagnostics})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action:
                AgentAction::Invoke {
                    prompt,
                    provider,
                    cwd,
                },
        } => {
            let output = HostAgentProviderService::new().invoke(ProviderInvocation {
                provider,
                prompt,
                session_id: None,
                cwd: cwd.map(|path| path.display().to_string()),
                process_metadata: Default::default(),
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"ok": true, "output": output})).unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action:
                AgentAction::Resume {
                    session_id,
                    provider,
                },
        } => {
            let output = HostAgentProviderService::new().resume(&provider, &session_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"ok": true, "output": output})).unwrap()
            );
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
            let status = FileDaemonLifecycleService::new(RuntimeRoot {
                root: runtime_root.clone(),
            })
            .stop(port)?;
            let _ = http_probe(port);
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Restart { port, runtime_root },
        } => {
            let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
                root: runtime_root.clone(),
            });
            let _ = lifecycle.stop(port)?;
            let _ = http_probe(port);
            let status = FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root })
                .start_background_daemon(BackgroundDaemonConfig {
                    port,
                    ..Default::default()
                })?;
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
            print_json(&system_status_response(runtime_root)?);
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
                runtime_root,
                port,
                stop.as_deref(),
                &signal,
            )?);
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Status {
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root).status()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Attach {
                    path,
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root)
                .attach_with_migration(&path)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Switch {
                    name,
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root)
                .switch_with_migration(&name)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Detach {
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root).detach()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Register {
                    name,
                    path,
                    runtime_root,
                    target_root,
                },
        } => {
            let registry = FileProjectRegistryService::new(runtime_root, target_root)
                .register_path(Some(&name), &path, false)?;
            println!("{}", serde_json::to_string_pretty(&registry).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Clone {
                    source,
                    destination,
                    name,
                    make_current,
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root).clone_app(
                &source,
                &destination,
                name.as_deref(),
                make_current,
            )?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Remove {
                    name,
                    runtime_root,
                    target_root,
                },
        } => {
            let registry =
                FileProjectRegistryService::new(runtime_root, target_root).remove(&name)?;
            println!("{}", serde_json::to_string_pretty(&registry).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Migrate {
                    target_root,
                    runtime_root,
                },
        } => {
            let report =
                FileProjectRegistryService::new(runtime_root, target_root).migrate_current()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::to_value(report).unwrap()).unwrap()
            );
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Doctor {
                    target_root,
                    runtime_root,
                    repo_root,
                },
        } => {
            let report =
                FileDiagnosticsService::new(target_root, runtime_root, repo_root).doctor()?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Sync {
                    target_root: Some(target_root),
                    cache_dir,
                },
        } => {
            let refine_dir = refine_dir_for_target_root(&target_root);
            let store = cache_dir
                .as_ref()
                .and_then(|cache_dir| cache_dir.parent())
                .map(|runtime_root| {
                    FileProjectStateStore::with_runtime_root(&refine_dir, runtime_root)
                })
                .unwrap_or_else(|| FileProjectStateStore::new(&refine_dir));
            let snapshot = store.rebuild_projection()?;
            if let Some(cache_dir) = &cache_dir {
                store.persist_projection_snapshot(&cache_dir, &snapshot)?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "gaps": snapshot.gaps.len(),
                    "features": snapshot.features.len(),
                    "source_fingerprints": snapshot.source_fingerprints.len(),
                    "status_counts": snapshot.status_counts(),
                    "cache_updated": cache_dir.is_some()
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Project {
            action: ProjectAction::Sync {
                target_root: None, ..
            },
        } => {
            let response = daemon_json("POST", "/cache/rebuild", None)?;
            print_json(&response);
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Create {
                    name,
                    target_root: Some(target_root),
                    id,
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .create_gap_summary(&name, id.as_deref())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action: GapAction::List {
                target_root: Some(target_root),
            },
        } => {
            let gaps: Vec<_> = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .list_gap_summaries()?
                .into_iter()
                .map(|gap| gap.gap)
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gaps": gaps})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Show {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .show_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Edit {
                    id,
                    target_root: Some(target_root),
                    name,
                    priority,
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .update_gap_metadata_summary(
                    &id,
                    name.as_deref(),
                    priority.as_deref(),
                    None,
                    None,
                )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Note {
                    id,
                    body,
                    target_root: Some(target_root),
                    author,
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .add_gap_note_summary(&id, &author, &body)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::NoteEdit {
                    id,
                    note_id,
                    body,
                    target_root: Some(target_root),
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root));
            let detail = service.show_gap_detail(&id)?;
            let notes = edit_gap_note_values(gap_notes_from_detail(&detail), &note_id, &body)?;
            let gap = service.replace_gap_notes_summary(&id, &notes)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::NoteDelete {
                    id,
                    note_id,
                    target_root: Some(target_root),
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root));
            let detail = service.show_gap_detail(&id)?;
            let notes = delete_gap_note_values(gap_notes_from_detail(&detail), &note_id)?;
            let gap = service.replace_gap_notes_summary(&id, &notes)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Round {
                    id,
                    target_root: Some(target_root),
                    reporter,
                    actual,
                    target,
                    edit_latest,
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root));
            let gap = if edit_latest {
                service.edit_latest_gap_round_summary(
                    &id,
                    reporter.as_deref(),
                    None,
                    actual.as_deref(),
                    target.as_deref(),
                )?
            } else {
                let Some(reporter) = reporter.as_deref() else {
                    return Err(
                        crate::process::supervisor::errors::RefineError::InvalidInput(
                            "round reporter is required".to_string(),
                        ),
                    );
                };
                let Some(actual) = actual.as_deref() else {
                    return Err(
                        crate::process::supervisor::errors::RefineError::InvalidInput(
                            "round actual is required".to_string(),
                        ),
                    );
                };
                let Some(target) = target.as_deref() else {
                    return Err(
                        crate::process::supervisor::errors::RefineError::InvalidInput(
                            "round target is required".to_string(),
                        ),
                    );
                };
                service.append_gap_round_summary(&id, reporter, actual, target)?
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Delete {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .delete_gap_record(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"deleted": true, "id": id})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Cancel {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .cancel_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Start {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .start_gap_workflow(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Retry {
                    id,
                    target_root: Some(target_root),
                    stage,
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root));
            let gap = match stage.as_str() {
                "quality" | "qa" => service.retry_gap_quality_summary(&id)?,
                "merge" => service.retry_gap_merge_summary(&id)?,
                _ => {
                    return Err(
                        crate::process::supervisor::errors::RefineError::InvalidInput(
                            "retry stage must be quality or merge".to_string(),
                        ),
                    );
                }
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Verify {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .verify_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Merge {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .merge_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::Undo {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let gap = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .undo_gap_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::AssignFeature {
                    id,
                    feature_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .assign_gap_to_feature(&feature_id, &id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::RemoveFeature {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root));
            let current = service.show_gap_summary(&id)?;
            let Some(feature_id) = current.gap.feature_id.clone() else {
                return Err(
                    crate::process::supervisor::errors::RefineError::InvalidInput(format!(
                        "Gap {id} is not assigned to a Feature"
                    )),
                );
            };
            let feature = service.remove_gap_from_feature(&feature_id, &id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Create {
                    name,
                    target_root: Some(target_root),
                    id,
                    description,
                    reporter,
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .create_feature_summary(
                    &name,
                    id.as_deref(),
                    description.as_deref(),
                    reporter.as_deref(),
                    None,
                )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Edit {
                    id,
                    target_root: Some(target_root),
                    name,
                    description,
                    reporter,
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .update_feature_metadata_summary(
                    &id,
                    name.as_deref(),
                    description.as_deref(),
                    reporter.as_deref(),
                    None,
                )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::List {
                    target_root: Some(target_root),
                },
        } => {
            let features: Vec<_> =
                FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                    .list_feature_summaries()?
                    .into_iter()
                    .map(|feature| {
                        json!({
                            "feature": feature.feature,
                            "gap_ids": feature.gap_ids,
                            "rollup": feature.rollup
                        })
                    })
                    .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"features": features})).unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Show {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .show_feature_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::AddGap {
                    id,
                    gap_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .assign_gap_to_feature(&id, &gap_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::RemoveGap {
                    id,
                    gap_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .remove_gap_from_feature(&id, &gap_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::ReorderGap {
                    id,
                    gap_id,
                    order,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .reorder_gap_in_feature(&id, &gap_id, order)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::OrderGap {
                    id,
                    gap_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .order_gap_in_feature(&id, &gap_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::UnorderGap {
                    id,
                    gap_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .unorder_gap_in_feature(&id, &gap_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Move {
                    id,
                    target,
                    target_root: Some(target_root),
                },
        } => {
            let Some(target) = GapStatus::parse_wire(&target) else {
                return Err(
                    crate::process::supervisor::errors::RefineError::InvalidInput(
                        "target must be backlog or todo".to_string(),
                    ),
                );
            };
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .move_feature_workflow(&id, target)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Transfer {
                    id,
                    node_id,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .transfer_feature_to_node(&node_id, &id)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Cancel {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .cancel_feature_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "gap_ids": feature.gap_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Delete {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            FileWorkItemService::new(refine_dir_for_target_root(&target_root))
                .delete_feature_record(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"deleted": true, "id": id})).unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Import {
                    target_root,
                    text,
                    file,
                    csv,
                    reporter,
                    feature_id,
                },
        } => {
            if skipped_target_root(&target_root) {
                let import = FileImportService::new(PathBuf::new());
                let source = if let Some(file) = file {
                    fs::read_to_string(&file).map_err(|error| {
                        RefineError::Io(format!(
                            "failed to read import file {}: {error}",
                            file.display()
                        ))
                    })?
                } else {
                    text.ok_or_else(|| {
                        RefineError::InvalidInput(
                            "feature import requires --text or --file".to_string(),
                        )
                    })?
                };
                let drafts = if csv {
                    import.parse_csv(&source, reporter.as_deref())?
                } else {
                    import.parse_structured_or_text(&source, reporter.as_deref())?
                };
                if drafts.is_empty() {
                    return Err(RefineError::InvalidInput(
                        "import input did not contain any drafts".to_string(),
                    ));
                }
                let response = daemon_json(
                    "POST",
                    "/import/persist",
                    Some(json!({
                        "drafts": drafts,
                        "reporter": reporter,
                        "feature_id": feature_id
                    })),
                )?;
                print_json(&response);
                return Ok(());
            }
            let service = FileImportService::new(refine_dir_for_target_root(&target_root));
            let result = if let Some(file) = file {
                service.import_from_file(file, csv, reporter.as_deref(), feature_id.as_deref())?
            } else {
                let Some(text) = text.as_deref() else {
                    return Err(
                        crate::process::supervisor::errors::RefineError::InvalidInput(
                            "feature import requires --text or --file".to_string(),
                        ),
                    );
                };
                service.import_from_text(text, csv, reporter.as_deref(), feature_id.as_deref())?
            };
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Gap { action } => dispatch_gap_daemon(action),
        Commands::Feature { action } => dispatch_feature_daemon(action),
        Commands::Workflow { action } => dispatch_workflow_daemon(action),
        Commands::Node { action } => dispatch_node_daemon(action),
        Commands::Cluster { action } => dispatch_cluster_daemon(action),
    }
}

fn run_website(
    port: u16,
    bind_address: std::net::IpAddr,
    static_root: PathBuf,
    once: bool,
) -> RefineResult<()> {
    let static_root = absolute_cli_path(static_root)?;
    let listener = LocalHttpDaemon::bind_address(bind_address, port)?;
    let addr = LocalHttpDaemon::local_addr(&listener)?;
    let actual_port = addr.port();
    let daemon = LocalHttpDaemon {
        server: InProcessWebServer {
            status: DaemonStatus {
                port: actual_port,
                daemon_healthy: false,
                web_available: true,
                worker_state: "disabled".to_string(),
                target_app_state: "not-applicable".to_string(),
                launch_mode: current_launch_mode(),
                executable_path: current_launch_executable(),
                active_operations: Vec::new(),
                degraded_integrations: Vec::new(),
            },
            projection: ProjectionSnapshot::default(),
            target_root: None,
            runtime_root: None,
        },
        static_root: Some(static_root),
    };
    eprintln!("refine: serving website at http://{addr}");
    if once {
        daemon.serve_once(listener)?;
    } else {
        daemon.serve_static(listener)?;
    }
    Ok(())
}

fn run_system_start(
    port: u16,
    bind_address: std::net::IpAddr,
    cache_dir: Option<PathBuf>,
    static_root: Option<PathBuf>,
    runtime_root: PathBuf,
    once: bool,
    foreground: bool,
) -> RefineResult<()> {
    let runtime_root = absolute_cli_path(runtime_root)?;
    let cache_dir = cache_dir.map(absolute_cli_path).transpose()?;
    let static_root = static_root.map(absolute_cli_path).transpose()?;
    if !foreground && !once {
        let status = FileDaemonLifecycleService::new(RuntimeRoot {
            root: runtime_root.clone(),
        })
        .start_background_daemon(BackgroundDaemonConfig {
            port,
            bind_address,
            cache_dir,
            static_root,
        })?;
        println!(
            "{}",
            serde_json::to_string_pretty(&web_response(status)).unwrap()
        );
        return Ok(());
    }
    let listener = LocalHttpDaemon::bind_address(bind_address, port)?;
    let addr = LocalHttpDaemon::local_addr(&listener)?;
    let actual_port = addr.port();
    let port_runtime_root = RuntimeRoot {
        root: runtime_root.clone(),
    }
    .port_root(actual_port);
    eprintln!("refine: preparing daemon at http://{addr}");
    eprintln!("refine: loading active project registry");
    let project_status = FileProjectRegistryService::new(&runtime_root, None).status()?;
    let snapshot = if let Some(target_root) = project_status.target_root {
        eprintln!("refine: warming project cache for {target_root}");
        let target_root = PathBuf::from(target_root);
        let refine_dir = refine_dir_for_target_root(&target_root);
        let cache_root = cache_dir
            .clone()
            .unwrap_or_else(|| port_runtime_root.join("cache"));
        let store = FileProjectStateStore::with_runtime_root(&refine_dir, &port_runtime_root);
        store.load_or_refresh_projection(&cache_root)?
    } else {
        eprintln!("refine: no active project; using empty project cache");
        ProjectionSnapshot::default()
    };
    let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
        root: runtime_root.clone(),
    });
    let status = lifecycle.start(actual_port)?;
    let daemon = LocalHttpDaemon {
        server: InProcessWebServer {
            status,
            projection: snapshot,
            target_root: None,
            runtime_root: Some(port_runtime_root),
        },
        static_root: static_root.or_else(default_static_root),
    };
    daemon.recover_runtime_state_with_progress(|message| {
        eprintln!("refine: {message}");
    })?;
    eprintln!("running foreground Refine daemon at http://{addr}");
    if once {
        daemon.serve_once(listener)?;
    } else {
        daemon.serve_until_unhealthy(listener, lifecycle, actual_port)?;
    }
    Ok(())
}

pub(super) fn system_status_response(runtime_root: PathBuf) -> RefineResult<serde_json::Value> {
    let runtime = RuntimeRoot {
        root: runtime_root.clone(),
    };
    let lifecycle = FileDaemonLifecycleService::new(runtime.clone());
    let ports = lifecycle.running_statuses()?;
    let running_ports: Vec<u16> = ports.iter().map(|status| status.port).collect();
    let ports = ports
        .iter()
        .map(|status| port_status_with_processes(&runtime, status))
        .collect::<Vec<_>>();
    Ok(json!({
        "product": "refine",
        "version": env!("CARGO_PKG_VERSION"),
        "current_version": env!("CARGO_PKG_VERSION"),
        "launch_mode": current_launch_mode(),
        "executable_path": current_launch_executable(),
        "api_contract_version": API_CONTRACT_VERSION,
        "running_ports": running_ports,
        "ports": ports,
    }))
}

pub(super) fn system_ps_response(
    runtime_root: PathBuf,
    port: Option<u16>,
    stop: Option<&str>,
    signal: &str,
) -> RefineResult<serde_json::Value> {
    let runtime = RuntimeRoot {
        root: runtime_root.clone(),
    };
    if let Some(process_id) = stop {
        return stop_system_process(&runtime, port, process_id, signal);
    }
    let ports = selected_process_ports(&runtime, port)?;
    let mut flattened = Vec::new();
    let mut port_values = Vec::new();
    for port in ports {
        let port_root = runtime.port_root(port);
        let summary = FileProcessStatusService::new(&port_root).summary()?;
        if let Some(processes) = summary.get("processes").and_then(|value| value.as_array()) {
            for process in processes {
                let mut process = process.clone();
                if let Some(object) = process.as_object_mut() {
                    object.insert("port".to_string(), json!(port));
                    object.insert(
                        "runtime_root".to_string(),
                        json!(port_root.display().to_string()),
                    );
                }
                flattened.push(process);
            }
        }
        port_values.push(json!({
            "port": port,
            "runtime_root": port_root.display().to_string(),
            "process_count": summary.get("processes").and_then(|value| value.as_array()).map(|processes| processes.len()).unwrap_or(0),
            "process_summary": summary
        }));
    }
    Ok(json!({
        "product": "refine",
        "runtime_root": runtime_root,
        "ports": port_values,
        "process_count": flattened.len(),
        "processes": flattened
    }))
}

fn selected_process_ports(runtime: &RuntimeRoot, port: Option<u16>) -> RefineResult<Vec<u16>> {
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

fn stop_system_process(
    runtime: &RuntimeRoot,
    port: Option<u16>,
    process_id: &str,
    signal: &str,
) -> RefineResult<serde_json::Value> {
    let ports = selected_process_ports(runtime, port)?;
    let mut misses = Vec::new();
    for port in ports {
        let port_root = runtime.port_root(port);
        let service = FileProcessStatusService::new(&port_root);
        match service.stop(process_id, signal) {
            Ok(process) => {
                return Ok(json!({
                    "stopped": true,
                    "port": port,
                    "runtime_root": port_root.display().to_string(),
                    "process": process.api_json()
                }));
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

fn port_status_with_processes(runtime: &RuntimeRoot, status: &DaemonStatus) -> serde_json::Value {
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

fn minimal_status_process(process: &Value) -> Value {
    json!({
        "pid": process.get("pid").cloned().unwrap_or(Value::Null),
        "status": process.get("status").cloned().unwrap_or(Value::Null),
        "label": process.get("label").cloned().unwrap_or(Value::Null),
    })
}

pub(super) fn absolute_cli_path(path: PathBuf) -> RefineResult<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        let cwd = std::env::current_dir().map_err(|error| {
            RefineError::Io(format!("failed to resolve current directory: {error}"))
        })?;
        Ok(cwd.join(path))
    }
}

#[cfg(not(test))]
fn dispatch_project_daemon(action: ProjectAction) -> RefineResult<()> {
    let response = match action {
        ProjectAction::Status { .. } => daemon_json("GET", "/project/status", None)?,
        ProjectAction::Attach { path, .. } => {
            daemon_json("POST", "/project/attach", Some(json!({ "path": path })))?
        }
        ProjectAction::Switch { name, .. } => {
            daemon_json("POST", "/apps/switch", Some(json!({ "name": name })))?
        }
        ProjectAction::Detach { .. } => daemon_json("POST", "/project/detach", None)?,
        ProjectAction::Register { name, path, .. } => daemon_json(
            "POST",
            "/apps/register",
            Some(json!({
                "name": name,
                "path": path
            })),
        )?,
        ProjectAction::Clone {
            source,
            destination,
            name,
            make_current,
            ..
        } => daemon_json(
            "POST",
            "/apps/clone",
            Some(json!({
                "source": source,
                "destination": destination,
                "name": name,
                "make_current": make_current
            })),
        )?,
        ProjectAction::Remove { name, .. } => {
            daemon_json("DELETE", "/apps", Some(json!({ "name": name })))?
        }
        ProjectAction::Migrate { .. } => daemon_json("POST", "/project/migrate", None)?,
        ProjectAction::Sync { .. } => daemon_json("POST", "/project/sync", None)?,
        ProjectAction::Doctor { .. } => daemon_json("GET", "/diagnostics", None)?,
    };
    print_json(&response);
    Ok(())
}

fn dispatch_gap_daemon(action: GapAction) -> RefineResult<()> {
    let response = match action {
        GapAction::Create {
            name,
            target_root: None,
            id,
        } => daemon_json(
            "POST",
            "/work/gaps",
            Some(json!({
                "name": name,
                "id": id
            })),
        )?,
        GapAction::List { target_root: None } => daemon_json("GET", "/work/gaps?limit=1000", None)?,
        GapAction::Show {
            id,
            target_root: None,
        } => daemon_json("GET", &format!("/work/gaps/{}", path_segment(&id)), None)?,
        GapAction::Edit {
            id,
            target_root: None,
            name,
            priority,
        } => daemon_json(
            "PATCH",
            &format!("/work/gaps/{}", path_segment(&id)),
            Some(json!({
                "name": name,
                "priority": priority
            })),
        )?,
        GapAction::Note {
            id,
            body,
            target_root: None,
            author,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/notes", path_segment(&id)),
            Some(json!({
                "body": body,
                "author": author
            })),
        )?,
        GapAction::NoteEdit {
            id,
            note_id,
            body,
            target_root: None,
        } => {
            let detail = daemon_json("GET", &format!("/work/gaps/{}", path_segment(&id)), None)?;
            let notes =
                edit_gap_note_values(gap_notes_from_detail(&detail["gap"]), &note_id, &body)?;
            daemon_json(
                "PATCH",
                &format!("/work/gaps/{}", path_segment(&id)),
                Some(json!({ "notes": notes })),
            )?
        }
        GapAction::NoteDelete {
            id,
            note_id,
            target_root: None,
        } => {
            let detail = daemon_json("GET", &format!("/work/gaps/{}", path_segment(&id)), None)?;
            let notes = delete_gap_note_values(gap_notes_from_detail(&detail["gap"]), &note_id)?;
            daemon_json(
                "PATCH",
                &format!("/work/gaps/{}", path_segment(&id)),
                Some(json!({ "notes": notes })),
            )?
        }
        GapAction::Round {
            id,
            target_root: None,
            reporter,
            actual,
            target,
            edit_latest,
        } => {
            let method = if edit_latest { "PATCH" } else { "POST" };
            let suffix = if edit_latest {
                "/rounds/latest"
            } else {
                "/rounds"
            };
            daemon_json(
                method,
                &format!("/work/gaps/{}{}", path_segment(&id), suffix),
                Some(json!({
                    "reporter": reporter,
                    "actual": actual,
                    "target": target
                })),
            )?
        }
        GapAction::Start {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/start", path_segment(&id)),
            None,
        )?,
        GapAction::Cancel {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/cancel", path_segment(&id)),
            None,
        )?,
        GapAction::Retry {
            id,
            target_root: None,
            stage,
        } => {
            let action = if stage.trim().eq_ignore_ascii_case("merge") {
                "retry-merge"
            } else {
                "retry-quality"
            };
            daemon_json(
                "POST",
                &format!("/work/gaps/{}/{}", path_segment(&id), action),
                None,
            )?
        }
        GapAction::Verify {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/verify", path_segment(&id)),
            None,
        )?,
        GapAction::Merge {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/merge", path_segment(&id)),
            None,
        )?,
        GapAction::Undo {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/undo", path_segment(&id)),
            None,
        )?,
        GapAction::Delete {
            id,
            target_root: None,
        } => daemon_json("DELETE", &format!("/work/gaps/{}", path_segment(&id)), None)?,
        GapAction::AssignFeature {
            id,
            feature_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/gaps/{}",
                path_segment(&feature_id),
                path_segment(&id)
            ),
            None,
        )?,
        GapAction::RemoveFeature {
            id,
            target_root: None,
        } => {
            let current = daemon_json("GET", &format!("/work/gaps/{}", path_segment(&id)), None)?;
            let feature_id = current
                .get("gap")
                .and_then(|gap| gap.get("feature_id"))
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RefineError::Conflict(format!("Gap {id} is not assigned to a Feature"))
                })?;
            daemon_json(
                "DELETE",
                &format!(
                    "/work/features/{}/gaps/{}",
                    path_segment(feature_id),
                    path_segment(&id)
                ),
                None,
            )?
        }
        other => {
            return Err(RefineError::InvalidInput(format!(
                "Gap command cannot be routed to the daemon in this mode: {other:?}"
            )));
        }
    };
    print_json(&response);
    Ok(())
}

fn dispatch_feature_daemon(action: FeatureAction) -> RefineResult<()> {
    let response = match action {
        FeatureAction::Create {
            name,
            target_root: None,
            id,
            description,
            reporter,
        } => daemon_json(
            "POST",
            "/work/features",
            Some(json!({
                "name": name,
                "id": id,
                "description": description,
                "reporter": reporter
            })),
        )?,
        FeatureAction::List { target_root: None } => {
            daemon_json("GET", "/work/features?limit=1000", None)?
        }
        FeatureAction::Show {
            id,
            target_root: None,
        } => daemon_json(
            "GET",
            &format!("/work/features/{}", path_segment(&id)),
            None,
        )?,
        FeatureAction::Edit {
            id,
            target_root: None,
            name,
            description,
            reporter,
        } => daemon_json(
            "PATCH",
            &format!("/work/features/{}", path_segment(&id)),
            Some(json!({
                "name": name,
                "description": description,
                "reporter": reporter
            })),
        )?,
        FeatureAction::AddGap {
            id,
            gap_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/gaps/{}",
                path_segment(&id),
                path_segment(&gap_id)
            ),
            None,
        )?,
        FeatureAction::RemoveGap {
            id,
            gap_id,
            target_root: None,
        } => daemon_json(
            "DELETE",
            &format!(
                "/work/features/{}/gaps/{}",
                path_segment(&id),
                path_segment(&gap_id)
            ),
            None,
        )?,
        FeatureAction::ReorderGap {
            id,
            gap_id,
            order,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/gaps/{}/reorder",
                path_segment(&id),
                path_segment(&gap_id)
            ),
            Some(json!({ "order": order })),
        )?,
        FeatureAction::OrderGap {
            id,
            gap_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/gaps/{}/order",
                path_segment(&id),
                path_segment(&gap_id)
            ),
            None,
        )?,
        FeatureAction::UnorderGap {
            id,
            gap_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/gaps/{}/unorder",
                path_segment(&id),
                path_segment(&gap_id)
            ),
            None,
        )?,
        FeatureAction::Move {
            id,
            target,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/features/{}/move", path_segment(&id)),
            Some(json!({ "status": target })),
        )?,
        FeatureAction::Transfer {
            id,
            node_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/features/{}/transfer", path_segment(&id)),
            Some(json!({ "target_node_id": node_id })),
        )?,
        FeatureAction::Cancel {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/features/{}/cancel", path_segment(&id)),
            None,
        )?,
        FeatureAction::Delete {
            id,
            target_root: None,
        } => daemon_json(
            "DELETE",
            &format!("/work/features/{}", path_segment(&id)),
            None,
        )?,
        FeatureAction::Import {
            target_root,
            text,
            file,
            csv,
            reporter,
            feature_id,
        } if skipped_target_root(&target_root) => {
            let source = if let Some(file) = file {
                fs::read_to_string(&file).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to read import file {}: {error}",
                        file.display()
                    ))
                })?
            } else {
                text.ok_or_else(|| {
                    RefineError::InvalidInput(
                        "feature import requires --text or --file".to_string(),
                    )
                })?
            };
            let parsed = if csv {
                daemon_json(
                    "POST",
                    "/import/csv/parse",
                    Some(json!({
                        "text": source,
                        "reporter": reporter
                    })),
                )?
            } else {
                daemon_json(
                    "POST",
                    "/import/extract",
                    Some(json!({
                        "text": source,
                        "reporter": reporter,
                        "purpose": "feature import"
                    })),
                )?
            };
            let drafts = parsed.get("drafts").cloned().unwrap_or_else(|| json!([]));
            daemon_json(
                "POST",
                "/import/persist",
                Some(json!({
                    "drafts": drafts,
                    "reporter": reporter,
                    "feature_id": feature_id
                })),
            )?
        }
        other => {
            return Err(RefineError::InvalidInput(format!(
                "Feature command cannot be routed to the daemon in this mode: {other:?}"
            )));
        }
    };
    print_json(&response);
    Ok(())
}

fn dispatch_workflow_daemon(action: WorkflowAction) -> RefineResult<()> {
    let response = match action {
        WorkflowAction::Pause { .. } => {
            daemon_json("POST", "/workflow/pause", Some(json!({ "paused": true })))?
        }
        WorkflowAction::Resume { .. } => {
            daemon_json("POST", "/workflow/pause", Some(json!({ "paused": false })))?
        }
    };
    print_json(&response);
    Ok(())
}

#[cfg(not(test))]
fn dispatch_log_daemon(action: LogAction) -> RefineResult<()> {
    let response = match action {
        LogAction::List { target_root, limit } if skipped_target_root(&target_root) => {
            let response = daemon_json("GET", &format!("/activity?limit={limit}"), None)?;
            json!({
                "entries": response.get("activity").cloned().unwrap_or_default()
            })
        }
        LogAction::Tail { target_root, limit } if skipped_target_root(&target_root) => {
            let response = daemon_json("GET", &format!("/activity?limit={limit}"), None)?;
            json!({
                "entries": response.get("activity").cloned().unwrap_or_default(),
                "tail": true
            })
        }
        LogAction::Show { id, target_root } if skipped_target_root(&target_root) => {
            let response = daemon_json("GET", "/activity?limit=1000", None)?;
            let Some(entry) = response
                .get("activity")
                .and_then(|value| value.as_array())
                .and_then(|entries| {
                    entries.iter().find(|entry| {
                        entry.get("id").and_then(|value| value.as_str()) == Some(id.as_str())
                    })
                })
                .cloned()
            else {
                return Err(RefineError::NotFound(format!(
                    "Log entry {id} was not found"
                )));
            };
            json!({ "entry": entry })
        }
        LogAction::Query {
            q,
            target_root,
            limit,
            offset,
            gap_id,
            severity,
            category,
            actor,
        } if skipped_target_root(&target_root) => {
            let mut query = vec![
                format!("limit={limit}"),
                format!("offset={offset}"),
                format!("q={}", query_component(&q)),
            ];
            if let Some(value) = gap_id {
                query.push(format!("gap_id={}", query_component(&value)));
            }
            if let Some(value) = severity {
                query.push(format!("severity={}", query_component(&value)));
            }
            if let Some(value) = category {
                query.push(format!("category={}", query_component(&value)));
            }
            if let Some(value) = actor {
                query.push(format!("actor={}", query_component(&value)));
            }
            let response = daemon_json("GET", &format!("/activity?{}", query.join("&")), None)?;
            json!({
                "entries": response.get("activity").cloned().unwrap_or_default()
            })
        }
        LogAction::Export { target_root: None } => {
            let response = daemon_json("GET", "/activity?limit=1000", None)?;
            let entries = response.get("activity").cloned().unwrap_or_default();
            let exported = entries.as_array().map(Vec::len).unwrap_or_default();
            json!({"entries": entries, "exported": exported})
        }
        LogAction::Bundle {
            target_root,
            redact_secrets,
            ..
        } if skipped_target_root(&target_root) => daemon_json(
            "POST",
            "/diagnostics/support-bundle",
            Some(json!({ "redact_secrets": redact_secrets })),
        )?,
        other => {
            return Err(RefineError::InvalidInput(format!(
                "Log command cannot be routed to the daemon in this mode: {other:?}"
            )));
        }
    };
    print_json(&response);
    Ok(())
}

#[cfg(not(test))]
fn dispatch_agent_daemon(action: AgentAction) -> RefineResult<()> {
    let response = match action {
        AgentAction::Detect => daemon_json("GET", "/agents", None)?,
        AgentAction::Configure { provider } => daemon_json(
            "POST",
            &format!("/agents/{}/configure", path_segment(&provider)),
            None,
        )?,
        AgentAction::Auth { provider } => daemon_json(
            "POST",
            &format!("/agents/{}/auth", path_segment(&provider)),
            None,
        )?,
        AgentAction::Diagnose { provider } => daemon_json(
            "GET",
            &format!("/agents/{}/diagnostics", path_segment(&provider)),
            None,
        )?,
        AgentAction::Invoke {
            prompt,
            provider,
            cwd,
        } => daemon_json(
            "POST",
            &format!("/agents/{}/invoke", path_segment(&provider)),
            Some(json!({
                "prompt": prompt,
                "cwd": cwd.map(|path| path.display().to_string())
            })),
        )?,
        AgentAction::Resume {
            session_id,
            provider,
        } => daemon_json(
            "POST",
            &format!("/agents/{}/resume", path_segment(&provider)),
            Some(json!({
                "session_id": session_id
            })),
        )?,
    };
    print_json(&response);
    Ok(())
}

fn dispatch_node_daemon(action: NodeAction) -> RefineResult<()> {
    let response = match action {
        // Init runs locally: it is how a freshly provisioned machine becomes
        // a working node before any daemon exists to proxy through.
        NodeAction::Init {
            node_id,
            repo_url,
            target_path,
            agent_providers,
            runtime_root,
            port,
        } => {
            let report = initialize_worker(WorkerInitOptions {
                node_id,
                repo_url,
                target_path,
                agent_providers,
                runtime_root: absolute_cli_path(runtime_root)?,
                port,
            })?;
            let ok = report.get("ok").and_then(|value| value.as_bool()) == Some(true);
            print_json(&report);
            if !ok {
                return Err(RefineError::InvalidInput(
                    "node init did not complete; see steps above".to_string(),
                ));
            }
            return Ok(());
        }
        NodeAction::List { target_root: None } => daemon_json("GET", "/nodes", None)?,
        NodeAction::Show {
            id,
            target_root: None,
        } => {
            let nodes = daemon_json("GET", "/nodes", None)?;
            let active_node_id = nodes
                .get("active_node_id")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let node = nodes
                .get("nodes")
                .and_then(|value| value.as_array())
                .and_then(|nodes| {
                    nodes.iter().find(|node| {
                        node.get("id").and_then(|value| value.as_str()) == Some(id.as_str())
                    })
                })
                .cloned()
                .ok_or_else(|| RefineError::NotFound(format!("node {id} was not found")))?;
            json!({
                "node": node,
                "active": id == active_node_id
            })
        }
        NodeAction::Create {
            id,
            target_root: None,
        } => daemon_json("POST", "/nodes", Some(json!({ "id": id })))?,
        NodeAction::Activate {
            id,
            target_root: None,
        } => daemon_json("POST", "/nodes/activate", Some(json!({ "node_id": id })))?,
        NodeAction::Archive {
            id,
            target_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/nodes/{}", path_segment(&id)),
            Some(json!({ "archived": true })),
        )?,
        NodeAction::Rename {
            id,
            name,
            target_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/nodes/{}", path_segment(&id)),
            Some(json!({ "display_name": name })),
        )?,
        NodeAction::Settings {
            id,
            target_root: None,
        } => {
            let nodes = daemon_json("GET", "/nodes", None)?;
            let exists = nodes
                .get("nodes")
                .and_then(|value| value.as_array())
                .is_some_and(|nodes| {
                    nodes.iter().any(|node| {
                        node.get("id").and_then(|value| value.as_str()) == Some(id.as_str())
                    })
                });
            if !exists {
                return Err(RefineError::NotFound(format!("node {id} was not found")));
            }
            let settings = daemon_json("GET", "/settings", None)?;
            json!({
                "node_id": id,
                "settings": settings.get("settings").cloned().unwrap_or(settings)
            })
        }
        NodeAction::Transfer {
            id,
            item_id,
            target_root: None,
        } => daemon_json(
            "POST",
            "/nodes/transfer-gaps",
            Some(json!({
                "target_node_id": id,
                "item_id": item_id,
                "exclude_ids": []
            })),
        )?,
        other => {
            return Err(RefineError::NotImplemented(format!(
                "Node command is not available through the daemon API yet: {other:?}"
            )));
        }
    };
    print_json(&response);
    Ok(())
}

fn dispatch_cluster_daemon(action: ClusterAction) -> RefineResult<()> {
    let response = match action {
        ClusterAction::List { target_root: None } => daemon_json("GET", "/cluster", None)?,
        ClusterAction::Show {
            id,
            target_root: None,
        } => {
            let cluster = daemon_json("GET", "/cluster", None)?;
            let node = cluster
                .get("nodes")
                .and_then(|value| value.as_array())
                .and_then(|nodes| {
                    nodes.iter().find(|node| {
                        node.get("id").and_then(|value| value.as_str()) == Some(id.as_str())
                    })
                })
                .cloned()
                .ok_or_else(|| RefineError::NotFound(format!("node {id} was not found")))?;
            json!({ "node": node })
        }
        ClusterAction::AddNode {
            id,
            target_root: None,
        } => daemon_json("POST", "/cluster/nodes", Some(json!({ "id": id })))?,
        ClusterAction::EditNode {
            id,
            display_name,
            ssh_host,
            ssh_user,
            ssh_identity_path,
            ssh_port,
            refine_checkout,
            target_app_path,
            refine_port,
            provider,
            provisioning,
            enabled,
            target_root: None,
        } => {
            let provisioning = provisioning
                .as_deref()
                .map(parse_provisioning_object)
                .transpose()?;
            daemon_json(
                "PATCH",
                &format!("/cluster/nodes/{}", path_segment(&id)),
                Some(remote_node_edit_body(
                    display_name,
                    ssh_host,
                    ssh_user,
                    ssh_identity_path,
                    ssh_port,
                    refine_checkout,
                    target_app_path,
                    refine_port,
                    provider,
                    provisioning,
                    enabled,
                )),
            )?
        }
        ClusterAction::EnableNode {
            id,
            target_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/cluster/nodes/{}", path_segment(&id)),
            Some(json!({ "enabled": true })),
        )?,
        ClusterAction::DisableNode {
            id,
            target_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/cluster/nodes/{}", path_segment(&id)),
            Some(json!({ "enabled": false })),
        )?,
        ClusterAction::RemoveNode {
            id,
            target_root: None,
        } => daemon_json(
            "DELETE",
            &format!("/cluster/nodes/{}", path_segment(&id)),
            None,
        )?,
        ClusterAction::Bootstrap {
            id,
            dry_run,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/bootstrap", path_segment(&id)),
            Some(json!({ "dry_run": dry_run })),
        )?,
        ClusterAction::Run {
            id,
            command,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/run", path_segment(&id)),
            Some(json!({ "command": command })),
        )?,
        ClusterAction::Providers { target_root: None } => {
            daemon_json("GET", "/cluster/providers", None)?
        }
        ClusterAction::Provision {
            id,
            provider,
            dry_run,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/provision", path_segment(&id)),
            Some(json!({ "provider": provider, "dry_run": dry_run })),
        )?,
        ClusterAction::Deprovision {
            id,
            dry_run,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/deprovision", path_segment(&id)),
            Some(json!({ "dry_run": dry_run })),
        )?,
        ClusterAction::ProvisionStatus {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/provision-status", path_segment(&id)),
            None,
        )?,
        ClusterAction::Distribute {
            to,
            converge,
            dry_run,
            target_root: None,
        } => daemon_json(
            "POST",
            "/cluster/distribute",
            Some(json!({ "to": to, "converge": converge, "dry_run": dry_run })),
        )?,
        ClusterAction::Transfer {
            id,
            item_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/transfer", path_segment(&id)),
            Some(json!({ "item_id": item_id })),
        )?,
        ClusterAction::Maintenance { target_root: None } => {
            let cluster = daemon_json("GET", "/cluster", None)?;
            json!({
                "ok": true,
                "maintenance": {
                    "active": true,
                    "updated_at": cluster.get("updated_at").cloned().unwrap_or(serde_json::Value::Null)
                },
                "cluster": cluster
            })
        }
        ClusterAction::Sync { target_root: None } => {
            let cluster = daemon_json("GET", "/cluster", None)?;
            let synced = cluster
                .get("nodes")
                .and_then(|value| value.as_array())
                .map(|nodes| {
                    nodes
                        .iter()
                        .filter(|node| {
                            node.get("enabled").and_then(|value| value.as_bool()) != Some(false)
                        })
                        .count()
                })
                .unwrap_or(0);
            json!({
                "ok": true,
                "synced": synced,
                "cluster": cluster
            })
        }
        other => {
            return Err(RefineError::NotImplemented(format!(
                "Cluster command is not available through the daemon API yet: {other:?}"
            )));
        }
    };
    print_json(&response);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn remote_node_edit_body(
    display_name: Option<String>,
    ssh_host: Option<String>,
    ssh_user: Option<String>,
    ssh_identity_path: Option<String>,
    ssh_port: Option<u16>,
    refine_checkout: Option<String>,
    target_app_path: Option<String>,
    refine_port: Option<u16>,
    provider: Option<String>,
    provisioning: Option<crate::model::JsonObject>,
    enabled: Option<bool>,
) -> serde_json::Value {
    let mut body = serde_json::Map::new();
    if let Some(value) = display_name {
        body.insert("display_name".to_string(), json!(value));
    }
    if let Some(value) = ssh_host {
        body.insert("ssh_host".to_string(), json!(value));
    }
    if let Some(value) = ssh_user {
        body.insert("ssh_user".to_string(), json!(value));
    }
    if let Some(value) = ssh_identity_path {
        body.insert("ssh_identity_path".to_string(), json!(value));
    }
    if let Some(value) = ssh_port {
        body.insert("ssh_port".to_string(), json!(value));
    }
    if let Some(value) = refine_checkout {
        body.insert("refine_checkout".to_string(), json!(value));
    }
    if let Some(value) = target_app_path {
        body.insert("target_app_path".to_string(), json!(value));
    }
    if let Some(value) = refine_port {
        body.insert("refine_port".to_string(), json!(value));
    }
    if let Some(value) = provider {
        body.insert("provider".to_string(), json!(value));
    }
    if let Some(value) = provisioning {
        body.insert("provisioning".to_string(), json!(value));
    }
    if let Some(value) = enabled {
        body.insert("enabled".to_string(), json!(value));
    }
    serde_json::Value::Object(body)
}

fn parse_provisioning_object(raw: &str) -> RefineResult<crate::model::JsonObject> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
        RefineError::InvalidInput(format!("provisioning must be a JSON object: {error}"))
    })?;
    match value {
        serde_json::Value::Object(object) => Ok(object),
        _ => Err(RefineError::InvalidInput(
            "provisioning must be a JSON object".to_string(),
        )),
    }
}

fn daemon_json(
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> RefineResult<serde_json::Value> {
    let body_bytes = body
        .map(|value| serde_json::to_vec(&value))
        .transpose()
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode daemon request: {error}"))
        })?
        .unwrap_or_default();
    let port = daemon_port();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| {
        RefineError::Degraded(format!(
            "Refine daemon is required for this CLI command but is not reachable at http://127.0.0.1:{port}: {error}. Start it with `refine system start`."
        ))
    })?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Refine-API-Version: 1\r\nIdempotency-Key: cli-{}\r\n\r\n",
        body_bytes.len(),
        new_cli_idempotency_key()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(&body_bytes))
        .map_err(|error| RefineError::Io(format!("failed to write daemon request: {error}")))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| RefineError::Io(format!("failed to read daemon response: {error}")))?;
    parse_daemon_response(&response)
}

fn daemon_port() -> u16 {
    std::env::var("REFINE_DAEMON_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(8082)
}

fn parse_daemon_response(response: &[u8]) -> RefineResult<serde_json::Value> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| RefineError::Serialization("daemon response missing headers".to_string()))?;
    let (head, body) = response.split_at(split);
    let body = &body[4..];
    let head = String::from_utf8_lossy(head);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| RefineError::Serialization("daemon response missing status".to_string()))?;
    let value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice::<serde_json::Value>(body).map_err(|error| {
            RefineError::Serialization(format!("failed to parse daemon response body: {error}"))
        })?
    };
    if status < 400 {
        return Ok(value);
    }
    let message = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .unwrap_or("daemon request failed")
        .to_string();
    match status {
        400 => Err(RefineError::InvalidInput(message)),
        401 | 403 => Err(RefineError::Unauthorized(message)),
        404 => Err(RefineError::NotFound(message)),
        409 => Err(RefineError::Conflict(message)),
        _ => Err(RefineError::Degraded(message)),
    }
}

fn print_json(value: &serde_json::Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

fn path_segment(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                escaped.push(*byte as char)
            }
            other => escaped.push_str(&format!("%{other:02X}")),
        }
    }
    escaped
}

fn query_component(value: &str) -> String {
    path_segment(value)
}

fn gap_notes_from_detail(detail: &Value) -> Vec<Value> {
    detail
        .get("notes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn edit_gap_note_values(
    mut notes: Vec<Value>,
    note_id: &str,
    body: &str,
) -> RefineResult<Vec<Value>> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(RefineError::InvalidInput(
            "note body cannot be empty".to_string(),
        ));
    }
    let mut found = false;
    for note in &mut notes {
        if note.get("id").and_then(Value::as_str) == Some(note_id) {
            let object = note.as_object_mut().ok_or_else(|| {
                RefineError::InvalidInput("notes must be an array of objects".to_string())
            })?;
            object.insert("body".to_string(), Value::String(trimmed.to_string()));
            found = true;
            break;
        }
    }
    if !found {
        return Err(RefineError::NotFound(format!(
            "note {note_id} was not found"
        )));
    }
    Ok(notes)
}

fn delete_gap_note_values(notes: Vec<Value>, note_id: &str) -> RefineResult<Vec<Value>> {
    let original_len = notes.len();
    let next = notes
        .into_iter()
        .filter(|note| note.get("id").and_then(Value::as_str) != Some(note_id))
        .collect::<Vec<_>>();
    if next.len() == original_len {
        return Err(RefineError::NotFound(format!(
            "note {note_id} was not found"
        )));
    }
    Ok(next)
}

fn skipped_target_root(path: &PathBuf) -> bool {
    path.as_os_str().is_empty()
}

fn refine_dir_for_target_root(target_root: &Path) -> PathBuf {
    target_root.join(".refine")
}

pub(super) fn explicit_target_root_path(command: &Commands) -> Option<&PathBuf> {
    match command {
        Commands::Project { action } => match action {
            ProjectAction::Status { target_root, .. }
            | ProjectAction::Attach { target_root, .. }
            | ProjectAction::Switch { target_root, .. }
            | ProjectAction::Detach { target_root, .. }
            | ProjectAction::Register { target_root, .. }
            | ProjectAction::Clone { target_root, .. }
            | ProjectAction::Remove { target_root, .. }
            | ProjectAction::Migrate { target_root, .. }
            | ProjectAction::Sync { target_root, .. }
            | ProjectAction::Doctor { target_root, .. } => target_root.as_ref(),
        },
        Commands::Gap { action } => match action {
            GapAction::Create { target_root, .. }
            | GapAction::List { target_root }
            | GapAction::Show { target_root, .. }
            | GapAction::Edit { target_root, .. }
            | GapAction::Note { target_root, .. }
            | GapAction::NoteEdit { target_root, .. }
            | GapAction::NoteDelete { target_root, .. }
            | GapAction::Round { target_root, .. }
            | GapAction::Start { target_root, .. }
            | GapAction::Cancel { target_root, .. }
            | GapAction::Retry { target_root, .. }
            | GapAction::Verify { target_root, .. }
            | GapAction::Merge { target_root, .. }
            | GapAction::Undo { target_root, .. }
            | GapAction::Delete { target_root, .. }
            | GapAction::AssignFeature { target_root, .. }
            | GapAction::RemoveFeature { target_root, .. } => target_root.as_ref(),
        },
        Commands::Feature { action } => match action {
            FeatureAction::Create { target_root, .. }
            | FeatureAction::List { target_root }
            | FeatureAction::Show { target_root, .. }
            | FeatureAction::Edit { target_root, .. }
            | FeatureAction::AddGap { target_root, .. }
            | FeatureAction::RemoveGap { target_root, .. }
            | FeatureAction::ReorderGap { target_root, .. }
            | FeatureAction::OrderGap { target_root, .. }
            | FeatureAction::UnorderGap { target_root, .. }
            | FeatureAction::Move { target_root, .. }
            | FeatureAction::Transfer { target_root, .. }
            | FeatureAction::Cancel { target_root, .. }
            | FeatureAction::Delete { target_root, .. } => target_root.as_ref(),
            FeatureAction::Import { target_root, .. } => Some(target_root),
        },
        Commands::Workflow { action } => match action {
            WorkflowAction::Pause { .. } | WorkflowAction::Resume { .. } => None,
        },
        Commands::Node { action } => match action {
            NodeAction::List { target_root }
            | NodeAction::Show { target_root, .. }
            | NodeAction::Create { target_root, .. }
            | NodeAction::Activate { target_root, .. }
            | NodeAction::Archive { target_root, .. }
            | NodeAction::Rename { target_root, .. }
            | NodeAction::Settings { target_root, .. }
            | NodeAction::Transfer { target_root, .. } => target_root.as_ref(),
            NodeAction::Init { .. } => None,
        },
        Commands::Cluster { action } => match action {
            ClusterAction::List { target_root }
            | ClusterAction::Show { target_root, .. }
            | ClusterAction::AddNode { target_root, .. }
            | ClusterAction::EditNode { target_root, .. }
            | ClusterAction::EnableNode { target_root, .. }
            | ClusterAction::DisableNode { target_root, .. }
            | ClusterAction::RemoveNode { target_root, .. }
            | ClusterAction::Bootstrap { target_root, .. }
            | ClusterAction::Providers { target_root }
            | ClusterAction::Provision { target_root, .. }
            | ClusterAction::Deprovision { target_root, .. }
            | ClusterAction::ProvisionStatus { target_root, .. }
            | ClusterAction::Distribute { target_root, .. }
            | ClusterAction::Sync { target_root }
            | ClusterAction::Run { target_root, .. }
            | ClusterAction::Transfer { target_root, .. }
            | ClusterAction::Maintenance { target_root } => target_root.as_ref(),
        },
        Commands::Log { action } => match action {
            LogAction::List { target_root, .. }
            | LogAction::Tail { target_root, .. }
            | LogAction::Show { target_root, .. }
            | LogAction::Query { target_root, .. }
            | LogAction::Bundle { target_root, .. } => Some(target_root),
            LogAction::Export { target_root } => target_root.as_ref(),
        },
        Commands::Agent { .. } => None,
        Commands::Website { .. } => None,
        Commands::Next { target_root } => target_root.as_ref(),
        Commands::Commands => None,
        Commands::System { action } => match action {
            SystemAction::Doctor { target_root, .. } => target_root.as_ref(),
            SystemAction::Install { .. }
            | SystemAction::Repair { .. }
            | SystemAction::Update { .. }
            | SystemAction::Rollback { .. }
            | SystemAction::Uninstall { .. }
            | SystemAction::Start { .. }
            | SystemAction::Stop { .. }
            | SystemAction::Restart { .. }
            | SystemAction::Status { .. }
            | SystemAction::Ps { .. }
            | SystemAction::ApiGroups => None,
        },
    }
    .filter(|path| !skipped_target_root(path))
}

fn new_cli_idempotency_key() -> String {
    format!(
        "{}:{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

fn web_response(status: DaemonStatus) -> serde_json::Value {
    json!({
        "url": format!("http://127.0.0.1:{}/", status.port),
        "status": status
    })
}

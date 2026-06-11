use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;

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
use crate::tools::host::cluster::{ClusterNodeUpdate, ClusterService, FileClusterRegistryService};
use crate::tools::host::deployed_update::{
    DeployedUpdateOptions, FileDeployedUpdateHost, discover_refine_checkout, run_deployed_update,
};
use crate::tools::host::installation::{FileInstallationService, InstallationService};
use crate::tools::observability::activity::{ActivityService, FileActivityService};
use crate::tools::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::tools::observability::support_bundle::{FileSupportBundleService, SupportBundleService};
use crate::tools::product::imports::FileImportService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::project_registry::{FileProjectRegistryService, ProjectRegistryService};
use crate::tools::product::project_state::{
    FileProjectStateStore, ProjectStateStore, ProjectionQuery, ProjectionSnapshot,
};
use crate::tools::product::work_items::{BulkGapFilter, BulkGapSelection, FileWorkItemService};

use super::actions::*;
use super::helpers::*;

pub fn run() -> RefineResult<()> {
    let cli = Cli::parse();
    dispatch(cli)
}

pub fn dispatch(cli: Cli) -> RefineResult<()> {
    #[cfg(not(test))]
    if let Some(path) = explicit_durable_root_path(&cli.command) {
        return Err(RefineError::InvalidInput(format!(
            "direct --durable-root CLI dispatch is not supported in normal operation; use the daemon API for durable state mutations instead ({})",
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
                    durable_root,
                    runtime_root,
                    repo_root,
                },
        } => {
            let report =
                FileDiagnosticsService::new(durable_root, runtime_root, repo_root).doctor()?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::List {
                    durable_root: Some(durable_root),
                },
        } => {
            let nodes = FileNodeRegistryService::new(durable_root).list_response()?;
            println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Show {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let node = FileNodeRegistryService::new(durable_root).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Create {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let node = FileNodeRegistryService::new(durable_root).create(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Activate {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let nodes = FileNodeRegistryService::new(durable_root).activate(&id)?;
            println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Archive {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let node = FileNodeRegistryService::new(durable_root).archive(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Rename {
                    id,
                    name,
                    durable_root: Some(durable_root),
                },
        } => {
            let node = FileNodeRegistryService::new(durable_root).rename(&id, &name)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Settings {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let settings = FileNodeRegistryService::new(durable_root).settings(&id)?;
            println!("{}", serde_json::to_string_pretty(&settings).unwrap());
            Ok(())
        }
        Commands::Node {
            action:
                NodeAction::Transfer {
                    id,
                    item_id,
                    durable_root: Some(durable_root),
                },
        } => {
            FileNodeRegistryService::new(&durable_root).show(&id)?;
            let result = FileWorkItemService::new(durable_root).bulk_transfer_gaps_to_node(
                &id,
                BulkGapSelection {
                    filter: BulkGapFilter::default(),
                    selected_ids: Some(vec![item_id]),
                    exclude_ids: Vec::new(),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::List {
                    durable_root: Some(durable_root),
                },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).list_response()?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Show {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let node = FileClusterRegistryService::new(durable_root).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::AddNode {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).add_node(&id)?;
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
                    enabled,
                    durable_root: Some(durable_root),
                },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).upsert_node(
                &id,
                ClusterNodeUpdate {
                    display_name,
                    ssh_host,
                    ssh_user,
                    ssh_identity_path,
                    ssh_port: ssh_port.map(u64::from),
                    refine_checkout,
                    target_app_path,
                    refine_port: refine_port.map(u64::from),
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
                    durable_root: Some(durable_root),
                },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).set_enabled(&id, true)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::DisableNode {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).set_enabled(&id, false)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::RemoveNode {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).remove_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Bootstrap {
                    id,
                    dry_run,
                    durable_root: Some(durable_root),
                },
        } => {
            let result = FileClusterRegistryService::new(durable_root)
                .bootstrap_node_response(&id, dry_run)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Sync {
                    durable_root: Some(durable_root),
                },
        } => {
            let sync = FileClusterRegistryService::new(durable_root).sync_response()?;
            println!("{}", serde_json::to_string_pretty(&sync).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Run {
                    id,
                    command,
                    durable_root: Some(durable_root),
                },
        } => {
            let result = FileClusterRegistryService::new(durable_root).run_remote(&id, &command)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Transfer {
                    id,
                    item_id,
                    durable_root: Some(durable_root),
                },
        } => {
            let service = FileClusterRegistryService::new(&durable_root);
            service.transfer(&item_id, &id)?;
            let result = FileWorkItemService::new(durable_root).bulk_transfer_gaps_to_node(
                &id,
                BulkGapSelection {
                    filter: BulkGapFilter::default(),
                    selected_ids: Some(vec![item_id]),
                    exclude_ids: Vec::new(),
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action:
                ClusterAction::Maintenance {
                    durable_root: Some(durable_root),
                },
        } => {
            let maintenance =
                FileClusterRegistryService::new(durable_root).maintenance_response()?;
            println!("{}", serde_json::to_string_pretty(&maintenance).unwrap());
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::List {
                    durable_root,
                    limit,
                },
        } => {
            if skipped_durable_root(&durable_root) {
                let response = daemon_json("GET", &format!("/activity?limit={limit}"), None)?;
                print_json(&json!({
                    "entries": response.get("activity").cloned().unwrap_or_default()
                }));
                return Ok(());
            }
            let entries = FileActivityService::new(durable_root).recent(limit)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action:
                LogAction::Tail {
                    durable_root,
                    limit,
                },
        } => {
            if skipped_durable_root(&durable_root) {
                let response = daemon_json("GET", &format!("/activity?limit={limit}"), None)?;
                print_json(&json!({
                    "entries": response.get("activity").cloned().unwrap_or_default(),
                    "tail": true
                }));
                return Ok(());
            }
            let entries = FileActivityService::new(durable_root).recent(limit)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"entries": entries, "tail": true})).unwrap()
            );
            Ok(())
        }
        Commands::Log {
            action: LogAction::Show { id, durable_root },
        } => {
            if skipped_durable_root(&durable_root) {
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
            let service = FileActivityService::new(durable_root);
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
                    durable_root,
                    limit,
                    offset,
                    gap_id,
                    severity,
                    category,
                    actor,
                },
        } => {
            if skipped_durable_root(&durable_root) {
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
            let entries = FileActivityService::new(durable_root).query(
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
                    durable_root: Some(durable_root),
                },
        } => {
            let service = FileActivityService::new(durable_root);
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
            action: LogAction::Export { durable_root: None },
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
                    durable_root,
                    runtime_root,
                    repo_root,
                    redact_secrets,
                },
        } => {
            if skipped_durable_root(&durable_root) {
                let response = daemon_json(
                    "POST",
                    "/diagnostics/support-bundle",
                    Some(json!({ "redact_secrets": redact_secrets })),
                )?;
                print_json(&response);
                return Ok(());
            }
            let bundle = FileSupportBundleService::new(durable_root, runtime_root, repo_root)
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
        Commands::Project {
            action:
                ProjectAction::Status {
                    runtime_root,
                    durable_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, durable_root).status()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Attach {
                    path,
                    runtime_root,
                    durable_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, durable_root)
                .attach_with_migration(&path)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Switch {
                    name,
                    runtime_root,
                    durable_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, durable_root)
                .switch_with_migration(&name)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Detach {
                    runtime_root,
                    durable_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, durable_root).detach()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Register {
                    name,
                    path,
                    runtime_root,
                    durable_root,
                },
        } => {
            let registry = FileProjectRegistryService::new(runtime_root, durable_root)
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
                    durable_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, durable_root).clone_app(
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
                    durable_root,
                },
        } => {
            let registry =
                FileProjectRegistryService::new(runtime_root, durable_root).remove(&name)?;
            println!("{}", serde_json::to_string_pretty(&registry).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Migrate {
                    durable_root,
                    runtime_root,
                },
        } => {
            let report =
                FileProjectRegistryService::new(runtime_root, durable_root).migrate_current()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::to_value(report).unwrap()).unwrap()
            );
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Doctor {
                    durable_root,
                    runtime_root,
                    repo_root,
                },
        } => {
            let report =
                FileDiagnosticsService::new(durable_root, runtime_root, repo_root).doctor()?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Sync {
                    durable_root: Some(durable_root),
                    cache_dir,
                },
        } => {
            let store = cache_dir
                .as_ref()
                .and_then(|cache_dir| cache_dir.parent())
                .map(|runtime_root| {
                    FileProjectStateStore::with_runtime_root(&durable_root, runtime_root)
                })
                .unwrap_or_else(|| FileProjectStateStore::new(&durable_root));
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
                durable_root: None, ..
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
                    durable_root: Some(durable_root),
                    id,
                },
        } => {
            let gap =
                FileWorkItemService::new(durable_root).create_gap_summary(&name, id.as_deref())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Gap {
            action:
                GapAction::List {
                    durable_root: Some(durable_root),
                },
        } => {
            let gaps: Vec<_> = FileWorkItemService::new(durable_root)
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
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).show_gap_summary(&id)?;
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
                    durable_root: Some(durable_root),
                    name,
                    priority,
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).update_gap_metadata_summary(
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
                    durable_root: Some(durable_root),
                    author,
                },
        } => {
            let gap =
                FileWorkItemService::new(durable_root).add_gap_note_summary(&id, &author, &body)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
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
                    durable_root: Some(durable_root),
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
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
                    durable_root: Some(durable_root),
                    reporter,
                    actual,
                    target,
                    edit_latest,
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
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
                    durable_root: Some(durable_root),
                },
        } => {
            FileWorkItemService::new(durable_root).delete_gap_record(&id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).cancel_gap_summary(&id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).start_gap_workflow(&id)?;
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
                    durable_root: Some(durable_root),
                    stage,
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
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
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).verify_gap_summary(&id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).merge_gap_summary(&id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let gap = FileWorkItemService::new(durable_root).undo_gap_summary(&id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let feature =
                FileWorkItemService::new(durable_root).assign_gap_to_feature(&feature_id, &id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let service = FileWorkItemService::new(durable_root);
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
                    durable_root: Some(durable_root),
                    id,
                    description,
                    reporter,
                },
        } => {
            let feature = FileWorkItemService::new(durable_root).create_feature_summary(
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
                    durable_root: Some(durable_root),
                    name,
                    description,
                    reporter,
                },
        } => {
            let feature = FileWorkItemService::new(durable_root).update_feature_metadata_summary(
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
                    durable_root: Some(durable_root),
                },
        } => {
            let features: Vec<_> = FileWorkItemService::new(durable_root)
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
                    durable_root: Some(durable_root),
                },
        } => {
            let feature = FileWorkItemService::new(durable_root).show_feature_summary(&id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let feature =
                FileWorkItemService::new(durable_root).assign_gap_to_feature(&id, &gap_id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let feature =
                FileWorkItemService::new(durable_root).remove_gap_from_feature(&id, &gap_id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            let feature = FileWorkItemService::new(durable_root)
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
                FeatureAction::Move {
                    id,
                    target,
                    durable_root: Some(durable_root),
                },
        } => {
            let Some(target) = GapStatus::parse_wire(&target) else {
                return Err(
                    crate::process::supervisor::errors::RefineError::InvalidInput(
                        "target must be backlog or todo".to_string(),
                    ),
                );
            };
            let feature =
                FileWorkItemService::new(durable_root).move_feature_workflow(&id, target)?;
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
                FeatureAction::Cancel {
                    id,
                    durable_root: Some(durable_root),
                },
        } => {
            let feature = FileWorkItemService::new(durable_root).cancel_feature_summary(&id)?;
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
                    durable_root: Some(durable_root),
                },
        } => {
            FileWorkItemService::new(durable_root).delete_feature_record(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"deleted": true, "id": id})).unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::Import {
                    durable_root,
                    text,
                    file,
                    csv,
                    reporter,
                    feature_id,
                },
        } => {
            if skipped_durable_root(&durable_root) {
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
                    import.parse_text(&source, reporter.as_deref())?
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
            let service = FileImportService::new(durable_root);
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
    let snapshot = if let Some(client_repo) = project_status.client_repo {
        eprintln!("refine: warming project cache for {client_repo}");
        let durable_root = PathBuf::from(client_repo).join(".refine");
        let cache_root = cache_dir
            .clone()
            .unwrap_or_else(|| port_runtime_root.join("cache"));
        let store = FileProjectStateStore::with_runtime_root(&durable_root, &port_runtime_root);
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
    let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root });
    let ports = lifecycle.running_statuses()?;
    let running_ports: Vec<u16> = ports.iter().map(|status| status.port).collect();
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
            durable_root: None,
            id,
        } => daemon_json(
            "POST",
            "/work/gaps",
            Some(json!({
                "name": name,
                "id": id
            })),
        )?,
        GapAction::List { durable_root: None } => {
            daemon_json("GET", "/work/gaps?limit=1000", None)?
        }
        GapAction::Show {
            id,
            durable_root: None,
        } => daemon_json("GET", &format!("/work/gaps/{}", path_segment(&id)), None)?,
        GapAction::Edit {
            id,
            durable_root: None,
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
            durable_root: None,
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
            durable_root: None,
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
            durable_root: None,
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
            durable_root: None,
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
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/start", path_segment(&id)),
            None,
        )?,
        GapAction::Cancel {
            id,
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/cancel", path_segment(&id)),
            None,
        )?,
        GapAction::Retry {
            id,
            durable_root: None,
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
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/verify", path_segment(&id)),
            None,
        )?,
        GapAction::Merge {
            id,
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/merge", path_segment(&id)),
            None,
        )?,
        GapAction::Undo {
            id,
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/gaps/{}/undo", path_segment(&id)),
            None,
        )?,
        GapAction::Delete {
            id,
            durable_root: None,
        } => daemon_json("DELETE", &format!("/work/gaps/{}", path_segment(&id)), None)?,
        GapAction::AssignFeature {
            id,
            feature_id,
            durable_root: None,
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
            durable_root: None,
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
            durable_root: None,
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
        FeatureAction::List { durable_root: None } => {
            daemon_json("GET", "/work/features?limit=1000", None)?
        }
        FeatureAction::Show {
            id,
            durable_root: None,
        } => daemon_json(
            "GET",
            &format!("/work/features/{}", path_segment(&id)),
            None,
        )?,
        FeatureAction::Edit {
            id,
            durable_root: None,
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
            durable_root: None,
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
            durable_root: None,
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
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/gaps/{}/reorder",
                path_segment(&id),
                path_segment(&gap_id)
            ),
            Some(json!({ "order": order })),
        )?,
        FeatureAction::Move {
            id,
            target,
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/features/{}/move", path_segment(&id)),
            Some(json!({ "status": target })),
        )?,
        FeatureAction::Cancel {
            id,
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/features/{}/cancel", path_segment(&id)),
            None,
        )?,
        FeatureAction::Delete {
            id,
            durable_root: None,
        } => daemon_json(
            "DELETE",
            &format!("/work/features/{}", path_segment(&id)),
            None,
        )?,
        FeatureAction::Import {
            durable_root,
            text,
            file,
            csv,
            reporter,
            feature_id,
        } if skipped_durable_root(&durable_root) => {
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
            daemon_json(
                "POST",
                "/processes/background",
                Some(json!({ "stopped": true })),
            )?;
            daemon_json("POST", "/processes/agents", Some(json!({ "paused": true })))?
        }
        WorkflowAction::Resume { .. } => {
            daemon_json(
                "POST",
                "/processes/background",
                Some(json!({ "stopped": false })),
            )?;
            daemon_json(
                "POST",
                "/processes/agents",
                Some(json!({ "paused": false })),
            )?
        }
    };
    print_json(&response);
    Ok(())
}

#[cfg(not(test))]
fn dispatch_log_daemon(action: LogAction) -> RefineResult<()> {
    let response = match action {
        LogAction::List {
            durable_root,
            limit,
        } if skipped_durable_root(&durable_root) => {
            let response = daemon_json("GET", &format!("/activity?limit={limit}"), None)?;
            json!({
                "entries": response.get("activity").cloned().unwrap_or_default()
            })
        }
        LogAction::Tail {
            durable_root,
            limit,
        } if skipped_durable_root(&durable_root) => {
            let response = daemon_json("GET", &format!("/activity?limit={limit}"), None)?;
            json!({
                "entries": response.get("activity").cloned().unwrap_or_default(),
                "tail": true
            })
        }
        LogAction::Show { id, durable_root } if skipped_durable_root(&durable_root) => {
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
            durable_root,
            limit,
            offset,
            gap_id,
            severity,
            category,
            actor,
        } if skipped_durable_root(&durable_root) => {
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
        LogAction::Export { durable_root: None } => {
            let response = daemon_json("GET", "/activity?limit=1000", None)?;
            let entries = response.get("activity").cloned().unwrap_or_default();
            let exported = entries.as_array().map(Vec::len).unwrap_or_default();
            json!({"entries": entries, "exported": exported})
        }
        LogAction::Bundle {
            durable_root,
            redact_secrets,
            ..
        } if skipped_durable_root(&durable_root) => daemon_json(
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
        NodeAction::List { durable_root: None } => daemon_json("GET", "/nodes", None)?,
        NodeAction::Show {
            id,
            durable_root: None,
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
            durable_root: None,
        } => daemon_json("POST", "/nodes", Some(json!({ "id": id })))?,
        NodeAction::Activate {
            id,
            durable_root: None,
        } => daemon_json("POST", "/nodes/activate", Some(json!({ "node_id": id })))?,
        NodeAction::Archive {
            id,
            durable_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/nodes/{}", path_segment(&id)),
            Some(json!({ "archived": true })),
        )?,
        NodeAction::Rename {
            id,
            name,
            durable_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/nodes/{}", path_segment(&id)),
            Some(json!({ "display_name": name })),
        )?,
        NodeAction::Settings {
            id,
            durable_root: None,
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
            durable_root: None,
        } => daemon_json(
            "POST",
            "/nodes/transfer-gaps",
            Some(json!({
                "target_node_id": id,
                "selected_ids": [item_id],
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
        ClusterAction::List { durable_root: None } => daemon_json("GET", "/cluster", None)?,
        ClusterAction::Show {
            id,
            durable_root: None,
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
                .ok_or_else(|| RefineError::NotFound(format!("cluster node {id} was not found")))?;
            json!({ "node": node })
        }
        ClusterAction::AddNode {
            id,
            durable_root: None,
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
            enabled,
            durable_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/cluster/nodes/{}", path_segment(&id)),
            Some(cluster_node_edit_body(
                display_name,
                ssh_host,
                ssh_user,
                ssh_identity_path,
                ssh_port,
                refine_checkout,
                target_app_path,
                refine_port,
                enabled,
            )),
        )?,
        ClusterAction::EnableNode {
            id,
            durable_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/cluster/nodes/{}", path_segment(&id)),
            Some(json!({ "enabled": true })),
        )?,
        ClusterAction::DisableNode {
            id,
            durable_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/cluster/nodes/{}", path_segment(&id)),
            Some(json!({ "enabled": false })),
        )?,
        ClusterAction::RemoveNode {
            id,
            durable_root: None,
        } => daemon_json(
            "DELETE",
            &format!("/cluster/nodes/{}", path_segment(&id)),
            None,
        )?,
        ClusterAction::Bootstrap {
            id,
            dry_run,
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/bootstrap", path_segment(&id)),
            Some(json!({ "dry_run": dry_run })),
        )?,
        ClusterAction::Run {
            id,
            command,
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/run", path_segment(&id)),
            Some(json!({ "command": command })),
        )?,
        ClusterAction::Transfer {
            id,
            item_id,
            durable_root: None,
        } => daemon_json(
            "POST",
            &format!("/cluster/nodes/{}/transfer", path_segment(&id)),
            Some(json!({ "item_id": item_id })),
        )?,
        ClusterAction::Maintenance { durable_root: None } => {
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
        ClusterAction::Sync { durable_root: None } => {
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
fn cluster_node_edit_body(
    display_name: Option<String>,
    ssh_host: Option<String>,
    ssh_user: Option<String>,
    ssh_identity_path: Option<String>,
    ssh_port: Option<u16>,
    refine_checkout: Option<String>,
    target_app_path: Option<String>,
    refine_port: Option<u16>,
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
    if let Some(value) = enabled {
        body.insert("enabled".to_string(), json!(value));
    }
    serde_json::Value::Object(body)
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
        .unwrap_or(8080)
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

fn skipped_durable_root(path: &PathBuf) -> bool {
    path.as_os_str().is_empty()
}

pub(super) fn explicit_durable_root_path(command: &Commands) -> Option<&PathBuf> {
    match command {
        Commands::Project { action } => match action {
            ProjectAction::Status { durable_root, .. }
            | ProjectAction::Attach { durable_root, .. }
            | ProjectAction::Switch { durable_root, .. }
            | ProjectAction::Detach { durable_root, .. }
            | ProjectAction::Register { durable_root, .. }
            | ProjectAction::Clone { durable_root, .. }
            | ProjectAction::Remove { durable_root, .. }
            | ProjectAction::Migrate { durable_root, .. }
            | ProjectAction::Sync { durable_root, .. }
            | ProjectAction::Doctor { durable_root, .. } => durable_root.as_ref(),
        },
        Commands::Gap { action } => match action {
            GapAction::Create { durable_root, .. }
            | GapAction::List { durable_root }
            | GapAction::Show { durable_root, .. }
            | GapAction::Edit { durable_root, .. }
            | GapAction::Note { durable_root, .. }
            | GapAction::NoteEdit { durable_root, .. }
            | GapAction::NoteDelete { durable_root, .. }
            | GapAction::Round { durable_root, .. }
            | GapAction::Start { durable_root, .. }
            | GapAction::Cancel { durable_root, .. }
            | GapAction::Retry { durable_root, .. }
            | GapAction::Verify { durable_root, .. }
            | GapAction::Merge { durable_root, .. }
            | GapAction::Undo { durable_root, .. }
            | GapAction::Delete { durable_root, .. }
            | GapAction::AssignFeature { durable_root, .. }
            | GapAction::RemoveFeature { durable_root, .. } => durable_root.as_ref(),
        },
        Commands::Feature { action } => match action {
            FeatureAction::Create { durable_root, .. }
            | FeatureAction::List { durable_root }
            | FeatureAction::Show { durable_root, .. }
            | FeatureAction::Edit { durable_root, .. }
            | FeatureAction::AddGap { durable_root, .. }
            | FeatureAction::RemoveGap { durable_root, .. }
            | FeatureAction::ReorderGap { durable_root, .. }
            | FeatureAction::Move { durable_root, .. }
            | FeatureAction::Cancel { durable_root, .. }
            | FeatureAction::Delete { durable_root, .. } => durable_root.as_ref(),
            FeatureAction::Import { durable_root, .. } => Some(durable_root),
        },
        Commands::Workflow { action } => match action {
            WorkflowAction::Pause { .. } | WorkflowAction::Resume { .. } => None,
        },
        Commands::Node { action } => match action {
            NodeAction::List { durable_root }
            | NodeAction::Show { durable_root, .. }
            | NodeAction::Create { durable_root, .. }
            | NodeAction::Activate { durable_root, .. }
            | NodeAction::Archive { durable_root, .. }
            | NodeAction::Rename { durable_root, .. }
            | NodeAction::Settings { durable_root, .. }
            | NodeAction::Transfer { durable_root, .. } => durable_root.as_ref(),
        },
        Commands::Cluster { action } => match action {
            ClusterAction::List { durable_root }
            | ClusterAction::Show { durable_root, .. }
            | ClusterAction::AddNode { durable_root, .. }
            | ClusterAction::EditNode { durable_root, .. }
            | ClusterAction::EnableNode { durable_root, .. }
            | ClusterAction::DisableNode { durable_root, .. }
            | ClusterAction::RemoveNode { durable_root, .. }
            | ClusterAction::Bootstrap { durable_root, .. }
            | ClusterAction::Sync { durable_root }
            | ClusterAction::Run { durable_root, .. }
            | ClusterAction::Transfer { durable_root, .. }
            | ClusterAction::Maintenance { durable_root } => durable_root.as_ref(),
        },
        Commands::Log { action } => match action {
            LogAction::List { durable_root, .. }
            | LogAction::Tail { durable_root, .. }
            | LogAction::Show { durable_root, .. }
            | LogAction::Query { durable_root, .. }
            | LogAction::Bundle { durable_root, .. } => Some(durable_root),
            LogAction::Export { durable_root } => durable_root.as_ref(),
        },
        Commands::Agent { .. } => None,
        Commands::System { action } => match action {
            SystemAction::Doctor { durable_root, .. } => durable_root.as_ref(),
            SystemAction::Install { .. }
            | SystemAction::Repair { .. }
            | SystemAction::Update { .. }
            | SystemAction::Rollback { .. }
            | SystemAction::Uninstall { .. }
            | SystemAction::Start { .. }
            | SystemAction::Stop { .. }
            | SystemAction::Restart { .. }
            | SystemAction::Status { .. }
            | SystemAction::ApiGroups => None,
        },
    }
    .filter(|path| !skipped_durable_root(path))
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

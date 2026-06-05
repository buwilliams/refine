use std::path::PathBuf;

use clap::Parser;
use serde_json::json;

use crate::core::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::core::host::cluster::{ClusterService, FileClusterRegistryService};
use crate::core::host::installation::{FileInstallationService, InstallationService};
use crate::core::host::process_supervision::FileProcessSupervisor;
use crate::core::observability::activity::{ActivityService, FileActivityService};
use crate::core::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::core::observability::support_bundle::{FileSupportBundleService, SupportBundleService};
use crate::core::product::imports::FileImportService;
use crate::core::product::nodes::FileNodeRegistryService;
use crate::core::product::project_registry::{FileProjectRegistryService, ProjectRegistryService};
use crate::core::product::project_state::{
    FileProjectStateStore, ProjectStateStore, ProjectionQuery, ProjectionSnapshot,
};
use crate::core::product::scheduling::{
    FileSchedulingService, SchedulerControl, SchedulingService,
};
use crate::core::product::work_items::{
    BulkGapFilter, BulkGapSelection, BulkGapUpdate, FileWorkItemService,
};
use crate::core::supervisor::errors::RefineResult;
use crate::core::supervisor::lifecycle::{DaemonLifecycleService, FileDaemonLifecycleService};
use crate::core::supervisor::runtime::RuntimeRoot;
use crate::model::project::RegisteredApp;
use crate::model::workflow::{GapStatus, user_status_transition};
use crate::surfaces::web_server::{API_GROUPS, InProcessWebServer, LocalHttpDaemon};

use super::actions::*;
use super::helpers::*;

pub fn run() -> RefineResult<()> {
    let cli = Cli::parse();
    dispatch(cli)
}

pub fn dispatch(cli: Cli) -> RefineResult<()> {
    match cli.command {
        Commands::Workflow {
            action: WorkflowAction::Allowed { from, to },
        } => {
            let from: GapStatus = from.into();
            let to: GapStatus = to.into();
            let decision = user_status_transition(&from, &to);
            println!("{}", serde_json::to_string_pretty(&decision).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action:
                WorkflowAction::Transition {
                    id,
                    target,
                    durable_root: Some(durable_root),
                },
        } => {
            let target: GapStatus = target.into();
            let gap = FileWorkItemService::new(durable_root).transition_gap_status(&id, target)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"gap": gap.gap})).unwrap()
            );
            Ok(())
        }
        Commands::Workflow {
            action:
                WorkflowAction::BulkTransition {
                    target,
                    durable_root: Some(durable_root),
                    selected_ids,
                    status,
                    q,
                },
        } => {
            let target: GapStatus = target.into();
            let selection = BulkGapSelection {
                filter: BulkGapFilter {
                    status,
                    q,
                    ..Default::default()
                },
                selected_ids: if selected_ids.is_empty() {
                    None
                } else {
                    Some(selected_ids)
                },
                exclude_ids: Vec::new(),
            };
            let result = FileWorkItemService::new(durable_root).bulk_update_gaps(
                selection,
                BulkGapUpdate::Status(target.as_str().to_string()),
            )?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action:
                WorkflowAction::Schedule {
                    durable_root,
                    runtime_root,
                },
        } => {
            let scheduler = FileSchedulingService::with_durable_root(runtime_root, durable_root);
            let promoted = scheduler.promote()?;
            let state = scheduler.load_state()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "promoted": promoted,
                    "reservations": state.reservations
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Workflow {
            action: WorkflowAction::Pause { runtime_root },
        } => {
            let scheduler = FileSchedulingService::new(&runtime_root);
            let supervisor = FileProcessSupervisor::new(runtime_root);
            supervisor.set_agents_paused(true)?;
            let state = supervisor.set_background_processes_stopped(true)?;
            scheduler.pause(SchedulerControl::Agents)?;
            println!("{}", serde_json::to_string_pretty(&state).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action: WorkflowAction::Resume { runtime_root },
        } => {
            let scheduler = FileSchedulingService::new(&runtime_root);
            let supervisor = FileProcessSupervisor::new(runtime_root);
            supervisor.set_background_processes_stopped(false)?;
            let state = supervisor.set_agents_paused(false)?;
            scheduler.resume(SchedulerControl::Agents)?;
            println!("{}", serde_json::to_string_pretty(&state).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action: WorkflowAction::Restore { durable_root },
        } => {
            let result = FileWorkItemService::new(durable_root).bulk_update_gaps(
                BulkGapSelection {
                    filter: BulkGapFilter::default(),
                    selected_ids: None,
                    exclude_ids: Vec::new(),
                },
                BulkGapUpdate::Status("__last_workflow_state".to_string()),
            )?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Workflow {
            action: WorkflowAction::Enforce { durable_root },
        } => {
            let snapshot = FileProjectStateStore::new(durable_root).rebuild_projection()?;
            let automated: Vec<_> = snapshot
                .gaps
                .values()
                .filter(|gap| {
                    matches!(
                        gap.gap.status,
                        GapStatus::InProgress
                            | GapStatus::Qa
                            | GapStatus::ReadyMerge
                            | GapStatus::AwaitingRebuild
                    )
                })
                .map(|gap| gap.gap.id.clone())
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "ok": true,
                    "checked": snapshot.gaps.len(),
                    "automated": automated
                }))
                .unwrap()
            );
            Ok(())
        }
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
                    target,
                    runtime_root,
                    version,
                },
        } => {
            let status = FileInstallationService::new(runtime_root, version)
                .install(target.into_target())?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Repair {
                    runtime_root,
                    version,
                },
        } => {
            let status = FileInstallationService::new(runtime_root, version).repair()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Update {
                    version,
                    runtime_root,
                },
        } => {
            let status = FileInstallationService::new(runtime_root, env!("CARGO_PKG_VERSION"))
                .update(&version)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Rollback {
                    runtime_root,
                    version,
                },
        } => {
            let status = FileInstallationService::new(runtime_root, version).rollback()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Uninstall {
                    runtime_root,
                    version,
                },
        } => {
            FileInstallationService::new(runtime_root, version).uninstall()?;
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
            action: NodeAction::List { durable_root },
        } => {
            let nodes = FileNodeRegistryService::new(durable_root).list_response()?;
            println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Show { id, durable_root },
        } => {
            let node = FileNodeRegistryService::new(durable_root).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Create { id, durable_root },
        } => {
            let node = FileNodeRegistryService::new(durable_root).create(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Activate { id, durable_root },
        } => {
            let nodes = FileNodeRegistryService::new(durable_root).activate(&id)?;
            println!("{}", serde_json::to_string_pretty(&nodes).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Archive { id, durable_root },
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
                    durable_root,
                },
        } => {
            let node = FileNodeRegistryService::new(durable_root).rename(&id, &name)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Node {
            action: NodeAction::Settings { id, durable_root },
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
            action: ClusterAction::List { durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).list_response()?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::Show { id, durable_root },
        } => {
            let node = FileClusterRegistryService::new(durable_root).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::AddNode { id, durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).add_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::EditNode { id, durable_root },
        } => {
            let node = FileClusterRegistryService::new(durable_root).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::EnableNode { id, durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).set_enabled(&id, true)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::DisableNode { id, durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).set_enabled(&id, false)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::RemoveNode { id, durable_root },
        } => {
            let cluster = FileClusterRegistryService::new(durable_root).remove_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&cluster).unwrap());
            Ok(())
        }
        Commands::Cluster {
            action: ClusterAction::Sync { durable_root },
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
                    durable_root,
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
                    durable_root,
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
            action: ClusterAction::Maintenance { durable_root },
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
            let service = FileActivityService::new(durable_root);
            let limit = service.count()?.max(1);
            let Some(entry) = service
                .query(limit, 0, None, None, None, None, None)?
                .into_iter()
                .find(|entry| entry.id == id)
            else {
                return Err(crate::core::supervisor::errors::RefineError::NotFound(
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
            action:
                LogAction::Bundle {
                    durable_root,
                    runtime_root,
                    repo_root,
                    redact_secrets,
                },
        } => {
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
            action: SystemAction::Start { port, runtime_root },
        } => {
            let status =
                FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root }).start(port)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Stop { port, runtime_root },
        } => {
            let status =
                FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root }).stop(port)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Restart { port, runtime_root },
        } => {
            let status = FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root })
                .restart(port)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::System {
            action: SystemAction::Status { port, runtime_root },
        } => {
            let status =
                FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root }).status(port)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
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
            let status =
                FileProjectRegistryService::new(runtime_root, durable_root).attach(&path)?;
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
            let status =
                FileProjectRegistryService::new(runtime_root, durable_root).switch(&name)?;
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
            let path = absolutize_cli_path(&path)?;
            let timestamp = cli_timestamp();
            let registry = FileProjectRegistryService::new(runtime_root, durable_root).register(
                RegisteredApp {
                    name,
                    path: path.display().to_string(),
                    added_at: timestamp,
                    last_used_at: None,
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&registry).unwrap());
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
            let status = FileProjectRegistryService::new(runtime_root, durable_root).status()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "ok": true,
                    "migrated": false,
                    "schema": status.schema,
                    "message": "project schema is already compatible"
                }))
                .unwrap()
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
            let store = FileProjectStateStore::new(&durable_root);
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
                    actual.as_deref(),
                    target.as_deref(),
                )?
            } else {
                let Some(reporter) = reporter.as_deref() else {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "round reporter is required".to_string(),
                    ));
                };
                let Some(actual) = actual.as_deref() else {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "round actual is required".to_string(),
                    ));
                };
                let Some(target) = target.as_deref() else {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "round target is required".to_string(),
                    ));
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
            let service = FileWorkItemService::new(durable_root);
            let current = service.show_gap_summary(&id)?;
            if current.gap.status == GapStatus::Backlog {
                service.transition_gap_status(&id, GapStatus::Todo)?;
            }
            let gap = service.start_gap_summary(&id)?;
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
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "retry stage must be quality or merge".to_string(),
                    ));
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
                return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                    format!("Gap {id} is not assigned to a Feature"),
                ));
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
                return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                    "target must be backlog or todo".to_string(),
                ));
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
            let service = FileImportService::new(durable_root);
            let result = if let Some(file) = file {
                service.import_from_file(file, csv, reporter.as_deref(), feature_id.as_deref())?
            } else {
                let Some(text) = text.as_deref() else {
                    return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
                        "feature import requires --text or --file".to_string(),
                    ));
                };
                service.import_from_text(text, csv, reporter.as_deref(), feature_id.as_deref())?
            };
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::System {
            action:
                SystemAction::Serve {
                    port,
                    cache_dir,
                    static_root,
                    runtime_root,
                    token,
                    once,
                },
        } => {
            let project_status = FileProjectRegistryService::new(&runtime_root, None).status()?;
            let snapshot = if let Some(client_repo) = project_status.client_repo {
                let durable_root = PathBuf::from(client_repo).join(".refine");
                let store = FileProjectStateStore::new(&durable_root);
                let snapshot = store.rebuild_projection()?;
                if let Some(cache_dir) = &cache_dir {
                    store.persist_projection_snapshot(cache_dir, &snapshot)?;
                }
                snapshot
            } else {
                ProjectionSnapshot::default()
            };
            let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot {
                root: runtime_root.clone(),
            });
            let status = lifecycle.start(port)?;
            let daemon = LocalHttpDaemon {
                server: InProcessWebServer {
                    status,
                    projection: snapshot,
                    auth_token: token,
                    durable_root: None,
                    runtime_root: Some(runtime_root),
                },
                static_root: static_root.or_else(default_static_root),
            };
            let listener = LocalHttpDaemon::bind_loopback(port)?;
            let addr = LocalHttpDaemon::local_addr(&listener)?;
            eprintln!("serving Refine daemon API at http://{addr}");
            if once {
                daemon.serve_next(&listener)?;
            } else {
                loop {
                    daemon.serve_next(&listener)?;
                }
            }
            Ok(())
        }
        other => Err(crate::core::supervisor::errors::RefineError::InvalidInput(
            format!(
                "CLI command is missing required options or cannot run in this mode: {other:?}"
            ),
        )),
    }
}

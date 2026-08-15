use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Fleet {
            action: FleetAction::Request(words),
        } => dispatch_request(words),
        Commands::Fleet {
            action:
                FleetAction::Manage {
                    request,
                    provider,
                    runtime_root,
                    port,
                },
        } => dispatch_manage(request, provider, runtime_root, port),
        Commands::Fleet {
            action:
                FleetAction::Distribute {
                    instructions: Some(instructions),
                    ..
                },
        } => dispatch_distribute_instructions(instructions),
        Commands::Fleet {
            action:
                FleetAction::List {
                    target_root: Some(target_root),
                },
        } => {
            let fleet =
                FileFleetService::new(refine_dir_for_target_root(&target_root)?).list_response()?;
            println!("{}", serde_json::to_string_pretty(&fleet).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::Show {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let node =
                FileFleetService::new(refine_dir_for_target_root(&target_root)?).show(&id)?;
            println!("{}", serde_json::to_string_pretty(&node).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::AddNode {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let fleet =
                FileFleetService::new(refine_dir_for_target_root(&target_root)?).add_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&fleet).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::EditNode {
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
                    target_root: Some(target_root),
                },
        } => {
            let fleet = FileFleetService::new(refine_dir_for_target_root(&target_root)?)
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
                        enabled,
                    },
                )?;
            println!("{}", serde_json::to_string_pretty(&fleet).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::EnableNode {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let fleet = FileFleetService::new(refine_dir_for_target_root(&target_root)?)
                .set_enabled(&id, true)?;
            println!("{}", serde_json::to_string_pretty(&fleet).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::DisableNode {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let fleet = FileFleetService::new(refine_dir_for_target_root(&target_root)?)
                .set_enabled(&id, false)?;
            println!("{}", serde_json::to_string_pretty(&fleet).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::RemoveNode {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let fleet = FileFleetService::new(refine_dir_for_target_root(&target_root)?)
                .remove_node(&id)?;
            println!("{}", serde_json::to_string_pretty(&fleet).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::Bootstrap {
                    id,
                    dry_run,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileFleetService::new(refine_dir_for_target_root(&target_root)?)
                .bootstrap_node_response(&id, dry_run)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::Distribute {
                    instructions: None,
                    to,
                    converge,
                    dry_run,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileFleetService::new(refine_dir_for_target_root(&target_root)?)
                .distribute_response(to.as_deref(), converge, dry_run)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::Sync {
                    target_root: Some(target_root),
                },
        } => {
            let runtime_root = refine_dir_for_target_root(&target_root)?.join("runtime");
            let sync = FileGitSyncService::new(&target_root, runtime_root).sync()?;
            println!("{}", serde_json::to_string_pretty(&sync).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::Run {
                    id,
                    command,
                    target_root: Some(target_root),
                },
        } => {
            let result = FileFleetService::new(refine_dir_for_target_root(&target_root)?)
                .run_remote(&id, &command)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::Transfer {
                    id,
                    item_id,
                    target_root: Some(target_root),
                },
        } => {
            let service = FileFleetService::new(refine_dir_for_target_root(&target_root)?);
            service.transfer(&item_id, &id)?;
            let result = direct_work_item_service(&target_root)?
                .transfer_item_to_node(&id, &item_id)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Fleet {
            action:
                FleetAction::Maintenance {
                    target_root: Some(target_root),
                },
        } => {
            let maintenance = FileFleetService::new(refine_dir_for_target_root(&target_root)?)
                .maintenance_response()?;
            println!("{}", serde_json::to_string_pretty(&maintenance).unwrap());
            Ok(())
        }
        Commands::Fleet { action } => dispatch_fleet_daemon(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

/// `refine fleet "<request>"`: free text in place of a subcommand delegates to
/// the same agent session as `fleet manage`. A single word is almost always a
/// mistyped subcommand, so it is rejected instead of spawning an agent.
pub(super) fn dispatch_request(words: Vec<String>) -> RefineResult<()> {
    let request = words.join(" ");
    let request = request.trim();
    if request.is_empty() || !request.contains(char::is_whitespace) {
        return Err(RefineError::InvalidInput(format!(
            "unknown fleet command {request:?}; to delegate a request to an agent, quote a plain-language sentence: refine fleet \"<request>\""
        )));
    }
    dispatch_manage(request.to_string(), None, PathBuf::from("run"), 8082)
}

/// `refine fleet distribute "<instructions>"`: agent-directed distribution.
/// The agent reassigns Goals to nodes per the instructions and syncs state —
/// no machines are created and no SSH is involved.
pub(super) fn dispatch_distribute_instructions(instructions: String) -> RefineResult<()> {
    dispatch_manage(
        format!(
            "Distribute work across the existing fleet nodes as instructed; reassign Goals to \
             nodes and sync state, without creating machines or connecting over SSH. \
             Instructions: {instructions}"
        ),
        None,
        PathBuf::from("run"),
        8082,
    )
}

/// Open an interactive session with the resolved agent provider, seeded with
/// the manage-fleet runbook and the user's request. The provider owns the
/// conversation; Refine owns the working directory and the runbook contract.
pub(super) fn dispatch_manage(
    request: String,
    provider: Option<String>,
    runtime_root: PathBuf,
    port: u16,
) -> RefineResult<()> {
    let runtime_root = resolve_system_runtime_root(runtime_root)?;
    let checkout = discover_refine_checkout()?;
    if !checkout.join(FLEET_RUNBOOK_PATH).is_file() {
        return Err(RefineError::NotFound(format!(
            "fleet runbook not found: {}",
            checkout.join(FLEET_RUNBOOK_PATH).display()
        )));
    }
    let provider = resolve_agent_provider(&runtime_root, provider)?;
    let prompt = fleet_manage_prompt(&checkout, &request);
    let launch = HostAgentProviderService::with_runtime_root(runtime_root.join(port.to_string()))
        .interactive_command(&provider, &prompt)?;
    launch.validate_prompt_artifact()?;
    eprintln!(
        "refine: opening {} to manage the fleet (guided by {FLEET_RUNBOOK_PATH})",
        launch.display_name
    );
    let mut command = std::process::Command::new(&launch.binary);
    command.args(&launch.args).current_dir(&checkout);
    launch.launch_environment.apply_to_command(&mut command);
    let status = command.status().map_err(|error| {
        RefineError::Io(format!(
            "failed to launch {} for fleet manage: {error}",
            launch.binary
        ))
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(RefineError::Degraded(format!(
            "fleet manage agent session exited with {status}"
        )))
    }
}

pub(super) fn dispatch_fleet_daemon(action: FleetAction) -> RefineResult<()> {
    let response = match action {
        FleetAction::List { target_root: None } => daemon_json("GET", "/fleet", None)?,
        FleetAction::Show {
            id,
            target_root: None,
        } => {
            let fleet = daemon_json("GET", "/fleet", None)?;
            let node = fleet
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
        FleetAction::AddNode {
            id,
            target_root: None,
        } => daemon_json("POST", "/fleet/nodes", Some(json!({ "id": id })))?,
        FleetAction::EditNode {
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
            target_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/fleet/nodes/{}", path_segment(&id)),
            Some(remote_node_edit_body(
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
        FleetAction::EnableNode {
            id,
            target_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/fleet/nodes/{}", path_segment(&id)),
            Some(json!({ "enabled": true })),
        )?,
        FleetAction::DisableNode {
            id,
            target_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/fleet/nodes/{}", path_segment(&id)),
            Some(json!({ "enabled": false })),
        )?,
        FleetAction::RemoveNode {
            id,
            target_root: None,
        } => daemon_json(
            "DELETE",
            &format!("/fleet/nodes/{}", path_segment(&id)),
            None,
        )?,
        FleetAction::Bootstrap {
            id,
            dry_run,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/fleet/nodes/{}/bootstrap", path_segment(&id)),
            Some(json!({ "dry_run": dry_run })),
        )?,
        FleetAction::Run {
            id,
            command,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/fleet/nodes/{}/run", path_segment(&id)),
            Some(json!({ "command": command })),
        )?,
        FleetAction::Distribute {
            instructions: None,
            to,
            converge,
            dry_run,
            target_root: None,
        } => daemon_json(
            "POST",
            "/fleet/distribute",
            Some(json!({ "to": to, "converge": converge, "dry_run": dry_run })),
        )?,
        FleetAction::Transfer {
            id,
            item_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/fleet/nodes/{}/transfer", path_segment(&id)),
            Some(json!({ "item_id": item_id })),
        )?,
        FleetAction::Maintenance { target_root: None } => {
            let fleet = daemon_json("GET", "/fleet", None)?;
            json!({
                "ok": true,
                "maintenance": {
                    "active": true,
                    "updated_at": fleet.get("updated_at").cloned().unwrap_or(serde_json::Value::Null)
                },
                "fleet": fleet
            })
        }
        FleetAction::Sync { target_root: None } => {
            follow_daemon_operation(daemon_json("POST", "/project/sync", None)?)?
        }
        other => {
            return Err(RefineError::NotImplemented(format!(
                "Fleet command is not available through the daemon API yet: {other:?}"
            )));
        }
    };
    print_json(&response);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn remote_node_edit_body(
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

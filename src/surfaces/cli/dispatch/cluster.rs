use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Cluster {
            action:
                ClusterAction::List {
                    target_root: Some(target_root),
                },
        } => {
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
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
                FileClusterService::new(refine_dir_for_target_root(&target_root)?).show(&id)?;
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
                FileClusterService::new(refine_dir_for_target_root(&target_root)?).add_node(&id)?;
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
                    target_root: Some(target_root),
                },
        } => {
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
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
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
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
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
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
            let cluster = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
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
            let result = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
                .bootstrap_node_response(&id, dry_run)?;
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
            let result = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
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
            let runtime_root = refine_dir_for_target_root(&target_root)?.join("runtime");
            let sync = FileGitSyncService::new(&target_root, runtime_root).sync()?;
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
            let result = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
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
            let service = FileClusterService::new(refine_dir_for_target_root(&target_root)?);
            service.transfer(&item_id, &id)?;
            let result = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
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
            let maintenance = FileClusterService::new(refine_dir_for_target_root(&target_root)?)
                .maintenance_response()?;
            println!("{}", serde_json::to_string_pretty(&maintenance).unwrap());
            Ok(())
        }
        Commands::Cluster { action } => dispatch_cluster_daemon(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn dispatch_cluster_daemon(action: ClusterAction) -> RefineResult<()> {
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
            enabled,
            target_root: None,
        } => daemon_json(
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
                enabled,
            )),
        )?,
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
        ClusterAction::Sync { target_root: None } => daemon_json("POST", "/project/sync", None)?,
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

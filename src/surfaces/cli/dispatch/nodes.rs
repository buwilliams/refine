use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Node {
            action: NodeAction::List {
                target_root: Some(target_root),
            },
        } => {
            let nodes = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root)?)
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
            let node = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root)?)
                .show(&id)?;
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
            let node = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root)?)
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
            let nodes = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root)?)
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
            let node = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root)?)
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
            let node = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root)?)
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
            let settings = FileNodeRegistryService::new(refine_dir_for_target_root(&target_root)?)
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
            let result = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .transfer_item_to_node(&id, &item_id)?;
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Node { action } => dispatch_node_daemon(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn dispatch_node_daemon(action: NodeAction) -> RefineResult<()> {
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
            "/nodes/transfer-goals",
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

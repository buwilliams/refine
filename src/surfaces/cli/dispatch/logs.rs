use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
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
            let entries = FileActivityService::new(refine_dir_for_target_root(&target_root)?)
                .recent(limit)?;
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
            let entries = FileActivityService::new(refine_dir_for_target_root(&target_root)?)
                .recent(limit)?;
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
            let service = FileActivityService::new(refine_dir_for_target_root(&target_root)?);
            let limit = service.count()?.max(1);
            let Some(entry) = service
                .query(ActivityQuery {
                    limit,
                    ..ActivityQuery::default()
                })?
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
                    goal_id,
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
                if let Some(value) = goal_id {
                    query.push(format!("goal_id={}", query_component(&value)));
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
            let entries = FileActivityService::new(refine_dir_for_target_root(&target_root)?)
                .query(ActivityQuery {
                    limit,
                    offset,
                    goal_id: goal_id.as_deref(),
                    severity: severity.as_deref(),
                    category: category.as_deref(),
                    actor: actor.as_deref(),
                    q: Some(&q),
                })?;
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
            let service = FileActivityService::new(refine_dir_for_target_root(&target_root)?);
            let limit = service.count()?;
            let entries = if limit == 0 {
                Vec::new()
            } else {
                service.query(ActivityQuery {
                    limit,
                    ..ActivityQuery::default()
                })?
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
                refine_dir_for_target_root(&target_root)?,
                runtime_root,
                repo_root,
            )
            .export(redact_secrets)?;
            println!("{}", serde_json::to_string_pretty(&bundle).unwrap());
            Ok(())
        }
        _ => unreachable!("command family was routed incorrectly"),
    }
}

#[cfg(not(test))]
pub(super) fn dispatch_log_daemon(action: LogAction) -> RefineResult<()> {
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
            goal_id,
            severity,
            category,
            actor,
        } if skipped_target_root(&target_root) => {
            let mut query = vec![
                format!("limit={limit}"),
                format!("offset={offset}"),
                format!("q={}", query_component(&q)),
            ];
            if let Some(value) = goal_id {
                query.push(format!("goal_id={}", query_component(&value)));
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

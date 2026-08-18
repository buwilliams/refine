use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
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
            let feature = direct_work_item_service(&target_root)?.create_feature_summary(
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
                    "goal_ids": feature.goal_ids,
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
            let feature = direct_work_item_service(&target_root)?.update_feature_metadata_summary(
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
                    "goal_ids": feature.goal_ids,
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
            let features: Vec<_> = direct_work_item_service(&target_root)?
                .list_feature_summaries()?
                .into_iter()
                .map(|feature| {
                    json!({
                        "feature": feature.feature,
                        "goal_ids": feature.goal_ids,
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
            let feature = direct_work_item_service(&target_root)?.show_feature_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::AddGoal {
                    id,
                    goal_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature =
                direct_work_item_service(&target_root)?.assign_goal_to_feature(&id, &goal_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::RemoveGoal {
                    id,
                    goal_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature =
                direct_work_item_service(&target_root)?.remove_goal_from_feature(&id, &goal_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::ReorderGoal {
                    id,
                    goal_id,
                    order,
                    target_root: Some(target_root),
                },
        } => {
            let feature = direct_work_item_service(&target_root)?
                .reorder_goal_in_feature(&id, &goal_id, order)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::OrderGoal {
                    id,
                    goal_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature =
                direct_work_item_service(&target_root)?.order_goal_in_feature(&id, &goal_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
                    "rollup": feature.rollup
                }))
                .unwrap()
            );
            Ok(())
        }
        Commands::Feature {
            action:
                FeatureAction::UnorderGoal {
                    id,
                    goal_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature =
                direct_work_item_service(&target_root)?.unorder_goal_in_feature(&id, &goal_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
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
            let Some(target) = GoalStatus::parse_wire(&target) else {
                return Err(crate::error::RefineError::InvalidInput(
                    "target must be backlog or todo".to_string(),
                ));
            };
            let feature =
                direct_work_item_service(&target_root)?.move_feature_workflow(&id, target)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
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
            let result =
                direct_work_item_service(&target_root)?.transfer_feature_to_node(&node_id, &id)?;
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
            let feature = direct_work_item_service(&target_root)?.cancel_feature_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "feature": feature.feature,
                    "goal_ids": feature.goal_ids,
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
            direct_work_item_service(&target_root)?.delete_feature_record(&id)?;
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
            let service = FileImportService::new(refine_dir_for_target_root(&target_root)?);
            let result = if let Some(file) = file {
                service.import_from_file(file, csv, reporter.as_deref(), feature_id.as_deref())?
            } else {
                let Some(text) = text.as_deref() else {
                    return Err(crate::error::RefineError::InvalidInput(
                        "feature import requires --text or --file".to_string(),
                    ));
                };
                service.import_from_text(text, csv, reporter.as_deref(), feature_id.as_deref())?
            };
            println!("{}", serde_json::to_string_pretty(&result).unwrap());
            Ok(())
        }
        Commands::Feature { action } => dispatch_feature_daemon(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn dispatch_feature_daemon(action: FeatureAction) -> RefineResult<()> {
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
        FeatureAction::AddGoal {
            id,
            goal_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/goals/{}",
                path_segment(&id),
                path_segment(&goal_id)
            ),
            None,
        )?,
        FeatureAction::RemoveGoal {
            id,
            goal_id,
            target_root: None,
        } => daemon_json(
            "DELETE",
            &format!(
                "/work/features/{}/goals/{}",
                path_segment(&id),
                path_segment(&goal_id)
            ),
            None,
        )?,
        FeatureAction::ReorderGoal {
            id,
            goal_id,
            order,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/goals/{}/reorder",
                path_segment(&id),
                path_segment(&goal_id)
            ),
            Some(json!({ "order": order })),
        )?,
        FeatureAction::OrderGoal {
            id,
            goal_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/goals/{}/order",
                path_segment(&id),
                path_segment(&goal_id)
            ),
            None,
        )?,
        FeatureAction::UnorderGoal {
            id,
            goal_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/goals/{}/unorder",
                path_segment(&id),
                path_segment(&goal_id)
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

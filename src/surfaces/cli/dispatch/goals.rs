use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Goal {
            action:
                GoalAction::Create {
                    name,
                    target_root: Some(target_root),
                    id,
                },
        } => {
            let goal = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .create_goal_summary(&name, id.as_deref())?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action: GoalAction::List {
                target_root: Some(target_root),
            },
        } => {
            let goals: Vec<_> = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .list_goal_summaries()?
                .into_iter()
                .map(|goal| goal.goal)
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goals": goals})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Show {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let goal = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .show_goal_detail(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Export {
                    id,
                    target_root: Some(target_root),
                    output,
                },
        } => {
            let refine_dir = refine_dir_for_target_root(&target_root)?;
            let export =
                FileGoalExportService::new(refine_dir, &target_root).export_jira_csv(&id)?;
            write_goal_export(&export.csv, &export.filename, output.as_deref())
        }
        Commands::Goal {
            action:
                GoalAction::Edit {
                    id,
                    target_root: Some(target_root),
                    name,
                    priority,
                },
        } => {
            let goal = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .update_goal_metadata_summary(
                    &id,
                    name.as_deref(),
                    priority.as_deref(),
                    None,
                    None,
                )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Note {
                    id,
                    body,
                    target_root: Some(target_root),
                    author,
                },
        } => {
            let goal = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .add_goal_note_summary(&id, &author, &body)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::NoteEdit {
                    id,
                    note_id,
                    body,
                    target_root: Some(target_root),
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?);
            let detail = service.show_goal_detail(&id)?;
            let notes = edit_goal_note_values(goal_notes_from_detail(&detail), &note_id, &body)?;
            let goal = service.replace_goal_notes_summary(&id, &notes)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::NoteDelete {
                    id,
                    note_id,
                    target_root: Some(target_root),
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?);
            let detail = service.show_goal_detail(&id)?;
            let notes = delete_goal_note_values(goal_notes_from_detail(&detail), &note_id)?;
            let goal = service.replace_goal_notes_summary(&id, &notes)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Round {
                    id,
                    target_root: Some(target_root),
                    reporter,
                    prompt,
                    edit_latest,
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?);
            let goal = if edit_latest {
                service.edit_latest_goal_round_summary(
                    &id,
                    reporter.as_deref(),
                    None,
                    prompt.as_deref(),
                )?
            } else {
                let Some(reporter) = reporter.as_deref() else {
                    return Err(
                        crate::process::supervisor::errors::RefineError::InvalidInput(
                            "round reporter is required".to_string(),
                        ),
                    );
                };
                let Some(prompt) = prompt.as_deref() else {
                    return Err(
                        crate::process::supervisor::errors::RefineError::InvalidInput(
                            "round prompt is required".to_string(),
                        ),
                    );
                };
                service.append_goal_round_summary(&id, reporter, prompt)?
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Delete {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .delete_goal_record(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"deleted": true, "id": id})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Cancel {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let goal = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .cancel_goal_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Start {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let goal = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .start_goal_workflow(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Retry {
                    id,
                    target_root: Some(target_root),
                    stage,
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?);
            let goal = match stage.as_str() {
                "quality" | "qa" => service.retry_goal_quality_summary(&id)?,
                "merge" => service.retry_goal_merge_summary(&id)?,
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
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Approve {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let refine_dir = refine_dir_for_target_root(&target_root)?;
            let goal = FileMergerService::with_target_root(
                refine_dir.join("runtime"),
                &refine_dir,
                &target_root,
            )
            .approve_reviewed_goal(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Merge {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let refine_dir = refine_dir_for_target_root(&target_root)?;
            let goal = FileMergerService::with_target_root(
                refine_dir.join("runtime"),
                &refine_dir,
                &target_root,
            )
            .approve_reviewed_goal(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::Undo {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let goal = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .undo_goal_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal.goal})).unwrap()
            );
            Ok(())
        }
        Commands::Goal {
            action:
                GoalAction::AssignFeature {
                    id,
                    feature_id,
                    target_root: Some(target_root),
                },
        } => {
            let feature = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?)
                .assign_goal_to_feature(&feature_id, &id)?;
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
        Commands::Goal {
            action:
                GoalAction::RemoveFeature {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let service = FileWorkItemService::new(refine_dir_for_target_root(&target_root)?);
            let current = service.show_goal_summary(&id)?;
            let Some(feature_id) = current.goal.feature_id.clone() else {
                return Err(
                    crate::process::supervisor::errors::RefineError::InvalidInput(format!(
                        "Goal {id} is not assigned to a Feature"
                    )),
                );
            };
            let feature = service.remove_goal_from_feature(&feature_id, &id)?;
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
        Commands::Goal { action } => dispatch_goal_daemon(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn dispatch_goal_daemon(action: GoalAction) -> RefineResult<()> {
    let response = match action {
        GoalAction::Create {
            name,
            target_root: None,
            id,
        } => daemon_json(
            "POST",
            "/work/goals",
            Some(json!({
                "name": name,
                "id": id
            })),
        )?,
        GoalAction::Draft {
            target_root: None,
            text,
            file,
            reporter,
            provider,
        } => daemon_json(
            "POST",
            "/import/extract",
            Some(plan_goal_draft_body(text, file, reporter, provider)?),
        )?,
        GoalAction::List { target_root: None } => {
            daemon_json("GET", "/work/goals?limit=1000", None)?
        }
        GoalAction::Show {
            id,
            target_root: None,
        } => daemon_json("GET", &format!("/work/goals/{}", path_segment(&id)), None)?,
        GoalAction::Export {
            id,
            target_root: None,
            output,
        } => {
            let response = daemon_json(
                "GET",
                &format!("/work/goals/{}/export/jira", path_segment(&id)),
                None,
            )?;
            let export = response.get("export").ok_or_else(|| {
                RefineError::Serialization(
                    "Goal Jira export response is missing export".to_string(),
                )
            })?;
            let csv = export.get("csv").and_then(Value::as_str).ok_or_else(|| {
                RefineError::Serialization(
                    "Goal Jira export response is missing CSV content".to_string(),
                )
            })?;
            let filename = export
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("refine-goal-jira.csv");
            return write_goal_export(csv, filename, output.as_deref());
        }
        GoalAction::Edit {
            id,
            target_root: None,
            name,
            priority,
        } => daemon_json(
            "PATCH",
            &format!("/work/goals/{}", path_segment(&id)),
            Some(json!({
                "name": name,
                "priority": priority
            })),
        )?,
        GoalAction::Note {
            id,
            body,
            target_root: None,
            author,
        } => daemon_json(
            "POST",
            &format!("/work/goals/{}/notes", path_segment(&id)),
            Some(json!({
                "body": body,
                "author": author
            })),
        )?,
        GoalAction::NoteEdit {
            id,
            note_id,
            body,
            target_root: None,
        } => {
            let detail = daemon_json("GET", &format!("/work/goals/{}", path_segment(&id)), None)?;
            let notes =
                edit_goal_note_values(goal_notes_from_detail(&detail["goal"]), &note_id, &body)?;
            daemon_json(
                "PATCH",
                &format!("/work/goals/{}", path_segment(&id)),
                Some(json!({ "notes": notes })),
            )?
        }
        GoalAction::NoteDelete {
            id,
            note_id,
            target_root: None,
        } => {
            let detail = daemon_json("GET", &format!("/work/goals/{}", path_segment(&id)), None)?;
            let notes = delete_goal_note_values(goal_notes_from_detail(&detail["goal"]), &note_id)?;
            daemon_json(
                "PATCH",
                &format!("/work/goals/{}", path_segment(&id)),
                Some(json!({ "notes": notes })),
            )?
        }
        GoalAction::Round {
            id,
            target_root: None,
            reporter,
            prompt,
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
                &format!("/work/goals/{}{}", path_segment(&id), suffix),
                Some(json!({
                    "reporter": reporter,
                    "prompt": prompt
                })),
            )?
        }
        GoalAction::Start {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/goals/{}/start", path_segment(&id)),
            None,
        )?,
        GoalAction::Cancel {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/goals/{}/cancel", path_segment(&id)),
            None,
        )?,
        GoalAction::Retry {
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
                &format!("/work/goals/{}/{}", path_segment(&id), action),
                None,
            )?
        }
        GoalAction::Verify {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/goals/{}/verify", path_segment(&id)),
            None,
        )?,
        GoalAction::Approve {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/goals/{}/approve", path_segment(&id)),
            None,
        )?,
        GoalAction::Merge {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/goals/{}/merge", path_segment(&id)),
            None,
        )?,
        GoalAction::Undo {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/goals/{}/undo", path_segment(&id)),
            None,
        )?,
        GoalAction::Delete {
            id,
            target_root: None,
        } => daemon_json(
            "DELETE",
            &format!("/work/goals/{}", path_segment(&id)),
            None,
        )?,
        GoalAction::AssignFeature {
            id,
            feature_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/features/{}/goals/{}",
                path_segment(&feature_id),
                path_segment(&id)
            ),
            None,
        )?,
        GoalAction::RemoveFeature {
            id,
            target_root: None,
        } => {
            let current = daemon_json("GET", &format!("/work/goals/{}", path_segment(&id)), None)?;
            let feature_id = current
                .get("goal")
                .and_then(|goal| goal.get("feature_id"))
                .and_then(|value| value.as_str())
                .ok_or_else(|| {
                    RefineError::Conflict(format!("Goal {id} is not assigned to a Feature"))
                })?;
            daemon_json(
                "DELETE",
                &format!(
                    "/work/features/{}/goals/{}",
                    path_segment(feature_id),
                    path_segment(&id)
                ),
                None,
            )?
        }
        other => {
            return Err(RefineError::InvalidInput(format!(
                "Goal command cannot be routed to the daemon in this mode: {other:?}"
            )));
        }
    };
    print_json(&response);
    Ok(())
}

pub(super) fn write_goal_export(
    csv: &str,
    filename: &str,
    output: Option<&Path>,
) -> RefineResult<()> {
    let Some(output) = output else {
        print!("{csv}");
        return Ok(());
    };
    fs::write(output, csv).map_err(|error| {
        RefineError::Io(format!(
            "failed to write Jira export {}: {error}",
            output.display()
        ))
    })?;
    print_json(&json!({
        "exported": true,
        "filename": filename,
        "path": output.display().to_string()
    }));
    Ok(())
}

pub(super) fn goal_notes_from_detail(detail: &Value) -> Vec<Value> {
    detail
        .get("notes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn edit_goal_note_values(
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

pub(super) fn delete_goal_note_values(
    notes: Vec<Value>,
    note_id: &str,
) -> RefineResult<Vec<Value>> {
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

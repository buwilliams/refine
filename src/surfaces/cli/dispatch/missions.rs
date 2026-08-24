use super::*;

use crate::model::mission::MissionStatus;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Mission {
            action:
                MissionAction::Create {
                    name,
                    intent,
                    file,
                    reporter,
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let intent = resolve_intent(intent, file)?;
            let mission = direct_mission_service(&target_root)?.create_mission(
                &name,
                &intent,
                reporter.as_deref(),
                None,
                id.as_deref(),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::List {
                    target_root: Some(target_root),
                },
        } => {
            let missions = direct_mission_service(&target_root)?.list_mission_projections()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"missions": missions})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Show {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.show_mission(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Edit {
                    id,
                    name,
                    intent,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.edit_mission_frame(
                &id,
                name.as_deref(),
                intent.as_deref(),
                None,
                None,
                None,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Round {
                    id,
                    reporter,
                    prompt,
                    target_root: Some(target_root),
                },
        } => {
            let Some(prompt) = prompt.as_deref() else {
                return Err(RefineError::InvalidInput(
                    "round prompt is required".to_string(),
                ));
            };
            let mission = direct_mission_service(&target_root)?.append_round(
                &id,
                reporter.as_deref().unwrap_or(""),
                prompt,
                None,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Start {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.transition_mission(
                &id,
                MissionStatus::Investigate,
                None,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::ApprovePlan {
                    id,
                    plan_digest,
                    target_root: Some(target_root),
                },
        } => {
            let service = direct_mission_service(&target_root)?;
            let mission = service.approve_plan(&id, &plan_digest, "", "", None)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::ApproveOutcome {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.transition_mission(
                &id,
                MissionStatus::Consolidate,
                None,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Cancel {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.transition_mission(
                &id,
                MissionStatus::Cancelled,
                None,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Outcome {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.show_mission(&id)?;
            let outcome = mission
                .rounds
                .iter()
                .rev()
                .find_map(|round| round.outcome.clone())
                .ok_or_else(|| {
                    RefineError::NotFound(format!("Mission {id} has no published Outcome"))
                })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"outcome": outcome})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Contribute {
                    goal,
                    file,
                    target_root: Some(target_root),
                },
        } => {
            let contribution = fs::read_to_string(&file).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read contribution file {}: {error}",
                    file.display()
                ))
            })?;
            let contribution: crate::model::mission::GoalContribution =
                serde_json::from_str(&contribution).map_err(|error| {
                    RefineError::InvalidInput(format!(
                        "contribution file {} is not valid contribution JSON: {error}",
                        file.display()
                    ))
                })?;
            let refine_dir =
                crate::infrastructure::storage::project_layout::refine_dir_for_target_root(
                    &target_root,
                )?;
            let work_items = crate::application::work_items::FileWorkItemService::new(&refine_dir);
            let goal = work_items.settle_goal_mission_contribution(&goal, contribution)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"goal": goal})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Decide {
                    id,
                    decision_id,
                    choice,
                    rationale,
                    actor,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.answer_decision(
                &id,
                &decision_id,
                &choice,
                &rationale,
                actor.as_deref().unwrap_or(""),
                None,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Retry {
                    id,
                    stage,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.retry_stage(&id, &stage, None)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Transfer {
                    id,
                    node_id,
                    target_root: Some(target_root),
                },
        } => {
            let mission =
                direct_mission_service(&target_root)?.transfer_mission(&id, &node_id, None)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::AddGoal {
                    id,
                    goal_id,
                    wave,
                    role,
                    optional,
                    criteria,
                    target_root: Some(target_root),
                },
        } => {
            let mission = direct_mission_service(&target_root)?.add_plan_goal(
                &id,
                &goal_id,
                wave,
                role.as_deref(),
                !optional,
                &criteria,
                None,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::RemoveGoal {
                    id,
                    goal_id,
                    target_root: Some(target_root),
                },
        } => {
            let mission =
                direct_mission_service(&target_root)?.remove_plan_goal(&id, &goal_id, None)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"mission": mission})).unwrap()
            );
            Ok(())
        }
        Commands::Mission {
            action:
                MissionAction::Context {
                    id,
                    target_root: Some(target_root),
                },
        } => {
            let context = direct_mission_service(&target_root)?.mission_context_summary(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"context": context})).unwrap()
            );
            Ok(())
        }
        Commands::Mission { action } => dispatch_mission_daemon(action),
        _ => unreachable!("command family was routed incorrectly"),
    }
}

pub(super) fn dispatch_mission_daemon(action: MissionAction) -> RefineResult<()> {
    let response = match action {
        MissionAction::Create {
            name,
            intent,
            file,
            reporter,
            id,
            target_root: None,
        } => {
            let intent = resolve_intent(intent, file)?;
            daemon_json(
                "POST",
                "/work/missions",
                Some(json!({
                    "name": name,
                    "intent": intent,
                    "reporter": reporter,
                    "id": id
                })),
            )?
        }
        MissionAction::List { target_root: None } => {
            daemon_json("GET", "/work/missions?limit=1000", None)?
        }
        MissionAction::Show {
            id,
            target_root: None,
        } => daemon_json(
            "GET",
            &format!("/work/missions/{}", path_segment(&id)),
            None,
        )?,
        MissionAction::Edit {
            id,
            name,
            intent,
            target_root: None,
        } => daemon_json(
            "PATCH",
            &format!("/work/missions/{}", path_segment(&id)),
            Some(json!({
                "name": name,
                "intent": intent
            })),
        )?,
        MissionAction::Round {
            id,
            reporter,
            prompt,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/rounds", path_segment(&id)),
            Some(json!({
                "reporter": reporter,
                "prompt": prompt
            })),
        )?,
        MissionAction::Start {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/start", path_segment(&id)),
            None,
        )?,
        MissionAction::ApprovePlan {
            id,
            plan_digest,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/approve-plan", path_segment(&id)),
            Some(json!({ "plan_digest": plan_digest })),
        )?,
        MissionAction::ApproveOutcome {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/approve-outcome", path_segment(&id)),
            None,
        )?,
        MissionAction::Cancel {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/cancel", path_segment(&id)),
            None,
        )?,
        MissionAction::Advance {
            id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/advance", path_segment(&id)),
            None,
        )?,
        MissionAction::Contribute {
            goal,
            file,
            target_root: None,
        } => {
            let contribution = fs::read_to_string(&file).map_err(|error| {
                RefineError::Io(format!(
                    "failed to read contribution file {}: {error}",
                    file.display()
                ))
            })?;
            let contribution: serde_json::Value =
                serde_json::from_str(&contribution).map_err(|error| {
                    RefineError::InvalidInput(format!(
                        "contribution file {} is not valid JSON: {error}",
                        file.display()
                    ))
                })?;
            daemon_json(
                "POST",
                &format!("/work/goals/{}/mission-contribution", path_segment(&goal)),
                Some(json!({ "contribution": contribution })),
            )?
        }
        MissionAction::Outcome {
            id,
            target_root: None,
        } => daemon_json(
            "GET",
            &format!("/work/missions/{}/outcome", path_segment(&id)),
            None,
        )?,
        MissionAction::Decide {
            id,
            decision_id,
            choice,
            rationale,
            actor,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!(
                "/work/missions/{}/decisions/{}",
                path_segment(&id),
                path_segment(&decision_id)
            ),
            Some(json!({
                "choice": choice,
                "rationale": rationale,
                "actor": actor
            })),
        )?,
        MissionAction::Retry {
            id,
            stage,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/retry", path_segment(&id)),
            Some(json!({ "stage": stage })),
        )?,
        MissionAction::Transfer {
            id,
            node_id,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/transfer", path_segment(&id)),
            Some(json!({ "node_id": node_id })),
        )?,
        MissionAction::AddGoal {
            id,
            goal_id,
            wave,
            role,
            optional,
            criteria,
            target_root: None,
        } => daemon_json(
            "POST",
            &format!("/work/missions/{}/goals", path_segment(&id)),
            Some(json!({
                "goal_id": goal_id,
                "wave": wave,
                "role": role,
                "required": !optional,
                "criterion_ids": criteria,
            })),
        )?,
        MissionAction::RemoveGoal {
            id,
            goal_id,
            target_root: None,
        } => daemon_json(
            "DELETE",
            &format!(
                "/work/missions/{}/goals/{}",
                path_segment(&id),
                path_segment(&goal_id)
            ),
            None,
        )?,
        MissionAction::Context {
            id,
            target_root: None,
        } => daemon_json(
            "GET",
            &format!("/work/missions/{}/context", path_segment(&id)),
            None,
        )?,
        other => {
            return Err(RefineError::InvalidInput(format!(
                "Mission command cannot be routed to the daemon in this mode: {other:?}"
            )));
        }
    };
    print_json(&response);
    Ok(())
}

fn resolve_intent(intent: Option<String>, file: Option<PathBuf>) -> RefineResult<String> {
    if intent.is_some() && file.is_some() {
        return Err(RefineError::InvalidInput(
            "mission create accepts either --intent or --file, not both".to_string(),
        ));
    }
    let source = match (intent, file) {
        (Some(intent), None) => intent,
        (None, Some(file)) => fs::read_to_string(&file).map_err(|error| {
            RefineError::Io(format!(
                "failed to read intent file {}: {error}",
                file.display()
            ))
        })?,
        (None, None) => {
            return Err(RefineError::InvalidInput(
                "mission create requires --intent or --file".to_string(),
            ));
        }
        (Some(_), Some(_)) => unreachable!("validated above"),
    };
    if source.trim().is_empty() {
        return Err(RefineError::InvalidInput(
            "mission intent cannot be empty".to_string(),
        ));
    }
    Ok(source)
}

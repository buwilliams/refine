//! Lifecycle dispatch and short-lock gate settlement.
use super::*;
impl FileEventService {
    pub fn dispatch_goal_events(&self, target: &Path) -> RefineResult<()> {
        let node = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
            &self.refine_dir,
            self.runtime()?,
        )
        .active_node_id()?;
        for name in ["transitions", "occurrences"] {
            let directory = self.refine_dir.join("automation").join(name).join(&node);
            if !directory.exists() {
                continue;
            }
            for entry in std::fs::read_dir(&directory)
                .map_err(|e| RefineError::Io(e.to_string()))?
                .take(128)
            {
                let path = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
                if path.extension().and_then(|p| p.to_str()) != Some("json") {
                    continue;
                }
                let queued: Value = match read_json(&path) {
                    Ok(record) => record,
                    Err(_) if !path.exists() => continue,
                    Err(e) => return Err(e),
                };
                if queued["node_id"].as_str() != Some(&node) {
                    continue;
                }
                let relative =
                    Path::new(queued["goal_path"].as_str().ok_or_else(|| {
                        RefineError::InvalidInput("missing event Goal path".into())
                    })?);
                if relative
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                {
                    return Err(RefineError::InvalidInput(
                        "invalid synchronized Event Goal path".into(),
                    ));
                }
                let goal_path = self.refine_dir.join(relative);
                let goal =
                    crate::infrastructure::process::supervisor::coordination::with_record_lock(
                        &self.refine_dir,
                        &crate::infrastructure::process::supervisor::coordination::record_lock_key(
                            &goal_path,
                        ),
                        || {
                            if !goal_path.exists() {
                                let _ = std::fs::remove_file(&path);
                                return Ok(None);
                            }
                            let goal: Value = read_json(&goal_path)?;
                            if name == "occurrences"
                                && !goal
                                    .get("workflow_events")
                                    .and_then(Value::as_array)
                                    .is_some_and(|items| {
                                        items.iter().any(|item| item == &queued["occurrence"])
                                    })
                            {
                                let _ = std::fs::remove_file(&path);
                                return Ok(None);
                            }
                            Ok(Some(goal))
                        },
                    )?;
                let Some(goal) = goal else {
                    continue;
                };
                let Some(id) = goal["id"].as_str() else {
                    continue;
                };
                if goal["node_id"].as_str().unwrap_or("default") != node {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                let mut context = self.manual_context(target, &json!({"goal_id": id}))?;
                if name == "occurrences" {
                    // The index is written before the Goal replacement. Only a durable
                    // occurrence authorizes execution; an interrupted write cannot emit it.
                    if !goal
                        .get("workflow_events")
                        .and_then(Value::as_array)
                        .is_some_and(|items| items.iter().any(|item| item == &queued["occurrence"]))
                    {
                        continue;
                    }
                    context.data["occurrence"] = queued["occurrence"].clone();
                    if let Some(pinned) = queued.get("goal_context") {
                        context.data["goal"] = pinned.clone();
                        context.round_idx = pinned["rounds"]
                            .as_array()
                            .and_then(|rounds| rounds.len().checked_sub(1));
                    }
                    let config: AutomationConfig = serde_json::from_value(queued["config"].clone())
                        .map_err(|e| RefineError::Serialization(e.to_string()))?;
                    for event in config.events.values().filter(|e| {
                        e.source == queued["source"].as_str().map(str::to_string)
                            && e.enabled
                            && e.scope.applies(&node)
                    }) {
                        let source = queued["source"].as_str().unwrap_or_default();
                        let exit = source.ends_with(".exit");
                        let generation = if exit {
                            queued["previous_generation"].as_u64().unwrap_or(0)
                        } else {
                            queued["generation"].as_u64().unwrap_or(0)
                        };
                        let round = if exit {
                            queued["previous_round"].as_u64().unwrap_or(0)
                        } else {
                            queued["occurrence"]["round_idx"].as_u64().unwrap_or(0)
                        };
                        let variant = if exit {
                            format!(
                                "to-{}-{}",
                                queued["occurrence"]["to"].as_str().unwrap_or_default(),
                                queued["candidate_commit"].as_str().unwrap_or_default()
                            )
                        } else {
                            String::new()
                        };
                        let key = format!("{id}:{round}:{generation}:{node}:{source}:{variant}");
                        match self.prepare_pinned(
                            &config,
                            event,
                            context.clone(),
                            BTreeMap::new(),
                            &key,
                        ) {
                            Ok(_) => {}
                            Err(error) => {
                                write_json(
                                    &self
                                        .refine_dir
                                        .join("automation/occurrence-errors")
                                        .join(path.file_name().unwrap()),
                                    &json!({"event": event.id, "goal_id": id, "error": error.to_string()}),
                                )?;
                            }
                        }
                    }
                    let _ = std::fs::remove_file(&path);
                } else {
                    let Some(pending) = goal
                        .get("pending_event_transition")
                        .filter(|p| p["id"] == queued["id"] && p["state"] == "pending")
                    else {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    };
                    context.data["lifecycle_transition"] = pending.clone();
                    let pending_id = pending["id"].as_str().unwrap_or_default().to_string();
                    let config: AutomationConfig =
                        serde_json::from_value(pending["config"].clone())
                            .map_err(|e| RefineError::Serialization(e.to_string()))?;
                    let from = pending["from"].as_str().unwrap_or_default();
                    let source = format!("workflow.{from}.exit");
                    let entry_source = format!("workflow.{from}.enter");
                    let mut complete = true;
                    let mut failed = false;
                    let mut checked_invocations = Vec::new();
                    let entry_config: AutomationConfig = if pending["entry_config"].is_object() {
                        serde_json::from_value(pending["entry_config"].clone())
                            .map_err(|e| RefineError::Serialization(e.to_string()))?
                    } else {
                        super::super::gate_configuration::transition_entry_configuration(
                            &goal, &config, &node, from,
                        )?
                    };
                    let selected = config
                        .events
                        .values()
                        .filter(|e| e.source.as_deref() == Some(&source))
                        .map(|e| (&config, e))
                        .chain(
                            entry_config
                                .events
                                .values()
                                .filter(|e| {
                                    !["plan", "implement", "quality", "governance"].contains(&from)
                                        && e.source.as_deref() == Some(&entry_source)
                                })
                                .map(|e| (&entry_config, e)),
                        );
                    for (binding_config, event) in
                        selected.filter(|(_, e)| e.enabled && e.scope.applies(&node))
                    {
                        let effective = binding_config.bindings(event, &node);
                        if effective.is_empty() {
                            continue;
                        }
                        let blocking = effective
                            .iter()
                            .any(|(binding, _)| binding.mode == BindingMode::Blocking);
                        if !blocking {
                            continue;
                        }
                        let key = if event.source.as_deref() == Some(&entry_source) {
                            format!(
                                "{id}:{}:{}:{node}:{entry_source}:",
                                context.round_idx.unwrap_or(0),
                                goal["event_generation"].as_u64().unwrap_or(0)
                            )
                        } else {
                            pending_id.clone()
                        };
                        let mut binding_context = context.clone();
                        if event.source.as_deref() == Some(&entry_source)
                            && let Some(occurrence) =
                                goal["workflow_events"].as_array().and_then(|items| {
                                    items.iter().find(|item| {
                                        item["generation"] == goal["event_generation"]
                                            && item["to"] == pending["from"]
                                    })
                                })
                        {
                            binding_context.data["occurrence"] = occurrence.clone();
                        }
                        let invocation = match self.prepare_pinned(
                            binding_config,
                            event,
                            binding_context,
                            BTreeMap::new(),
                            &key,
                        ) {
                            Ok(invocation) => invocation,
                            Err(error) => {
                                write_json(
                                    &self
                                        .refine_dir
                                        .join("automation/occurrence-errors")
                                        .join(format!("{pending_id}.json")),
                                    &json!({"event_id": event.id, "goal_id": id, "error": error.to_string()}),
                                )?;
                                failed = true;
                                continue;
                            }
                        };
                        if let Some(required) =
                            super::super::execution::BlockingInvocation::pin(&invocation)
                        {
                            checked_invocations.push(required);
                        }
                        let assessment = invocation.gate_assessment();
                        complete &= assessment
                            != crate::application::workflow::gates::GateAssessment::Missing;
                        failed |= matches!(
                            assessment,
                            crate::application::workflow::gates::GateAssessment::Finding
                                | crate::application::workflow::gates::GateAssessment::Fault
                        );
                    }
                    if complete || failed {
                        crate::infrastructure::process::supervisor::coordination::with_record_lock(
                            &self.refine_dir,
                            id,
                            || {
                                self.settle_blocking(&checked_invocations, |validation| {
                                    if let Err(error) = validation {
                                        failed = true;
                                        write_json(
                                            &self
                                                .refine_dir
                                                .join("automation/occurrence-errors")
                                                .join(format!("{pending_id}.json")),
                                            &json!({"goal_id":id,"error":error.to_string()}),
                                        )?;
                                    }
                                    crate::application::work_items::FileWorkItemService::new(
                                        &self.refine_dir,
                                    )
                                    .settle_event_transition(
                                        id,
                                        &pending_id,
                                        failed,
                                    )
                                })
                            },
                        )?;
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
        Ok(())
    }
}

//! Goal mutations publish lifecycle occurrences atomically with semantic state.
//! Blocking user transitions retain intent on the Goal; workers never hold a Goal lock
//! while an agent runs, so Cancel and reassignment remain immediately available.
use super::FileEventService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::storage::automation::{AutomationStore, read_json, write_json};
use crate::model::automation::{AutomationConfig, BindingMode};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

thread_local! { static ACTION: std::cell::RefCell<Option<Value>> = const { std::cell::RefCell::new(None) }; }

pub(super) fn with_action<T>(
    receipt: Value,
    action: impl FnOnce() -> RefineResult<T>,
) -> RefineResult<T> {
    struct Reset(Option<Value>);
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTION.with(|value| *value.borrow_mut() = self.0.take());
        }
    }
    let _reset = Reset(ACTION.with(|value| value.replace(Some(receipt))));
    action()
}

pub const PENDING: &str = "Event gates pending:";

pub fn edge_key(goal: &Value, to: &str) -> String {
    super::execution::stable_id(&json!({"id": goal.get("id"), "node": goal.get("node_id"), "generation": goal.get("event_generation"), "rounds": goal.get("rounds").and_then(Value::as_array).map(Vec::len), "from": goal.get("status"), "to": to, "candidate": goal.get("candidate_commit")}).to_string())
}

fn approval_path(root: &Path, goal: &Value, to: &str) -> PathBuf {
    root.join("automation/approvals")
        .join(format!("{}.json", edge_key(goal, to)))
}

pub fn approve_exit(root: &Path, goal: &Value, to: &str) -> RefineResult<()> {
    write_json(
        &approval_path(root, goal, to),
        &json!({"goal_id": goal.get("id"), "to": to, "candidate_commit": goal.get("candidate_commit")}),
    )
}

/// Called under the Goal record lock, before its single durable replacement.
/// True means the requested status was retained as pending intent instead.
pub fn prepare_write(
    root: &Path,
    path: &Path,
    current: Option<&Value>,
    next: &mut Value,
) -> RefineResult<bool> {
    if !root.join("automation/config.json").exists() {
        return Ok(false);
    }
    ACTION.with(|receipt| {
        if let Some(receipt) = receipt.borrow().as_ref() {
            let mut receipts = current
                .and_then(|v| v.get("event_actions"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            receipts.insert(
                receipt["invocation_id"].as_str().unwrap_or_default().into(),
                receipt.clone(),
            );
            next["event_actions"] = json!(receipts);
            next["event_action_depth"] = receipt["depth"].clone();
        }
    });
    let from = current
        .and_then(|v| v.get("status"))
        .and_then(Value::as_str);
    let Some(to) = next
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(false);
    };
    if from == Some(&to) {
        return Ok(false);
    }
    let config = AutomationStore::new(root).load()?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| RefineError::InvalidInput("Goal path outside state root".into()))?;
    let node = next
        .get("node_id")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .to_string();
    let force = ["cancelled", "failed"].contains(&to.as_str());
    if let (Some(from), Some(current)) = (from, current) {
        let blocking = config
            .events
            .values()
            .filter(|e| {
                e.source.as_deref() == Some(&format!("workflow.{from}.exit"))
                    || (!["plan", "implement", "quality", "governance"].contains(&from)
                        && e.source.as_deref() == Some(&format!("workflow.{from}.enter")))
            })
            .any(|e| {
                config
                    .bindings(e, &node)
                    .iter()
                    .any(|(b, _)| b.mode == BindingMode::Blocking)
            });
        if blocking && !force && !approval_path(root, current, &to).exists() {
            if let Some(pending) = current
                .get("pending_event_transition")
                .filter(|p| p.get("state").and_then(Value::as_str) == Some("pending"))
            {
                return Err(RefineError::Conflict(format!(
                    "{PENDING} {}",
                    pending["id"].as_str().unwrap_or("transition")
                )));
            }
            let id = uuid::Uuid::new_v4().to_string();
            let requested = next.clone();
            *next = current.clone();
            if let Some(receipts) = requested.get("event_actions") {
                next["event_actions"] = receipts.clone();
                next["event_action_depth"] = requested["event_action_depth"].clone();
            }
            next["pending_event_transition"] = json!({"id": id, "state": "pending", "from": from, "to": to, "node_id": node, "requested": requested, "config": *config, "revision": current.get("workflow_revision").and_then(Value::as_u64).unwrap_or(0) + 1});
            write_json(
                &root
                    .join("automation/transitions")
                    .join(&node)
                    .join(format!("{id}.json")),
                &json!({"goal_path": relative, "id": id, "node_id": node}),
            )?;
            return Ok(true);
        }
    }
    let generation = current
        .and_then(|v| v.get("event_generation"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + 1;
    let occurrence = json!({"generation": generation, "from": from, "to": to, "node_id": node, "round_idx": next.get("rounds").and_then(Value::as_array).and_then(|r| r.len().checked_sub(1)), "at": chrono::Utc::now().to_rfc3339(), "forced": force});
    next["event_generation"] = json!(generation);
    next.as_object_mut()
        .unwrap()
        .remove("pending_event_transition");
    let mut occurrences = current
        .and_then(|v| v.get("workflow_events"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    occurrences.push(occurrence.clone());
    next["workflow_events"] = json!(occurrences);
    // Automated phase entry is consumed by the existing workflow worker. Other
    // lifecycle hooks use the same pending dispatch capability as custom Events.
    let mut sources = Vec::new();
    if !["plan", "implement", "quality", "governance"].contains(&to.as_str()) {
        sources.push(format!("workflow.{to}.enter"));
    }
    if let Some(from) = from {
        let source = format!("workflow.{from}.exit");
        if force
            || !config
                .events
                .values()
                .filter(|e| e.source.as_deref() == Some(&source))
                .any(|e| {
                    config
                        .bindings(e, &node)
                        .iter()
                        .any(|(b, _)| b.mode == BindingMode::Blocking)
                })
        {
            sources.push(source);
        }
    }
    for source in sources {
        if config
            .events
            .values()
            .filter(|e| e.source.as_deref() == Some(&source))
            .any(|e| !config.bindings(e, &node).is_empty())
        {
            let key = super::execution::stable_id(&format!("{}:{generation}:{source}", next["id"]));
            write_json(
                &root
                    .join("automation/occurrences")
                    .join(&node)
                    .join(format!("{key}.json")),
                &json!({"goal_path": relative, "source": source, "node_id": node, "generation": generation, "config": *config, "forced": force, "occurrence": occurrence, "goal_context": super::execution::goal_context(next), "previous_generation": current.and_then(|v| v.get("event_generation")).cloned().unwrap_or(json!(0)), "previous_round": current.and_then(|v| v.get("rounds")).and_then(Value::as_array).and_then(|r| r.len().checked_sub(1)), "candidate_commit": current.and_then(|v| v.get("candidate_commit"))}),
            )?;
        }
    }
    Ok(false)
}

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
                    let pending_id = pending["id"].as_str().unwrap_or_default().to_string();
                    let config: AutomationConfig =
                        serde_json::from_value(pending["config"].clone())
                            .map_err(|e| RefineError::Serialization(e.to_string()))?;
                    let from = pending["from"].as_str().unwrap_or_default();
                    let source = format!("workflow.{from}.exit");
                    let entry_source = format!("workflow.{from}.enter");
                    let mut complete = true;
                    let mut failed = false;
                    for event in config.events.values().filter(|e| {
                        (e.source.as_deref() == Some(&source)
                            || (!["plan", "implement", "quality", "governance"].contains(&from)
                                && e.source.as_deref() == Some(&entry_source)))
                            && e.enabled
                            && e.scope.applies(&node)
                    }) {
                        let key = if event.source.as_deref() == Some(&entry_source) {
                            format!(
                                "{id}:{}:{}:{node}:{entry_source}:",
                                context.round_idx.unwrap_or(0),
                                goal["event_generation"].as_u64().unwrap_or(0)
                            )
                        } else {
                            pending_id.clone()
                        };
                        let invocation = match self.prepare_pinned(
                            &config,
                            event,
                            context.clone(),
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
                        crate::application::work_items::FileWorkItemService::new(&self.refine_dir)
                            .settle_event_transition(id, &pending_id, failed)?;
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
        }
        Ok(())
    }
}

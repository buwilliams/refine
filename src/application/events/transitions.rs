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
    let current_config = AutomationStore::new(root).load()?;
    let relative = path
        .strip_prefix(root)
        .map_err(|_| RefineError::InvalidInput("Goal path outside state root".into()))?;
    let node = next
        .get("node_id")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .to_string();
    let config = (*current_config).clone();
    let explicit_override = next["workflow_controls"]
        .as_array()
        .is_some_and(|controls| {
            controls.len()
                > current
                    .and_then(|value| value["workflow_controls"].as_array())
                    .map_or(0, Vec::len)
                && controls
                    .last()
                    .is_some_and(|receipt| receipt["forced"] == true)
        });
    let force = ["cancelled", "failed"].contains(&to.as_str()) || explicit_override;
    let redirected = current
        .is_some_and(|value| value["pending_workflow_outcome"]["state"] == "pending")
        && next["pending_workflow_outcome"]["state"] == "redirected";
    let entry_config = if force {
        None
    } else if let (Some(goal), Some(from)) = (current, from) {
        Some(super::gate_configuration::transition_entry_configuration(
            goal, &config, &node, from,
        )?)
    } else {
        None
    };
    if let (Some(from), Some(current)) = (from, current) {
        let blocking = [
            (&config, format!("workflow.{from}.success")),
            (&config, format!("workflow.{from}.exit")),
            (
                entry_config.as_ref().unwrap_or(&config),
                format!("workflow.{from}.enter"),
            ),
        ]
        .into_iter()
        .any(|(selected, source)| {
            if source.ends_with(".enter")
                && ["plan", "implement", "quality", "governance"].contains(&from)
            {
                return false;
            }
            selected
                .events
                .values()
                .filter(|e| e.source.as_deref() == Some(&source))
                .any(|e| {
                    selected
                        .bindings(e, &node)
                        .iter()
                        .any(|(b, _)| b.mode == BindingMode::Blocking)
                })
        });
        if blocking && !force && !redirected && !approval_path(root, current, &to).exists() {
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
            next["pending_event_transition"] = json!({"id": id, "state": "pending", "from": from, "to": to, "node_id": node, "requested": requested, "config": config, "entry_config": entry_config, "revision": current.get("workflow_revision").and_then(Value::as_u64).unwrap_or(0) + 1});
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
    if let Some(from) = from {
        let success = format!("workflow.{from}.success");
        if !force
            && !current.is_some_and(|value| value["pending_workflow_outcome"]["state"] == "pending")
            && !config
                .events
                .values()
                .filter(|event| event.source.as_deref() == Some(&success))
                .any(|event| {
                    config
                        .bindings(event, &node)
                        .iter()
                        .any(|(binding, _)| binding.mode == BindingMode::Blocking)
                })
        {
            sources.push(success);
        }
        let source = format!("workflow.{from}.exit");
        if force
            || redirected
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
    if !["plan", "implement", "quality", "governance"].contains(&to.as_str()) {
        let source = format!("workflow.{to}.enter");
        super::gate_configuration::pin_lifecycle_entry(next, &current_config, &node, &source);
        sources.push(source);
    }
    for source in sources {
        let terminal_success = ["done", "failed", "cancelled"].contains(&to.as_str())
            && source == format!("workflow.{to}.enter")
            && config.events.values().any(|event| {
                event.source.as_deref() == Some(&format!("workflow.{to}.success"))
                    && event.enabled
                    && !config.bindings(event, &node).is_empty()
            });
        if terminal_success
            || config
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
                &json!({"goal_path": relative, "source": source, "node_id": node, "generation": generation, "config": config, "forced": force, "occurrence": occurrence, "goal_context": super::execution::goal_context(next), "previous_generation": current.and_then(|v| v.get("event_generation")).cloned().unwrap_or(json!(0)), "previous_round": current.and_then(|v| v.get("rounds")).and_then(Value::as_array).and_then(|r| r.len().checked_sub(1)), "candidate_commit": current.and_then(|v| v.get("candidate_commit"))}),
            )?;
        }
    }
    Ok(false)
}

mod dispatch;

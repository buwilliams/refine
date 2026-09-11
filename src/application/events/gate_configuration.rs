//! Requirements are pinned once per workflow occurrence, independently of later edits.
use super::FileEventService;
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::model::automation::AutomationConfig;
use serde_json::{Value, json};

impl FileEventService {
    pub(crate) fn gate_configuration(
        &self,
        goal_id: &str,
        round_idx: usize,
        node: &str,
        source: &str,
        validate: impl Fn() -> RefineResult<()>,
    ) -> RefineResult<AutomationConfig> {
        with_record_lock(&self.refine_dir, goal_id, || {
            validate()?;
            let items = FileWorkItemService::for_node(&self.refine_dir, node);
            let detail = items.show_goal_detail(goal_id)?;
            let round = detail["rounds"]
                .as_array()
                .and_then(|r| r.get(round_idx))
                .ok_or_else(|| RefineError::Conflict("Skill gate Round is unavailable".into()))?;
            let key = format!(
                "{}:{node}:{source}",
                detail["event_generation"].as_u64().unwrap_or(0)
            );
            let mut snapshots = round["gate_configurations"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            if let Some(config) = snapshots.get(&key) {
                return serde_json::from_value(config.clone())
                    .map_err(|e| RefineError::Serialization(e.to_string()));
            }
            let current = self.config()?;
            let config = select_source(&current, node, source);
            // Pin only this trigger's requirements; copying the entire project at every
            // boundary multiplies synchronized Goal state and provider context.
            snapshots.insert(key, json!(config));
            items.update_goal_round_evaluation_summary(
                goal_id,
                round_idx,
                &json!({"gate_configurations": snapshots, "event_configuration": config}),
            )?;
            Ok(config)
        })
    }
}

/// All adapters select the same trigger snapshot, including lifecycle dispatch.
pub(super) fn select_source(
    current: &AutomationConfig,
    node: &str,
    source: &str,
) -> AutomationConfig {
    let events = current
        .events
        .iter()
        .filter(|(_, event)| event.source.as_deref() == Some(source) && event.scope.applies(node))
        .map(|(id, event)| (id.clone(), event.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let referenced = events
        .values()
        .flat_map(|event| {
            event
                .bindings
                .iter()
                .map(|binding| binding.skill_id.as_str())
        })
        .collect::<std::collections::BTreeSet<_>>();
    let skills = current
        .skills
        .iter()
        .filter(|(id, _)| referenced.contains(id.as_str()))
        .map(|(id, skill)| (id.clone(), skill.clone()))
        .collect();
    AutomationConfig {
        schema_version: current.schema_version,
        revision: current.revision,
        events,
        skills,
    }
}

pub(super) fn occurrence_configuration(
    goal: &Value,
    node: &str,
    source: &str,
) -> RefineResult<Option<AutomationConfig>> {
    let key = format!(
        "{}:{node}:{source}",
        goal["event_generation"].as_u64().unwrap_or(0)
    );
    goal["rounds"]
        .as_array()
        .and_then(|r| r.last())
        .and_then(|round| round["gate_configurations"].get(&key))
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|e| RefineError::Serialization(e.to_string()))
        })
        .transpose()
}

/// Pin lifecycle Entry requirements with the durable occurrence, before either worker sees it.
pub(super) fn pin_lifecycle_entry(
    goal: &mut Value,
    config: &AutomationConfig,
    node: &str,
    source: &str,
) {
    let key = format!(
        "{}:{node}:{source}",
        goal["event_generation"].as_u64().unwrap_or(0)
    );
    if let Some(round) = goal["rounds"].as_array_mut().and_then(|r| r.last_mut()) {
        if !round["gate_configurations"].is_object() {
            round["gate_configurations"] = json!({});
        }
        round["gate_configurations"][key] = json!(select_source(config, node, source));
    }
}

/// A manual Entry keeps its admitted occurrence snapshot. Its Skill definitions
/// must not replace definitions independently selected for the requested Exit.
pub(super) fn transition_entry_configuration(
    goal: &Value,
    current: &AutomationConfig,
    node: &str,
    from: &str,
) -> RefineResult<AutomationConfig> {
    let source = format!("workflow.{from}.enter");
    Ok(occurrence_configuration(goal, node, &source)?
        .unwrap_or_else(|| select_source(current, node, &source)))
}

/// Read-only admission/diagnostic assessment of the same occurrence-pinned requirements
/// consumed by workflow execution. Missing requirements never create or mutate a snapshot.
impl FileEventService {
    pub(crate) fn missing_workflow_requirement(
        &self,
        goal: &Value,
        node: &str,
    ) -> RefineResult<Option<String>> {
        let current = self.config()?;
        for role in ["plan", "implement"] {
            let source = format!("workflow.{role}.enter");
            let config = occurrence_configuration(goal, node, &source)?
                .unwrap_or_else(|| select_source(&current, node, &source));
            if !has_blocking_workflow_skill(&config, node, &source) {
                return Ok(Some(format!(
                    "missing enabled blocking {role} Skill in node scope"
                )));
            }
        }
        Ok(None)
    }
}
pub(crate) fn has_blocking_workflow_skill(
    config: &AutomationConfig,
    node: &str,
    source: &str,
) -> bool {
    config
        .events
        .values()
        .filter(|e| e.enabled && e.source.as_deref() == Some(source) && e.scope.applies(node))
        .any(|e| {
            config
                .bindings(e, node)
                .iter()
                .any(|(b, _)| b.mode == crate::model::automation::BindingMode::Blocking)
        })
}

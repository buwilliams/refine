//! Requirements are pinned once per workflow occurrence, independently of later edits.
use super::FileEventService;
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::model::automation::AutomationConfig;
use serde_json::json;

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
            let items = FileWorkItemService::new(&self.refine_dir);
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
            let events = current
                .events
                .iter()
                .filter(|(_, event)| {
                    event.source.as_deref() == Some(source) && event.scope.applies(node)
                })
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
            let config = AutomationConfig {
                schema_version: current.schema_version,
                revision: current.revision,
                events,
                skills,
            };
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

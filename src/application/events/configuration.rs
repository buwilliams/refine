use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::storage::automation::AutomationStore;
use crate::model::automation::*;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillEventBinding {
    event_id: String,
    binding: Binding,
}

#[derive(Clone)]
pub struct FileEventService {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

impl FileEventService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: root.into(),
            runtime_root: None,
        }
    }
    pub fn with_runtime_root(root: impl Into<PathBuf>, runtime: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: root.into(),
            runtime_root: Some(runtime.into()),
        }
    }
    pub fn config(&self) -> RefineResult<Arc<AutomationConfig>> {
        AutomationStore::new(&self.refine_dir)
            .initialize(|| super::migration::migrate(&self.refine_dir))
    }
    pub fn catalog(&self) -> Value {
        json!({"sources": system_catalog(), "roles": WORKFLOW_STEPS.iter().copied().chain(std::iter::once("task")).collect::<Vec<_>>()})
    }
    pub fn list(&self, collection: &str, node: Option<&str>) -> RefineResult<Value> {
        let config = self.config()?;
        let items: Vec<Value> = match collection {
            "skills" => config
                .skills
                .values()
                .filter(|s| node.is_none_or(|n| s.scope.applies(n)))
                .map(|s| json!(s))
                .collect(),
            "events" => config
                .events
                .values()
                .filter(|e| node.is_none_or(|n| e.scope.applies(n)))
                .map(|e| json!(e))
                .collect(),
            _ => {
                return Err(RefineError::InvalidInput(
                    "unknown configuration collection".into(),
                ));
            }
        };
        Ok(json!({"revision": config.revision, "items": items}))
    }
    pub fn save(&self, collection: &str, id: &str, body: Value) -> RefineResult<Value> {
        if !valid_id(id) {
            return Err(RefineError::InvalidInput("invalid configuration ID".into()));
        }
        let revision = body
            .get("revision")
            .and_then(Value::as_u64)
            .ok_or_else(|| RefineError::InvalidInput("observed revision is required".into()))?;
        let mut item = body
            .get("item")
            .cloned()
            .ok_or_else(|| RefineError::InvalidInput("item is required".into()))?;
        let object = item
            .as_object_mut()
            .ok_or_else(|| RefineError::InvalidInput("item must be an object".into()))?;
        if object
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|v| v != id)
        {
            return Err(RefineError::InvalidInput(
                "item ID must match the path".into(),
            ));
        }
        object.insert("id".into(), json!(id));
        // The Skill editor owns its assignments. Apply the complete selection with
        // the Skill under the same revision fence; leave other Skills untouched.
        let assignments = body
            .get("event_bindings")
            .map(|value| serde_json::from_value::<Vec<SkillEventBinding>>(value.clone()))
            .transpose()
            .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
        if assignments.is_some() && collection != "skills" {
            return Err(RefineError::InvalidInput(
                "event_bindings can only be saved with a Skill".into(),
            ));
        }
        self.config()?;
        let config = AutomationStore::new(&self.refine_dir).update(revision, |config| {
            match collection {
                "skills" => {
                    let skill: Skill = serde_json::from_value(item.clone())
                        .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
                    config.skills.insert(id.into(), skill);
                    if let Some(assignments) = assignments {
                        for event in config.events.values_mut() {
                            event.bindings.retain(|binding| binding.skill_id != id);
                        }
                        for assignment in assignments {
                            if assignment.binding.skill_id != id {
                                return Err(RefineError::InvalidInput(
                                    "Event assignments must reference the edited Skill".into(),
                                ));
                            }
                            let event =
                                config.events.get_mut(&assignment.event_id).ok_or_else(|| {
                                    RefineError::NotFound(format!("Event {}", assignment.event_id))
                                })?;
                            event.bindings.push(assignment.binding);
                        }
                    }
                }
                "events" => {
                    let event: EventDefinition = serde_json::from_value(item.clone())
                        .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
                    if config
                        .events
                        .get(id)
                        .is_some_and(|old| old.kind != event.kind || old.source != event.source)
                    {
                        return Err(RefineError::InvalidInput(
                            "an Event's kind and system source are immutable".into(),
                        ));
                    }
                    config.events.insert(id.into(), event);
                }
                _ => {
                    return Err(RefineError::InvalidInput(
                        "unknown configuration collection".into(),
                    ));
                }
            }
            Ok(())
        })?;
        let saved = if collection == "skills" {
            json!(config.skills[id])
        } else {
            json!(config.events[id])
        };
        Ok(json!({"revision": config.revision, "item": saved}))
    }
    pub fn remove(&self, collection: &str, id: &str, revision: u64) -> RefineResult<Value> {
        self.config()?;
        let config = AutomationStore::new(&self.refine_dir).update(revision, |config| {
            match collection {
                "skills" => {
                    if config
                        .events
                        .values()
                        .any(|e| e.bindings.iter().any(|b| b.skill_id == id))
                    {
                        return Err(RefineError::Conflict(
                            "remove this Skill's Event bindings before deleting it".into(),
                        ));
                    }
                    if config.skills.remove(id).is_none() {
                        return Err(RefineError::NotFound(format!("Skill {id}")));
                    }
                }
                "events" => {
                    if config.events.remove(id).is_none() {
                        return Err(RefineError::NotFound(format!("Event {id}")));
                    }
                }
                _ => {
                    return Err(RefineError::InvalidInput(
                        "unknown configuration collection".into(),
                    ));
                }
            }
            Ok(())
        })?;
        Ok(json!({"revision": config.revision, "removed": id}))
    }
    pub fn runtime(&self) -> RefineResult<&Path> {
        self.runtime_root.as_deref().ok_or_else(|| {
            RefineError::InvalidInput("a daemon runtime is required to execute Events".into())
        })
    }
}

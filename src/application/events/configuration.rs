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

/// Public Skill-owned trigger settings. Binding identities remain stable on edits.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SkillTrigger {
    #[serde(default)]
    id: Option<String>,
    source: String,
    #[serde(default)]
    mode: BindingMode,
    #[serde(default)]
    order: i32,
    #[serde(default)]
    inputs: std::collections::BTreeMap<String, String>,
}

fn skill_value(skill: &Skill) -> Value {
    let mut value = json!(skill);
    value.as_object_mut().unwrap().remove("role");
    value
}

fn skill_assignment_ids(
    config: &AutomationConfig,
    id: &str,
) -> std::collections::BTreeMap<String, Vec<String>> {
    config
        .events
        .iter()
        .map(|(key, event)| {
            (
                key.clone(),
                event
                    .bindings
                    .iter()
                    .filter(|b| b.skill_id == id)
                    .map(|b| b.id.clone())
                    .collect(),
            )
        })
        .collect()
}
fn detach_removed_overrides(
    config: &mut AutomationConfig,
    prior: &std::collections::BTreeMap<String, Vec<String>>,
) {
    for (key, event) in &mut config.events {
        let Some(old) = prior.get(key) else {
            continue;
        };
        let removed: Vec<_> = old
            .iter()
            .filter(|id| {
                !event
                    .bindings
                    .iter()
                    .any(|b| &b.id == *id && b.scope.node_id.is_none())
            })
            .collect();
        for binding in &mut event.bindings {
            if binding
                .overrides
                .as_ref()
                .is_some_and(|id| removed.contains(&id))
            {
                binding.overrides = None;
            }
        }
    }
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
        let store = AutomationStore::new(&self.refine_dir);
        let config = store.initialize(|| super::migration::migrate(&self.refine_dir))?;
        if config.schema_version < SCHEMA_VERSION {
            store.upgrade(super::migration::single_trigger_skills)
        } else {
            Ok(config)
        }
    }
    pub fn catalog(&self) -> Value {
        json!({"sources": system_catalog(), "custom_source": CUSTOM_EVENT_ID, "roles": WORKFLOW_STEPS.iter().copied().chain(std::iter::once("task")).collect::<Vec<_>>()})
    }
    pub fn skill_catalog(&self) -> Value {
        json!({"sources": std::iter::once(CUSTOM_EVENT_ID.to_string()).chain(system_catalog()).collect::<Vec<_>>()})
    }
    pub fn show_skill(&self, id: &str) -> RefineResult<Value> {
        let config = self.config()?;
        let skill = config
            .skills
            .get(id)
            .ok_or_else(|| RefineError::NotFound(format!("Skill {id}")))?;
        let triggers: Vec<_> = config.events.values().flat_map(|event| {
            event.bindings.iter().filter(|b| b.skill_id == id).map(|b| {
                json!({"id": b.id, "source": event.source.as_deref().unwrap_or(CUSTOM_EVENT_ID), "mode": b.mode, "order": b.order, "inputs": b.inputs})
            })
        }).collect();
        Ok(
            json!({"revision": config.revision, "item": skill_value(skill), "trigger": triggers.first()}),
        )
    }
    pub fn list(&self, collection: &str, node: Option<&str>) -> RefineResult<Value> {
        let config = self.config()?;
        let trigger_sources: std::collections::BTreeMap<_, _> = config
            .events
            .values()
            .flat_map(|event| {
                event.bindings.iter().map(move |b| {
                    (
                        b.skill_id.as_str(),
                        event.source.as_deref().unwrap_or(CUSTOM_EVENT_ID),
                    )
                })
            })
            .collect();
        let items: Vec<Value> = match collection {
            "skills" => config
                .skills
                .values()
                .filter(|s| node.is_none_or(|n| s.scope.applies(n)))
                .map(|skill| {
                    let mut value = skill_value(skill);
                    value["trigger_source"] = json!(trigger_sources.get(skill.id.as_str()));
                    value
                })
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
        let mut manual_counts = std::collections::BTreeMap::<String, usize>::new();
        if collection == "skills"
            && let Some(node) = node
        {
            for custom in config
                .events
                .values()
                .filter(|event| event.kind == EventKind::Custom)
            {
                for (binding, _) in config.bindings(custom, node) {
                    if binding.mode != BindingMode::Context {
                        *manual_counts.entry(binding.skill_id.clone()).or_default() += 1;
                    }
                }
            }
        }
        let manual_skill_ids: Vec<_> = manual_counts
            .into_iter()
            .filter_map(|(id, count)| (count == 1).then_some(id))
            .collect();
        Ok(
            json!({"revision": config.revision, "items": items, "manual_skill_ids": manual_skill_ids}),
        )
    }
    pub fn save(&self, collection: &str, id: &str, body: Value) -> RefineResult<Value> {
        if !valid_id(id) {
            return Err(RefineError::InvalidInput("invalid configuration ID".into()));
        }
        if body.get("triggers").is_some() {
            return Err(RefineError::InvalidInput(
                "A Skill has one trigger; use trigger instead of triggers".into(),
            ));
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
        object.remove("trigger_source");
        // The Skill editor owns its assignments. Apply the complete selection with
        // the Skill under the same revision fence; leave other Skills untouched.
        let assignments = body
            .get("event_bindings")
            .map(|value| serde_json::from_value::<Vec<SkillEventBinding>>(value.clone()))
            .transpose()
            .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
        let trigger = body
            .get("trigger")
            .map(|value| serde_json::from_value::<SkillTrigger>(value.clone()))
            .transpose()
            .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
        if trigger.is_some() && (collection != "skills" || assignments.is_some()) {
            return Err(RefineError::InvalidInput(
                "Supply a trigger only with a Skill; do not combine them with event_bindings"
                    .into(),
            ));
        }
        if assignments.is_some() && collection != "skills" {
            return Err(RefineError::InvalidInput(
                "event_bindings can only be saved with a Skill".into(),
            ));
        }
        self.config()?;
        let config = AutomationStore::new(&self.refine_dir).update(revision, |config| {
            let prior = skill_assignment_ids(config, id);
            match collection {
                "skills" => {
                    let skill: Skill = serde_json::from_value(item.clone())
                        .map_err(|e| RefineError::InvalidInput(e.to_string()))?;
                    let is_new = !config.skills.contains_key(id);
                    if body.get("create_only") == Some(&json!(true)) && !is_new {
                        return Err(RefineError::Conflict(
                            "A Skill with this ID already exists".into(),
                        ));
                    }
                    if is_new && trigger.is_none() && assignments.is_none() {
                        return Err(RefineError::InvalidInput(
                            "Choose a trigger for the Skill".into(),
                        ));
                    }
                    config.skills.insert(id.into(), skill);
                    let assignments = if let Some(trigger) = trigger {
                        if trigger.source != CUSTOM_EVENT_ID
                            && !system_catalog().contains(&trigger.source)
                        {
                            return Err(RefineError::InvalidInput(format!(
                                "Unknown trigger {}",
                                trigger.source
                            )));
                        }
                        // Keep the existing event and override for an identified trigger,
                        // including configurations created before manual Skills existed.
                        let existing = trigger.id.as_ref().and_then(|trigger_id| {
                            config.events.values().find_map(|e| {
                                e.bindings
                                    .iter()
                                    .find(|b| {
                                        b.id == *trigger_id
                                            && b.skill_id == id
                                            && e.source.as_deref().unwrap_or(CUSTOM_EVENT_ID)
                                                == trigger.source
                                    })
                                    .map(|b| (e.id.clone(), b.overrides.clone()))
                            })
                        });
                        let event_id = existing
                            .as_ref()
                            .map(|(id, _)| id.clone())
                            .unwrap_or_else(|| trigger.source.clone());
                        let overrides = if config.skills[id].scope.node_id.is_some() {
                            existing.and_then(|(_, overrides)| overrides)
                        } else {
                            None
                        };
                        Some(vec![SkillEventBinding {
                            event_id,
                            binding: Binding {
                                id: trigger
                                    .id
                                    .unwrap_or_else(|| format!("trigger-{}", uuid::Uuid::new_v4())),
                                skill_id: id.into(),
                                enabled: true,
                                mode: if trigger.source == CUSTOM_EVENT_ID {
                                    BindingMode::Blocking
                                } else {
                                    trigger.mode
                                },
                                order: trigger.order,
                                scope: config.skills[id].scope.clone(),
                                inputs: trigger.inputs,
                                overrides,
                            },
                        }])
                    } else {
                        assignments
                    };
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
                            if assignment.event_id == CUSTOM_EVENT_ID {
                                config
                                    .events
                                    .entry(CUSTOM_EVENT_ID.into())
                                    .or_insert_with(custom_event);
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
            detach_removed_overrides(config, &prior);
            Ok(())
        })?;
        let saved = if collection == "skills" {
            skill_value(&config.skills[id])
        } else {
            json!(config.events[id])
        };
        Ok(json!({"revision": config.revision, "item": saved}))
    }
    pub fn remove(&self, collection: &str, id: &str, revision: u64) -> RefineResult<Value> {
        self.config()?;
        let config = AutomationStore::new(&self.refine_dir).update(revision, |config| {
            let prior = skill_assignment_ids(config, id);
            match collection {
                "skills" => {
                    for event in config.events.values_mut() {
                        event.bindings.retain(|binding| binding.skill_id != id);
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
            detach_removed_overrides(config, &prior);
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

//! Bounded failure decisions, admitted by the existing Skill worker.
use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::storage::automation::{read_json, write_json};
use crate::model::automation::AutomationConfig;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

impl FileEventService {
    pub fn dispatch_outcomes(&self, target: &Path) -> RefineResult<()> {
        let node = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
            &self.refine_dir,
            self.runtime()?,
        )
        .active_node_id()?;
        let directory = self.refine_dir.join("automation/outcomes").join(&node);
        if !directory.exists() {
            return Ok(());
        }
        let work = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            self.runtime()?,
            self.runtime()?.join("cache"),
        );
        for entry in std::fs::read_dir(&directory)
            .map_err(|e| RefineError::Io(e.to_string()))?
            .take(128)
        {
            let path = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let marker: Value = read_json(&path)?;
            let Some(id) = marker["goal_id"].as_str() else {
                continue;
            };
            let goal = work.show_goal_detail(id)?;
            let pending = &goal["pending_workflow_outcome"];
            if pending["id"] != marker["id"]
                || pending["state"] != "pending"
                || goal["node_id"].as_str().unwrap_or("default") != node
            {
                std::fs::remove_file(&path).map_err(|e| RefineError::Io(e.to_string()))?;
                continue;
            }
            let occurrence_id = pending["id"].as_str().unwrap_or_default();
            let expired = chrono::Utc::now().timestamp_millis()
                >= pending["deadline_ms"].as_i64().unwrap_or(0);
            let config: AutomationConfig = serde_json::from_value(pending["config"].clone())
                .map_err(|e| RefineError::Serialization(e.to_string()))?;
            let source = pending["source"].as_str().unwrap_or_default();
            let mut complete = true;
            let mut failure = None;
            for event in config.events.values().filter(|e| {
                e.enabled
                    && e.scope.applies(&node)
                    && e.source.as_deref() == Some(source)
                    && config.bindings(e, &node).iter().any(|(binding, _)| {
                        binding.mode != crate::model::automation::BindingMode::Context
                    })
            }) {
                let mut context = self.manual_context(target, &json!({"goal_id":id}))?;
                context.data["occurrence"] = pending["occurrence"].clone();
                context.data["outcome"] = json!({
                    "id":pending["id"], "source":pending["source"],
                    "category":pending["category"], "message":pending["message"],
                    "deadline_ms":pending["deadline_ms"], "candidate_commit":pending["candidate_commit"]
                });
                context
                    .metadata
                    .insert("outcome_deadline_ms".into(), pending["deadline_ms"].clone());
                let invocation =
                    self.prepare_pinned(&config, event, context, BTreeMap::new(), occurrence_id);
                let invocation = match invocation {
                    Ok(v) => v,
                    Err(e) => {
                        failure = Some(e.to_string());
                        break;
                    }
                };
                if expired {
                    if !invocation.state.terminal() {
                        self.cancel_invocation(&invocation.id)?;
                    }
                    failure = Some("Error handling deadline expired".into());
                    break;
                }
                match invocation.state {
                    InvocationState::Succeeded => {}
                    InvocationState::Pending | InvocationState::Running => {
                        complete = false;
                        break;
                    }
                    _ => {
                        failure = Some(
                            invocation
                                .error
                                .unwrap_or_else(|| "Error handler did not succeed".into()),
                        );
                        break;
                    }
                }
            }
            if complete || failure.is_some() {
                work.finish_pending_outcome(
                    id,
                    occurrence_id,
                    failure
                        .as_deref()
                        .unwrap_or("No explicit workflow redirect was requested"),
                )?;
            }
        }
        Ok(())
    }
}

/// Called while the Goal is locked. The marker is harmless until the Goal write
/// makes the matching occurrence durable; dispatch always verifies both.
pub(crate) fn prepare_error(
    root: &Path,
    goal: &mut Value,
    category: &str,
    message: &str,
) -> RefineResult<bool> {
    if let Some(pending) = goal.get("pending_workflow_outcome") {
        if pending["state"] == "pending" {
            return Ok(true);
        }
    }
    let store = crate::infrastructure::storage::automation::AutomationStore::new(root);
    if !store
        .path()
        .try_exists()
        .map_err(|e| RefineError::Io(e.to_string()))?
    {
        return Ok(false);
    }
    let config = store.load()?;
    let node = goal["node_id"].as_str().unwrap_or("default").to_string();
    let source = format!(
        "workflow.{}.error",
        goal["status"].as_str().unwrap_or("failed")
    );
    if !config.events.values().any(|e| {
        e.enabled
            && e.scope.applies(&node)
            && e.source.as_deref() == Some(&source)
            && config
                .bindings(e, &node)
                .iter()
                .any(|(binding, _)| binding.mode != crate::model::automation::BindingMode::Context)
    }) {
        return Ok(false);
    }
    // Retain only this outcome's definitions. Unrelated Skills should not inflate
    // every failed Goal or its synchronized history.
    let mut config = (*config).clone();
    config.events.retain(|_, event| {
        event.enabled && event.scope.applies(&node) && event.source.as_deref() == Some(&source)
    });
    let skills = config
        .events
        .values()
        .flat_map(|event| {
            event
                .bindings
                .iter()
                .map(|binding| binding.skill_id.clone())
        })
        .collect::<std::collections::BTreeSet<_>>();
    config.skills.retain(|id, _| skills.contains(id));
    let id = format!(
        "{}-{}",
        goal["id"].as_str().unwrap_or_default(),
        uuid::Uuid::new_v4().simple()
    );
    let occurrence = json!({"id":id,"generation":goal["event_generation"].as_u64().unwrap_or(0),"from":goal["status"],"to":goal["status"],"node_id":node,"round_idx":goal["rounds"].as_array().and_then(|r|r.len().checked_sub(1)),"at":chrono::Utc::now().to_rfc3339(),"error":true});
    goal.as_object_mut()
        .unwrap()
        .entry("workflow_events")
        .or_insert(json!([]))
        .as_array_mut()
        .unwrap()
        .push(occurrence.clone());
    use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
    let settings = FileSettingsService::for_node(root, &node).load()?;
    let seconds = settings["workflow_error_timeout_seconds"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(600)
        .clamp(1, 86400);
    goal["pending_workflow_outcome"] = json!({"id":id,"source":source,"state":"pending","category":category,"message":message,"candidate_commit":goal["candidate_commit"],"occurrence":occurrence,"config":config,"deadline_ms":chrono::Utc::now().timestamp_millis()+seconds*1000});
    write_json(
        &root
            .join("automation/outcomes")
            .join(&node)
            .join(format!("{id}.json")),
        &json!({"id":id,"goal_id":goal["id"]}),
    )?;
    Ok(true)
}

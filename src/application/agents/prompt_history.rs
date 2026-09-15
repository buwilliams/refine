//! Read retained launch prompts, including completed processes, without regenerating context.
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, ManagedProcess, ProcessOwner,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub fn goal_prompts(runtime: &Path, target: &Path, goal_id: &str) -> RefineResult<Value> {
    let mut records = BTreeMap::new();
    for root in [runtime.to_path_buf(), runtime.join("agents")] {
        let supervisor = FileProcessSupervisor::new(root);
        for directory in [supervisor.process_history_dir(), supervisor.processes_dir()] {
            let entries = match std::fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(RefineError::Io(error.to_string())),
            };
            for entry in entries {
                let entry = entry.map_err(|error| RefineError::Io(error.to_string()))?;
                if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = std::fs::read(entry.path()) else {
                    continue;
                };
                let Ok(process) = serde_json::from_slice::<ManagedProcess>(&bytes) else {
                    continue;
                };
                if process.owner != ProcessOwner::Agent {
                    continue;
                }
                let metadata: Value = process
                    .details
                    .as_deref()
                    .and_then(|details| serde_json::from_str(details).ok())
                    .unwrap_or(Value::Null);
                if metadata["goal_id"]
                    .as_str()
                    .or_else(|| metadata["attached_goal_id"].as_str())
                    != Some(goal_id)
                {
                    continue;
                }
                if let Some(project) = metadata["target_app_id"].as_str()
                    && Path::new(project) != target
                {
                    continue;
                }
                let prompt = metadata["rendered_prompt"].as_str();
                records.insert(process.id.clone(), json!({
                    "id":process.id, "started_at":process.started_at, "state":process.state,
                    "provider":metadata["provider"], "round_idx":metadata["round_idx"],
                    "step":metadata["implementation_phase"].as_str().or_else(|| metadata["workflow_state"].as_str()).or_else(|| metadata["profile"].as_str()),
                    "skill_id":metadata["skill_id"], "label":process.label,
                    "prompt":prompt, "bytes":prompt.map(str::len),
                    "transport":metadata["prompt_transport"]["kind"],
                }));
            }
        }
    }
    let mut items: Vec<_> = records.into_values().collect();
    items.sort_by(|a, b| {
        b["started_at"]
            .as_str()
            .cmp(&a["started_at"].as_str())
            .then(a["id"].as_str().cmp(&b["id"].as_str()))
    });
    Ok(json!({"items":items}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_preserves_full_prompts_and_isolates_goals_projects_and_rounds() {
        let root =
            std::env::temp_dir().join(format!("refine-prompt-history-{}", uuid::Uuid::new_v4()));
        let history = FileProcessSupervisor::new(&root).process_history_dir();
        std::fs::create_dir_all(&history).unwrap();
        let prompt = format!(
            "<script>not HTML</script> {{skill}}\n{}",
            "Full context 世界\n".repeat(10000)
        );
        for (id, goal, target, text) in [
            ("recorded", "GOAL1", "/project", Some(prompt.as_str())),
            ("old", "GOAL1", "/project", None),
            ("other-goal", "GOAL2", "/project", Some("Hidden")),
            ("other-project", "GOAL1", "/other", Some("Hidden")),
        ] {
            let process = json!({"id":id,"owner":"agent","pid":null,"state":"exited","label":"Plan","started_at":"2026-09-14T00:00:00Z","details":json!({"goal_id":goal,"target_app_id":target,"round_idx":1,"rendered_prompt":text}).to_string()});
            std::fs::write(
                history.join(format!("{id}.json")),
                serde_json::to_vec(&process).unwrap(),
            )
            .unwrap();
        }
        let result = goal_prompts(&root, Path::new("/project"), "GOAL1").unwrap();
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        let recorded = items.iter().find(|item| item["id"] == "recorded").unwrap();
        assert_eq!(recorded["prompt"], prompt);
        assert_eq!(recorded["round_idx"], 1);
        assert!(items.iter().find(|item| item["id"] == "old").unwrap()["prompt"].is_null());
        std::fs::remove_dir_all(root).unwrap();
    }
}

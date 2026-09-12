//! Explicit Round removal. A retained write plan makes interrupted cleanup retryable.
use super::*;
use serde_json::json;
use std::path::Path;

impl FileWorkItemService {
    pub(crate) fn append_workflow_round_log(
        &self,
        id: &str,
        round_idx: usize,
        revision: u64,
        entry: crate::model::log::LogEntry,
    ) -> RefineResult<()> {
        let _lock = self.acquire_goal_mutation_lock(id)?;
        // Completed attempts may still log their settlement. Only deletion of
        // their Round history revokes that provenance, not ordinary advancement.
        let mut reader = self.clone();
        reader.execution_occurrence = None;
        let goal = reader.show_goal_detail(id)?;
        if goal["round_edit_revision"].as_u64().unwrap_or(0) > revision {
            return Err(RefineError::Conflict(
                "Round history changed; stale workflow logs cannot be attached to another Round"
                    .into(),
            ));
        }
        crate::infrastructure::observability::logs::FileLogService::new(&self.refine_dir)
            .append_round_log(id, round_idx, entry)?;
        Ok(())
    }

    pub(super) fn resume_round_deletion(&self, id: &str) -> RefineResult<()> {
        let current = self.show_goal_summary(id)?;
        let journal = self
            .refine_dir
            .join(&current.goal.json_path)
            .with_file_name("round-deletion.json");
        if journal.exists() {
            let plan: Value = serde_json::from_slice(&fs::read(journal).map_err(io_error)?)
                .map_err(json_error)?;
            self.delete_goal_round(
                id,
                plan["round_idx"].as_u64().unwrap() as usize,
                plan["expected_revision"].as_u64().unwrap(),
            )?;
        }
        Ok(())
    }

    pub fn delete_goal_round(
        &self,
        id: &str,
        round_idx: usize,
        expected_revision: u64,
    ) -> RefineResult<GoalSummaryProjection> {
        let _lock = self.acquire_goal_mutation_lock(id)?;
        let current = self.show_goal_summary(id)?;
        self.ensure_goal_owned(&current)?;
        let (path, mut goal) = self.read_goal_value_unchecked_locked(&current)?;
        let journal = path.with_file_name("round-deletion.json");
        // Event writers and index repair share this lock without taking the Goal
        // mutation lock, so stopped callbacks cannot race the cleanup plan.
        let _history_lock =
            crate::infrastructure::process::supervisor::coordination::acquire_record_lock(
                &self.refine_dir,
                &format!("round-history-{id}"),
            )?;
        let plan = if journal.exists() {
            let plan: Value = serde_json::from_slice(&fs::read(&journal).map_err(io_error)?)
                .map_err(json_error)?;
            if plan["round_idx"] != round_idx || plan["expected_revision"] != expected_revision {
                return Err(RefineError::Conflict(
                    "Resume the pending Round deletion before deleting another Round".into(),
                ));
            }
            plan
        } else {
            if workflow_revision(&goal) != expected_revision {
                return Err(RefineError::Conflict(
                    "Goal changed; refresh before deleting a Round".into(),
                ));
            }
            let retained_branch = goal["branch_name"].as_str().map(str::to_string);
            let rounds = goal["rounds"]
                .as_array_mut()
                .ok_or_else(|| RefineError::Conflict("Goal has no Rounds".into()))?;
            if round_idx >= rounds.len() {
                return Err(RefineError::NotFound("Round not found".into()));
            }
            let removed_latest = round_idx + 1 == rounds.len();
            rounds.remove(round_idx);
            if !removed_latest
                && let (Some(branch), Some(latest)) = (retained_branch, rounds.last_mut())
            {
                latest["workspace_branch"] = json!(branch);
            }
            self.stop_goal_execution(id, None)?;
            // Park the Goal until the user explicitly chooses the next step. This
            // also prevents admission while the cleanup journal is being applied.
            goal["status"] = json!("backlog");
            goal["round_edit_revision"] = json!(expected_revision.saturating_add(1));
            for key in [
                "pending_event_transition",
                "pending_workflow_outcome",
                "workflow_integration_control",
                "workflow_requested_step",
            ] {
                goal.as_object_mut().unwrap().remove(key);
            }
            // These Goal-level projections describe the most recent execution;
            // the remaining Round records retain their own candidate context.
            if round_idx == goal["rounds"].as_array().unwrap().len() {
                for key in ["candidate_commit", "base_commit", "branch_name"] {
                    goal.as_object_mut().unwrap().remove(key);
                }
            }
            scrub_references(&mut goal, round_idx, true);
            goal["updated"] = json!(now_timestamp());
            // The receipt contains only the action, never a copy of the deleted Round.
            goal.as_object_mut()
                .unwrap()
                .entry("workflow_controls")
                .or_insert(json!([]))
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "request_id":uuid::Uuid::new_v4().to_string(), "forced":true,
                    "from":current.goal.status, "to":"backlog", "at":now_timestamp(),
                    "request":{"reason":"Explicit Round deletion", "actor":"operator"}
                }));
            let mut writes = Vec::new();
            let logs =
                crate::infrastructure::observability::logs::goal_logs_path(&self.refine_dir, id);
            if logs.exists() {
                let mut kept = Vec::new();
                for line in fs::read_to_string(&logs)
                    .map_err(io_error)?
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                {
                    let mut entry: Value = serde_json::from_str(line).map_err(json_error)?;
                    if entry["round_idx"].as_u64() == Some(round_idx as u64) {
                        continue;
                    }
                    scrub_references(&mut entry, round_idx, false);
                    kept.push(serde_json::to_string(&entry).map_err(json_error)?);
                }
                writes.push(json!({"path":logs,"contents":if kept.is_empty(){String::new()}else{format!("{}\n",kept.join("\n"))}}));
            }
            plan_event_records(
                &self.refine_dir,
                self.active_node_root.as_deref(),
                id,
                round_idx,
                &mut writes,
            )?;
            let plan = json!({"round_idx":round_idx,"expected_revision":expected_revision,"goal":goal,"writes":writes});
            crate::infrastructure::storage::automation::write_json(&journal, &plan)?;
            plan
        };
        let live = self.show_goal_detail(id)?;
        if workflow_revision(&live) == expected_revision {
            write_json_atomically(&path, &plan["goal"])?;
        } else if live["round_edit_revision"] != plan["goal"]["round_edit_revision"] {
            return Err(RefineError::Conflict(
                "Goal changed while Round deletion was pending".into(),
            ));
        }
        // The new occurrence is durable. Let stopped consumers release transcript
        // leases; the journal prevents further Goal writes until cleanup finishes.
        drop(_history_lock);
        drop(_lock);
        if let Some(runtime) = &self.active_node_root {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            for root in [runtime.clone(), runtime.join("agents")] {
                let supervisor =
                    crate::infrastructure::process::subprocess::FileProcessSupervisor::new(root);
                loop {
                    match supervisor.delete_round_process_records(
                        id,
                        round_idx,
                        plan["goal"]["round_edit_revision"].as_u64().unwrap(),
                    ) {
                        Ok(()) => break,
                        Err(RefineError::Conflict(_)) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(std::time::Duration::from_millis(50))
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        let _lock = self.acquire_goal_mutation_lock(id)?;
        let _history_lock =
            crate::infrastructure::process::supervisor::coordination::acquire_record_lock(
                &self.refine_dir,
                &format!("round-history-{id}"),
            )?;
        for write in plan["writes"].as_array().unwrap() {
            let destination = Path::new(write["path"].as_str().unwrap());
            if let Some(contents) = write["contents"].as_str() {
                replace_file_durably(destination, contents.as_bytes())?;
            } else {
                match fs::remove_file(destination) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(io_error(e)),
                }
            }
        }
        fs::remove_file(journal).map_err(io_error)?;
        self.show_goal_summary(id)
    }
}

fn io_error(e: std::io::Error) -> RefineError {
    RefineError::Io(e.to_string())
}
fn json_error(e: serde_json::Error) -> RefineError {
    RefineError::Serialization(e.to_string())
}

// Remove deleted-Round references and compact zero-based indexes everywhere in
// retained structured history. The top-level rounds array was already edited.
fn scrub_references(value: &mut Value, removed: usize, root: bool) {
    match value {
        Value::Array(values) => {
            values.retain(|v| {
                v["round_idx"].as_u64() != Some(removed as u64)
                    && v["source_round"].as_u64() != Some(removed as u64 + 1)
            });
            for v in values {
                scrub_references(v, removed, false);
            }
        }
        Value::Object(object) => {
            for key in ["round_idx", "previous_round"] {
                if let Some(index) = object.get(key).and_then(Value::as_u64) {
                    object.insert(
                        key.into(),
                        if index == removed as u64 {
                            Value::Null
                        } else {
                            json!(if index > removed as u64 {
                                index - 1
                            } else {
                                index
                            })
                        },
                    );
                }
            }
            if let Some(index) = object.get("source_round").and_then(Value::as_u64) {
                if index > removed as u64 + 1 {
                    object.insert("source_round".into(), json!(index - 1));
                }
            }
            for (key, child) in object {
                if key == "rounds" && !root {
                    if let Some(rounds) = child.as_array_mut() {
                        if removed < rounds.len() {
                            rounds.remove(removed);
                        }
                    }
                }
                scrub_references(child, removed, false);
            }
        }
        _ => (),
    }
}

fn json_files(root: &Path, output: &mut Vec<PathBuf>) -> RefineResult<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let kind = entry.file_type().map_err(io_error)?;
        if kind.is_dir() {
            json_files(&entry.path(), output)?;
        } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "json") {
            output.push(entry.path());
        }
    }
    Ok(())
}

fn plan_event_records(
    root: &Path,
    runtime: Option<&Path>,
    goal_id: &str,
    removed: usize,
    writes: &mut Vec<Value>,
) -> RefineResult<()> {
    let mut files = Vec::new();
    for name in [
        "invocations",
        "history",
        "goal-history",
        "pending",
        "index-updates",
        "occurrences",
        "transitions",
        "approvals",
    ] {
        json_files(&root.join("automation").join(name), &mut files)?;
    }
    if let Some(runtime) = runtime {
        for name in [
            "workflow-failures",
            "workflow-failure-fences",
            "skill-waits",
            "operations",
        ] {
            json_files(&runtime.join(name), &mut files)?;
        }
    }
    let records = files
        .into_iter()
        .map(|path| {
            let value: Value =
                serde_json::from_slice(&fs::read(&path).map_err(io_error)?).map_err(json_error)?;
            Ok((path, value))
        })
        .collect::<RefineResult<Vec<_>>>()?;
    let deleted_ids = records
        .iter()
        .filter(|(_, v)| {
            v["context"]["goal_id"] == goal_id
                && v["context"]["round_idx"].as_u64() == Some(removed as u64)
        })
        .filter_map(|(_, v)| v["id"].as_str().map(str::to_string))
        .collect::<BTreeSet<_>>();
    for (path, mut value) in records {
        let deleted = value["id"]
            .as_str()
            .is_some_and(|id| deleted_ids.contains(id));
        let own = value["context"]["goal_id"] == goal_id
            || value["goal_id"] == goal_id
            || value["goal_context"]["id"] == goal_id;
        if deleted
            || own
                && [
                    value["round_idx"].as_u64(),
                    value["occurrence"]["round_idx"].as_u64(),
                    value["context"]["round_idx"].as_u64(),
                ]
                .contains(&Some(removed as u64))
        {
            writes.push(json!({"path":path,"contents":null}));
        } else if own {
            scrub_references(&mut value, removed, false);
            writes.push(json!({"path":path,"contents":serde_json::to_string_pretty(&value).map_err(json_error)?}));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_later_human_move_resumes_interrupted_round_cleanup() {
        let root =
            std::env::temp_dir().join(format!("round-cleanup-resume-{}", uuid::Uuid::new_v4()));
        let service = FileWorkItemService::new(&root);
        service
            .create_goal_summary("Resume cleanup", Some("GOAL1"))
            .unwrap();
        for request in ["original", "delete"] {
            service
                .append_goal_round_summary("GOAL1", "User", request)
                .unwrap();
        }
        let mut goal = service.show_goal_detail("GOAL1").unwrap();
        let revision = workflow_revision(&goal);
        goal["rounds"].as_array_mut().unwrap().remove(1);
        goal["round_edit_revision"] = json!(revision + 1);
        let path = goal_json_path(&root, "GOAL1");
        let journal = path.with_file_name("round-deletion.json");
        let obstruction = root.join("obstruction");
        fs::create_dir(&obstruction).unwrap();
        let plan = json!({"round_idx":1,"expected_revision":revision,"goal":goal,"writes":[{"path":obstruction,"contents":null}]});
        crate::infrastructure::storage::automation::write_json(&journal, &plan).unwrap();
        assert!(service.delete_goal_round("GOAL1", 1, revision).is_err());
        assert!(journal.exists());
        assert_eq!(
            service.show_goal_detail("GOAL1").unwrap()["rounds"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        fs::remove_dir(&obstruction).unwrap();
        service
            .override_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        assert!(!journal.exists());
        assert_eq!(
            service.authored_goal_commitment("GOAL1").unwrap().2,
            "original"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn deletion_removes_whole_round_logs_and_skill_records_and_compacts_remaining_history() {
        let root = std::env::temp_dir().join(format!("round-delete-{}", uuid::Uuid::new_v4()));
        let service = FileWorkItemService::new(&root);
        service
            .create_goal_summary("Round deletion", Some("GOAL1"))
            .unwrap();
        for prompt in ["first", "delete me", "third"] {
            service
                .append_goal_round_summary("GOAL1", "Operator", prompt)
                .unwrap();
        }
        let logs = crate::infrastructure::observability::logs::goal_logs_path(&root, "GOAL1");
        fs::create_dir_all(logs.parent().unwrap()).unwrap();
        let entries = ["first", "delete me", "third"]
            .into_iter()
            .enumerate()
            .map(|(index, message)| {
                json!({
            "round_idx":index,"goal_id":"GOAL1","datetime":"2026-09-12T00:00:00Z","severity":"info",
            "category":"workflow","message":message,"details":null,"actions":[],"actor":"refine"
        }).to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&logs, format!("{entries}\n")).unwrap();
        for (name, value) in [
            (
                "invocations/deleted.json",
                json!({"id":"deleted","context":{"goal_id":"GOAL1","round_idx":1},"results":{"body":"deleted result"}}),
            ),
            (
                "history/deleted.json",
                json!({"id":"deleted","goal_id":"GOAL1"}),
            ),
            ("pending/default/deleted.json", json!({"id":"deleted"})),
            (
                "invocations/kept.json",
                json!({"id":"kept","context":{"goal_id":"GOAL1","round_idx":2}}),
            ),
            (
                "invocations/other.json",
                json!({"id":"other","context":{"goal_id":"OTHER","round_idx":1}}),
            ),
        ] {
            crate::infrastructure::storage::automation::write_json(
                &root.join("automation").join(name),
                &value,
            )
            .unwrap();
        }
        service
            .update_goal_git_refs("GOAL1", "refine/GOAL1/round-3", "main", "base", None)
            .unwrap();
        let revision = workflow_revision(&service.show_goal_detail("GOAL1").unwrap());
        assert!(service.delete_goal_round("GOAL1", 1, revision - 1).is_err());
        service.delete_goal_round("GOAL1", 1, revision).unwrap();
        let goal = service.show_goal_detail("GOAL1").unwrap();
        assert_eq!(goal["status"], "backlog");
        assert_eq!(goal["rounds"].as_array().unwrap().len(), 2);
        assert_eq!(goal["rounds"][1]["prompt"], "third");
        assert_eq!(
            goal["rounds"][1]["workspace_branch"],
            "refine/GOAL1/round-3"
        );
        crate::application::workflow::engine::context::validate_round_workspace_branch(
            &goal,
            "GOAL1",
            1,
            "refine/GOAL1/round-3",
            "refine/{goal_id}",
        )
        .unwrap();
        assert!(!goal.to_string().contains("delete me"));
        let stale_log = crate::model::log::LogEntry {
            datetime: now_timestamp(),
            severity: "info".into(),
            category: "workflow".into(),
            message: "late deleted output".into(),
            details: None,
            actions: Vec::new(),
            actor: None,
            goal_id: Some("GOAL1".into()),
        };
        assert!(
            service
                .append_workflow_round_log("GOAL1", 1, revision, stale_log)
                .is_err()
        );
        assert!(!fs::read_to_string(&logs).unwrap().contains("delete me"));
        assert!(
            fs::read_to_string(&logs)
                .unwrap()
                .contains("\"round_idx\":1")
        );
        assert!(!root.join("automation/invocations/deleted.json").exists());
        assert!(!root.join("automation/history/deleted.json").exists());
        assert!(
            !root
                .join("automation/pending/default/deleted.json")
                .exists()
        );
        assert!(root.join("automation/invocations/other.json").exists());
        let kept: Value = serde_json::from_slice(
            &fs::read(root.join("automation/invocations/kept.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(kept["context"]["round_idx"], 1);
        service
            .override_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        assert_eq!(
            service.authored_goal_commitment("GOAL1").unwrap().2,
            "third"
        );
        for index in [1, 0] {
            let revision = workflow_revision(&service.show_goal_detail("GOAL1").unwrap());
            service.delete_goal_round("GOAL1", index, revision).unwrap();
        }
        assert!(
            service.show_goal_detail("GOAL1").unwrap()["rounds"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(root).unwrap();
    }
}

//! Failed settlement is an unresolved outcome of a workflow occurrence, never a claim veto.
//! Runtime records are evidence inputs. Only the synchronized current occurrence can
//! make one relevant; a new decision needs no runtime cleanup.
use super::*;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static EVIDENCE: OnceLock<Mutex<BTreeMap<PathBuf, Vec<Value>>>> = OnceLock::new();

pub(super) fn remember(engine: &WorkflowEngine, value: &Value) {
    if value["settlement"].get("unpersisted_evidence").is_some() {
        let mut cache = EVIDENCE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let records = cache.entry(engine.runtime_root.clone()).or_default();
        if !records.iter().any(|record| {
            record["goal_id"] == value["goal_id"]
                && record["target_root"] == value["target_root"]
                && record["round_idx"] == value["round_idx"]
                && record["generation"] == value["generation"]
        }) {
            records.push(value.clone());
        }
    }
}

impl WorkflowEngine {
    pub(crate) fn unresolved_workflow_outcome(
        &self,
        goal_id: &str,
        goal: &Value,
    ) -> crate::error::RefineResult<Option<String>> {
        let mut evidence = EVIDENCE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&self.runtime_root)
            .cloned()
            .unwrap_or_default();
        let directory = self.runtime_root.join("workflow-failures");
        match std::fs::read_dir(&directory) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }
                    match std::fs::read(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| {
                            serde_json::from_slice::<Value>(&bytes).map_err(|e| e.to_string())
                        }) {
                        Ok(record) => evidence.push(record),
                        Err(error) => eprintln!(
                            "refine unreadable historical failure evidence {}: {error}",
                            path.display()
                        ),
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "refine workflow failure storage unavailable: {error}; retaining in-memory outcome evidence"
            ),
        }
        for record in evidence {
            if record["goal_id"] != goal_id || !self.failure_belongs_to_occurrence(&record, goal) {
                continue;
            }
            if record["settlement"].get("unpersisted_evidence").is_some() {
                return Ok(Some(format!(
                    "Current workflow step has an unresolved failed outcome: {}; persist its Error outcome or make an explicit workflow decision",
                    record["original_error"]
                        .as_str()
                        .unwrap_or("failure evidence retained")
                )));
            }
        }
        // Old fence files contain no outcome or authority. Preserve them for audit,
        // and only diagnose an unresolved legacy occurrence if the current workflow
        // still names the exact original execution, with no later lifecycle decision.
        use sha2::{Digest, Sha256};
        let key = format!(
            "{}:{goal_id}",
            self.target_root
                .as_deref()
                .unwrap_or(std::path::Path::new(""))
                .display()
        );
        let path = self
            .runtime_root
            .join("workflow-failure-fences")
            .join(format!("{:x}.json", Sha256::digest(key.as_bytes())));
        if let Ok(bytes) = std::fs::read(&path) {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(record) if self.failure_belongs_to_occurrence(&record, goal) => {
                    return Ok(Some("Current workflow step has unresolved legacy failure evidence; reconcile its Error outcome or make an explicit workflow decision".into()));
                }
                Ok(_) => {}
                Err(error) => eprintln!(
                    "refine unreadable historical failure marker {}: {error}",
                    path.display()
                ),
            }
        }
        Ok(None)
    }

    fn failure_belongs_to_occurrence(&self, record: &Value, goal: &Value) -> bool {
        let Some(rounds) = goal["rounds"].as_array() else {
            return false;
        };
        let Some(index) = rounds.len().checked_sub(1) else {
            return false;
        };
        if record["round_idx"].as_u64() != Some(index as u64) {
            return false;
        }
        if let Some(target) = record["target_root"].as_str() {
            if self.target_root.as_deref() != Some(std::path::Path::new(target)) {
                return false;
            }
        }
        if let Some(generation) = record["generation"].as_u64() {
            return goal["event_generation"].as_u64().unwrap_or(0) == generation;
        }
        // A workflow decision recorded by the occurrence-aware runtime
        // supersedes older legacy evidence even if an old process reports late
        // or its clock is ahead. These are existing Goal record revisions, not
        // another runtime authority counter.
        if goal["workflow_events"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|event| {
                event["workflow_revision"].as_u64().is_some_and(|revision| {
                    record["workflow_revision"]
                        .as_u64()
                        .is_some_and(|origin| revision > origin)
                })
            })
        {
            return false;
        }
        // Legacy files predate occurrence tokens. Map their timestamp onto the
        // synchronized lifecycle history; never consult a retained execution claim.
        let Some(failed_at) = record["failure_at"]
            .as_str()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        else {
            // A bare legacy marker only proves the unchanged post-claim record.
            // File mtimes (including backup restoration) never establish authority.
            return record["workflow_revision"]
                .as_u64()
                .and_then(|revision| revision.checked_add(1))
                == goal["workflow_revision"].as_u64();
        };
        if goal["workflow_controls"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|decision| {
                decision["request"]["expected_revision"]
                    .as_u64()
                    .is_some_and(|revision| {
                        record["workflow_revision"]
                            .as_u64()
                            .is_some_and(|origin| revision > origin)
                    })
            })
        {
            return false;
        }
        for key in ["workflow_events", "workflow_controls"] {
            if goal[key].as_array().into_iter().flatten().any(|event| {
                event["at"]
                    .as_str()
                    .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                    .is_some_and(|at| at.timestamp() > failed_at.timestamp())
            }) {
                return false;
            }
        }
        true
    }
}

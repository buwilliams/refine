//! Failed attempts remain ineligible until a new explicit decision clears their authority.
use super::*;
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};
static FAILED: OnceLock<Mutex<BTreeMap<std::path::PathBuf, WorkflowAttemptAuthority>>> =
    OnceLock::new();
impl WorkflowEngine {
    fn failure_fence_path(&self, goal: &str) -> std::path::PathBuf {
        use sha2::{Digest, Sha256};
        let key = format!(
            "{}:{goal}",
            self.target_root
                .as_deref()
                .unwrap_or(std::path::Path::new(""))
                .display()
        );
        self.runtime_root
            .join("workflow-failure-fences")
            .join(format!("{:x}.json", Sha256::digest(key.as_bytes())))
    }
    pub(crate) fn fence_failed_attempt(&self, goal: &str, authority: WorkflowAttemptAuthority) {
        let path = self.failure_fence_path(goal);
        FAILED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.clone(), authority);
        let value = json!({"round_idx":authority.round_idx,"workflow_revision":authority.workflow_revision});
        let saved = path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .map_err(|e| e.to_string())
            .and_then(|_| {
                crate::infrastructure::process::subprocess::write_json_atomically(
                    &path,
                    &serde_json::to_vec(&value).unwrap(),
                    "failed workflow admission fence",
                )
                .map_err(|e| e.to_string())
            });
        if let Err(error) = saved {
            eprintln!("Unable to persist failed workflow admission fence: {error}");
        }
    }
    pub(crate) fn failed_attempt_is_fenced(&self, goal: &str, detail: &Value) -> bool {
        let path = self.failure_fence_path(goal);
        let retained = FAILED
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&path)
            .copied()
            .or_else(|| {
                std::fs::read(&path).ok().and_then(|bytes| {
                    serde_json::from_slice::<Value>(&bytes)
                        .ok()
                        .and_then(|value| {
                            Some(WorkflowAttemptAuthority {
                                round_idx: usize::try_from(value["round_idx"].as_u64()?).ok()?,
                                workflow_revision: value["workflow_revision"].as_u64()?,
                            })
                        })
                })
            });
        retained.is_some_and(|authority| {
            let claim = &detail["rounds"][authority.round_idx]["workflow_attempt_authority"];
            claim["workflow_revision"].as_u64() == Some(authority.workflow_revision)
        })
    }
}

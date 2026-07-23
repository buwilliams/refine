use std::path::{Path, PathBuf};

use serde_json::json;

use crate::model::JsonObject;
use crate::model::goal::WorkflowQualityTiming;
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::workflow_subprocess_metadata;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::host::git_worktrees::MergeResult;
use crate::tools::host::quality::{FileQualityService, POST_BUILD, PRE_MERGE};
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::{WorkflowClaim, json_object, now_timestamp};

pub struct WorkflowContext<'a> {
    pub runtime_root: &'a Path,
    pub target_root: &'a Path,
    pub claim_id: String,
    pub goal_id: String,
    pub provider: String,
    pub execution_id: String,
    pub round_idx: usize,
    pub settings: JsonObject,
    pub work_items: FileWorkItemService,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub agent_cwd: Option<PathBuf>,
    pub provider_output: Option<String>,
    pub commit: Option<String>,
    pub implementation_changed: bool,
    pub merge: Option<MergeResult>,
    pub final_status: Option<GoalStatus>,
    quality_timing: Option<String>,
}

impl<'a> WorkflowContext<'a> {
    pub fn new(
        runtime_root: &'a Path,
        target_root: &'a Path,
        claim: WorkflowClaim,
        execution_id: &str,
        round_idx: usize,
        settings: JsonObject,
        work_items: FileWorkItemService,
    ) -> Self {
        Self {
            runtime_root,
            target_root,
            claim_id: claim.claim_id,
            goal_id: claim.goal_id,
            provider: claim.provider,
            execution_id: execution_id.to_string(),
            round_idx,
            settings,
            work_items,
            branch: None,
            worktree_path: None,
            agent_cwd: None,
            provider_output: None,
            commit: None,
            implementation_changed: false,
            merge: None,
            final_status: None,
            quality_timing: None,
        }
    }

    pub fn runtime_root(&self) -> &Path {
        self.runtime_root
    }

    pub fn target_root(&self) -> &Path {
        self.target_root
    }

    pub fn refine_dir(&self) -> PathBuf {
        self.work_items.refine_dir.clone()
    }

    pub fn request_transition(&mut self, from: GoalStatus, to: GoalStatus) -> RefineResult<()> {
        self.work_items
            .advance_automated_goal_status(&self.goal_id, to.clone())?;
        self.log(
            "state",
            &format!(
                "Workflow status changed: {} -> {}",
                from.as_str(),
                to.as_str()
            ),
            None,
        )
    }

    pub fn log(
        &self,
        category: &str,
        message: &str,
        details: Option<JsonObject>,
    ) -> RefineResult<()> {
        let mut details = details.unwrap_or_default();
        details
            .entry("execution_id".to_string())
            .or_insert_with(|| json!(&self.execution_id));
        FileLogService::new(self.refine_dir()).append_round_log(
            &self.goal_id,
            self.round_idx,
            LogEntry {
                datetime: now_timestamp(),
                severity: "info".to_string(),
                category: category.to_string(),
                message: message.to_string(),
                details: Some(details),
                actions: Vec::new(),
                actor: Some("refine".to_string()),
                goal_id: Some(self.goal_id.clone()),
            },
        )?;
        Ok(())
    }

    pub fn fail(&self, category: &str, error: &RefineError) -> RefineResult<()> {
        let _ = self.work_items.fail_automated_goal_if_active(&self.goal_id);
        self.log(
            category,
            &format!("Workflow failed: {error}"),
            Some(json_object(json!({"error": error.to_string()}))),
        )
    }

    pub fn workflow_process_metadata(&self, workflow_state: &str, behavior: &str) -> JsonObject {
        let mut metadata = workflow_subprocess_metadata(
            &self.execution_id,
            &self.goal_id,
            workflow_state,
            behavior,
            Some(self.round_idx),
        );
        metadata.insert("claim_id".to_string(), json!(&self.claim_id));
        metadata
    }

    pub fn require_branch(&self) -> RefineResult<&str> {
        self.branch
            .as_deref()
            .ok_or_else(|| missing_artifact("branch", &self.goal_id))
    }

    pub fn require_worktree_path(&self) -> RefineResult<&str> {
        self.worktree_path
            .as_deref()
            .ok_or_else(|| missing_artifact("worktree", &self.goal_id))
    }

    pub fn require_agent_cwd(&self) -> RefineResult<&Path> {
        self.agent_cwd
            .as_deref()
            .ok_or_else(|| missing_artifact("agent cwd", &self.goal_id))
    }

    pub fn require_commit(&self) -> RefineResult<&str> {
        self.commit
            .as_deref()
            .ok_or_else(|| missing_artifact("commit", &self.goal_id))
    }

    /// Returns the Quality ordering committed to this candidate round.
    ///
    /// The first validation transition durably pins the current setting. Later Build, Quality,
    /// and retry workers all reuse that value, so a settings edit cannot change which required
    /// stages the already-created candidate traverses.
    pub fn quality_timing(&mut self, current_status: GoalStatus) -> RefineResult<String> {
        if let Some(timing) = &self.quality_timing {
            return Ok(timing.clone());
        }
        let detail = self.work_items.show_goal_detail(&self.goal_id)?;
        let round = detail
            .get("rounds")
            .and_then(serde_json::Value::as_array)
            .and_then(|rounds| rounds.get(self.round_idx))
            .ok_or_else(|| {
                RefineError::NotFound(format!(
                    "Goal {} has no round {}",
                    self.goal_id,
                    self.round_idx + 1
                ))
            })?;
        if let Some(raw_timing) = round
            .get("workflow_quality_timing")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            let timing = WorkflowQualityTiming::parse_wire(raw_timing).ok_or_else(|| {
                RefineError::Serialization(format!(
                    "Goal {} round {} has invalid workflow_quality_timing {raw_timing:?}",
                    self.goal_id,
                    self.round_idx + 1
                ))
            })?;
            self.quality_timing = Some(timing.as_str().to_string());
            return Ok(timing.as_str().to_string());
        }

        // Round 6 first wrote this commitment lazily. Existing candidates that already reached
        // Build or QA therefore need a status/evidence based transition, not today's mutable
        // setting. These states unambiguously reveal the ordering that got the candidate there.
        let (timing, migrated) = match current_status {
            GoalStatus::ReadyMerge => (
                FileQualityService::new(self.refine_dir())
                    .load_settings()?
                    .timing,
                false,
            ),
            GoalStatus::Build => {
                let quality_completed = round
                    .get("quality_state")
                    .and_then(serde_json::Value::as_str)
                    == Some("passed");
                (
                    if quality_completed {
                        PRE_MERGE
                    } else {
                        POST_BUILD
                    }
                    .to_string(),
                    true,
                )
            }
            GoalStatus::Qa => {
                let build_completed = round
                    .get("logs")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|logs| {
                        logs.iter().any(|log| {
                            log.get("category").and_then(serde_json::Value::as_str) == Some("build")
                                && log.get("message").and_then(serde_json::Value::as_str)
                                    == Some("Target app build passed")
                        })
                    });
                (
                    if build_completed {
                        POST_BUILD
                    } else {
                        PRE_MERGE
                    }
                    .to_string(),
                    true,
                )
            }
            status => {
                return Err(RefineError::Conflict(format!(
                    "cannot commit Quality timing for Goal {} while it is {}",
                    self.goal_id,
                    status.as_str()
                )));
            }
        };
        let timing = WorkflowQualityTiming::parse_wire(&timing).ok_or_else(|| {
            RefineError::Serialization(format!(
                "Quality settings contain invalid timing {timing:?}"
            ))
        })?;
        self.work_items.update_goal_round_evaluation_summary(
            &self.goal_id,
            self.round_idx,
            &json!({"workflow_quality_timing": timing.as_str()}),
        )?;
        self.log(
            "quality",
            if migrated {
                "Migrated Quality timing for in-flight candidate validation"
            } else {
                "Committed Quality timing for candidate validation"
            },
            Some(crate::workflow::json_object(json!({
                "timing": timing.as_str(),
                "current_status": current_status.as_str(),
                "migration": migrated
            }))),
        )?;
        self.quality_timing = Some(timing.as_str().to_string());
        Ok(timing.as_str().to_string())
    }
}

fn missing_artifact(name: &str, goal_id: &str) -> RefineError {
    RefineError::Conflict(format!(
        "workflow artifact {name} is missing for Goal {goal_id}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::host::quality::{POST_BUILD, PRE_MERGE, QualitySettingsPatch};
    use crate::workflow::WorkflowClaimState;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn candidate_quality_timing_is_durable_across_setting_changes_and_retry_contexts() {
        let root = unique_temp_dir("workflow-quality-timing-commitment");
        let refine_dir = root.join("state");
        let runtime_root = root.join("run/8080");
        let target_root = root.join("app");
        fs::create_dir_all(&target_root).unwrap();
        let work_items = FileWorkItemService::new(&refine_dir);
        work_items
            .create_goal_summary("Pinned Quality timing", Some("GOAL1"))
            .unwrap();
        work_items
            .append_goal_round_summary("GOAL1", "Buddy", "Implement")
            .unwrap();
        FileQualityService::new(&refine_dir)
            .save_settings(QualitySettingsPatch {
                timing: Some(POST_BUILD.to_string()),
                ..Default::default()
            })
            .unwrap();
        let claim = WorkflowClaim {
            claim_id: "claim-1".to_string(),
            goal_id: "GOAL1".to_string(),
            node_id: "default".to_string(),
            provider: "smoke-ai".to_string(),
            target_app_id: target_root.display().to_string(),
            execution_id: Some("exec-1".to_string()),
            state: WorkflowClaimState::Running,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        let mut first = WorkflowContext::new(
            &runtime_root,
            &target_root,
            claim.clone(),
            "exec-1",
            0,
            JsonObject::new(),
            work_items.clone(),
        );
        let process_metadata =
            first.workflow_process_metadata("in-progress", "WorkflowImplementation");
        assert_eq!(process_metadata["claim_id"], "claim-1");
        assert_eq!(process_metadata["execution_id"], "exec-1");
        assert_eq!(process_metadata["round_idx"], 0);
        assert_eq!(
            first.quality_timing(GoalStatus::ReadyMerge).unwrap(),
            POST_BUILD
        );

        FileQualityService::new(&refine_dir)
            .save_settings(QualitySettingsPatch {
                timing: Some(PRE_MERGE.to_string()),
                ..Default::default()
            })
            .unwrap();
        let mut retry = WorkflowContext::new(
            &runtime_root,
            &target_root,
            claim,
            "exec-2",
            0,
            JsonObject::new(),
            work_items.clone(),
        );
        assert_eq!(retry.quality_timing(GoalStatus::Build).unwrap(), POST_BUILD);
        let detail = work_items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(detail["rounds"][0]["workflow_quality_timing"], POST_BUILD);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn in_flight_build_and_qa_rounds_migrate_both_timings_and_keep_them_on_retry() {
        for (status, prior_quality, prior_build, expected) in [
            (GoalStatus::Build, false, false, POST_BUILD),
            (GoalStatus::Build, true, false, PRE_MERGE),
            (GoalStatus::Qa, false, false, PRE_MERGE),
            (GoalStatus::Qa, false, true, POST_BUILD),
        ] {
            let root = unique_temp_dir(&format!("workflow-quality-migrate-{expected}"));
            let refine_dir = root.join("state");
            let runtime_root = root.join("run/8080");
            let target_root = root.join("app");
            fs::create_dir_all(&target_root).unwrap();
            let work_items = FileWorkItemService::new(&refine_dir);
            work_items
                .create_goal_summary("Migrate timing", Some("GOAL1"))
                .unwrap();
            work_items
                .append_goal_round_summary("GOAL1", "Buddy", "Implement")
                .unwrap();
            if prior_quality {
                work_items
                    .update_goal_round_evaluation_summary(
                        "GOAL1",
                        0,
                        &json!({"quality_state": "passed"}),
                    )
                    .unwrap();
            }
            if prior_build {
                FileLogService::new(&refine_dir)
                    .append_round_log(
                        "GOAL1",
                        0,
                        LogEntry {
                            datetime: now_timestamp(),
                            severity: "info".to_string(),
                            category: "build".to_string(),
                            message: "Target app build passed".to_string(),
                            details: None,
                            actions: Vec::new(),
                            actor: Some("refine".to_string()),
                            goal_id: Some("GOAL1".to_string()),
                        },
                    )
                    .unwrap();
            }
            FileQualityService::new(&refine_dir)
                .save_settings(QualitySettingsPatch {
                    timing: Some(if expected == PRE_MERGE {
                        POST_BUILD.to_string()
                    } else {
                        PRE_MERGE.to_string()
                    }),
                    ..Default::default()
                })
                .unwrap();
            let claim = WorkflowClaim {
                claim_id: "claim-1".to_string(),
                goal_id: "GOAL1".to_string(),
                node_id: "default".to_string(),
                provider: "smoke-ai".to_string(),
                target_app_id: target_root.display().to_string(),
                execution_id: Some("exec-1".to_string()),
                state: WorkflowClaimState::Running,
                created_at: "now".to_string(),
                updated_at: "now".to_string(),
            };
            let mut first = WorkflowContext::new(
                &runtime_root,
                &target_root,
                claim.clone(),
                "exec-1",
                0,
                JsonObject::new(),
                work_items.clone(),
            );
            assert_eq!(first.quality_timing(status.clone()).unwrap(), expected);

            FileQualityService::new(&refine_dir)
                .save_settings(QualitySettingsPatch {
                    timing: Some(if expected == PRE_MERGE {
                        POST_BUILD.to_string()
                    } else {
                        PRE_MERGE.to_string()
                    }),
                    ..Default::default()
                })
                .unwrap();
            let mut retry = WorkflowContext::new(
                &runtime_root,
                &target_root,
                claim,
                "exec-2",
                0,
                JsonObject::new(),
                work_items.clone(),
            );
            assert_eq!(retry.quality_timing(status).unwrap(), expected);
            assert_eq!(
                work_items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_quality_timing"],
                expected
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}

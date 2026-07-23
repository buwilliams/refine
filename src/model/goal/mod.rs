use serde::{Deserialize, Serialize};

use crate::model::log::RoundLogEntry;
use crate::model::workflow::GoalStatus;
use crate::model::{JsonObject, Timestamp};
use crate::tools::host::git_worktrees::MergeResult;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalPriority {
    Low,
    Medium,
    High,
}

impl GoalPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn parse_wire(value: &str) -> Option<Self> {
        match value {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Goal {
    pub id: String,
    pub name: String,
    pub status: GoalStatus,
    pub priority: GoalPriority,
    #[serde(default)]
    pub reporter: Option<String>,
    pub branch_name: Option<String>,
    pub feature_id: Option<String>,
    pub feature_order: Option<i64>,
    pub node_id: Option<String>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub notes: Vec<GoalNote>,
    pub rounds: Vec<GoalRound>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GoalIndexProjection {
    pub id: String,
    pub name: String,
    pub status: GoalStatus,
    pub priority: GoalPriority,
    pub reporter: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub round_count: usize,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub branch_name: Option<String>,
    pub node_id: Option<String>,
    pub feature_id: Option<String>,
    pub feature_order: Option<i64>,
    pub json_path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GoalNote {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created: Timestamp,
    pub updated: Timestamp,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GoalRound {
    pub reporter: String,
    #[serde(default)]
    pub assignee: Option<String>,
    pub prompt: String,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub guidance_decision: Option<String>,
    #[serde(default)]
    pub implementation_report: Option<String>,
    #[serde(default)]
    pub implementation_reported_at: Option<Timestamp>,
    /// Quality ordering durably committed to this candidate round.
    #[serde(default)]
    pub workflow_quality_timing: Option<WorkflowQualityTiming>,
    /// Git remote durably committed before publishing and integrating this candidate.
    #[serde(default)]
    pub workflow_git_remote: Option<String>,
    /// Successful Ready Merge integration evidence for this exact candidate.
    #[serde(default)]
    pub workflow_integration: Option<RoundIntegration>,
    pub governance: Option<RoundGovernance>,
    pub quality: Option<RoundQuality>,
    pub logs: Vec<RoundLogEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoundIntegration {
    pub candidate_commit: String,
    pub target_branch: String,
    pub target_commit: String,
    pub remote: String,
    pub pushed: bool,
    pub integrated_at: Timestamp,
    pub merge: MergeResult,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowQualityTiming {
    PreMerge,
    PostBuild,
}

impl WorkflowQualityTiming {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreMerge => "pre_merge",
            Self::PostBuild => "post_build",
        }
    }

    pub fn parse_wire(value: &str) -> Option<Self> {
        match value {
            "pre_merge" => Some(Self::PreMerge),
            "post_build" => Some(Self::PostBuild),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoundGovernance {
    pub rule_state: Option<String>,
    pub meta_rule_state: Option<String>,
    pub product_state: Option<String>,
    pub constitution_state: Option<String>,
    pub governance_message: Option<String>,
    pub governance_details: Option<JsonObject>,
    pub governance_checked_at: Option<Timestamp>,
    pub governance_rule_actions: Vec<JsonObject>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoundQuality {
    pub quality_state: Option<String>,
    pub quality_message: Option<String>,
    pub quality_details: Option<JsonObject>,
    pub quality_checked_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::{GoalRound, WorkflowQualityTiming};

    #[test]
    fn goal_round_quality_timing_serializes_compatibly_with_raw_goal_state() {
        let legacy = serde_json::json!({
            "reporter": "Buddy",
            "prompt": "Implement",
            "created": "2026-07-22T00:00:00Z",
            "updated": "2026-07-22T00:00:00Z",
            "guidance_decision": null,
            "governance": null,
            "quality": null,
            "logs": []
        });
        let legacy_round: GoalRound = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy_round.workflow_quality_timing, None);
        assert_eq!(legacy_round.workflow_git_remote, None);
        assert_eq!(legacy_round.workflow_integration, None);

        let mut current = serde_json::to_value(legacy_round).unwrap();
        current["workflow_quality_timing"] = serde_json::json!("post_build");
        let current_round: GoalRound = serde_json::from_value(current.clone()).unwrap();
        assert_eq!(
            current_round.workflow_quality_timing,
            Some(WorkflowQualityTiming::PostBuild)
        );
        assert_eq!(
            serde_json::to_value(current_round).unwrap()["workflow_quality_timing"],
            "post_build"
        );
    }
}

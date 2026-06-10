use serde::{Deserialize, Serialize};

use crate::model::log::RoundLogEntry;
use crate::model::workflow::GapStatus;
use crate::model::{JsonObject, Timestamp};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GapPriority {
    Low,
    Medium,
    High,
}

impl GapPriority {
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
pub struct Gap {
    pub id: String,
    pub name: String,
    pub status: GapStatus,
    pub priority: GapPriority,
    #[serde(default)]
    pub assignee: Option<String>,
    pub branch_name: Option<String>,
    pub feature_id: Option<String>,
    pub feature_order: Option<i64>,
    pub node_id: Option<String>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub notes: Vec<GapNote>,
    pub rounds: Vec<GapRound>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GapIndexProjection {
    pub id: String,
    pub name: String,
    pub status: GapStatus,
    pub priority: GapPriority,
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
pub struct GapNote {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created: Timestamp,
    pub updated: Timestamp,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GapRound {
    pub reporter: String,
    pub actual: String,
    pub target: String,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub guidance_decision: Option<String>,
    pub governance: Option<RoundGovernance>,
    pub quality: Option<RoundQuality>,
    pub logs: Vec<RoundLogEntry>,
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

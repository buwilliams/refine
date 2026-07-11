use serde::{Deserialize, Serialize};

use crate::model::{JsonObject, Timestamp};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LogEntry {
    pub datetime: Timestamp,
    pub severity: String,
    pub category: String,
    pub message: String,
    pub details: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<LogAction>,
    pub actor: Option<String>,
    pub goal_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActivityEntry {
    pub id: String,
    pub datetime: Timestamp,
    pub severity: String,
    pub category: String,
    pub message: String,
    pub goal_id: Option<String>,
    pub actor: Option<String>,
    pub details: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<LogAction>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RoundLogEntry {
    #[serde(flatten)]
    pub entry: LogEntry,
    pub round_idx: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LogAction {
    Link { label: String, href: String },
    Command { label: String, command: String },
    Raw { payload: JsonObject },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LogQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub goal_id: Option<String>,
    pub since_id: Option<String>,
    pub severity: Option<String>,
    pub category: Option<String>,
    pub actor: Option<String>,
    pub q: Option<String>,
    pub sort: Option<String>,
    pub direction: Option<String>,
}

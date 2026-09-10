use crate::model::JsonObject;
use serde::{Deserialize, Serialize};

pub mod integration;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct GovernanceEvaluation {
    pub(crate) failed: bool,
    pub(crate) message: Option<String>,
    pub(crate) recovery_analysis: Option<String>,
    pub(crate) recovery_round_prompt: Option<String>,
    pub(crate) details: JsonObject,
}

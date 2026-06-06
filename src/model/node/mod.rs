use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{JsonObject, Timestamp};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Node {
    pub id: String,
    pub display_name: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub archived: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NodeRegistry {
    pub nodes: Vec<Node>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActiveNodeSelection {
    pub active_node_id: String,
    pub volume_root: String,
    pub updated_at: Timestamp,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NodeSettings {
    pub application: BTreeMap<String, JsonObject>,
    pub runtime: BTreeMap<String, JsonObject>,
    pub target_app_config: BTreeMap<String, JsonObject>,
    pub target_app_runtime: BTreeMap<String, JsonObject>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NodeOwnership {
    pub node_id: String,
}

impl NodeRegistry {
    pub fn active_node_allowed(&self, active_node_id: &str) -> bool {
        self.nodes
            .iter()
            .any(|node| node.id == active_node_id && !node.archived)
    }
}

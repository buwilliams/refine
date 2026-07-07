use serde::{Deserialize, Serialize};

use crate::model::{JsonObject, Timestamp};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Node {
    pub id: String,
    pub display_name: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub settings: JsonObject,
    #[serde(default = "default_node_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ssh_host: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ssh_user: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ssh_identity_path: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default = "default_refine_checkout")]
    pub refine_checkout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_app_path: String,
    #[serde(default = "default_refine_port")]
    pub refine_port: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub provisioning: JsonObject,
    #[serde(default)]
    pub health: Option<NodeHealth>,
    #[serde(default)]
    pub archived: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NodeHealth {
    pub status: String,
    pub checked_at: Timestamp,
    pub details: Option<JsonObject>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NodeRegistry {
    pub nodes: Vec<Node>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActiveNodeSelection {
    pub active_node_id: String,
    pub refine_dir: String,
    pub updated_at: Timestamp,
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

fn default_node_enabled() -> bool {
    true
}

fn default_ssh_port() -> u16 {
    22
}

fn default_refine_checkout() -> String {
    "~/refine".to_string()
}

fn default_refine_port() -> u16 {
    8082
}

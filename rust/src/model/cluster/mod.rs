use serde::{Deserialize, Serialize};

use crate::model::{JsonObject, Timestamp};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Cluster {
    pub nodes: Vec<ClusterNode>,
    pub updated_at: Timestamp,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClusterNode {
    pub id: String,
    pub display_name: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub refine_checkout: String,
    pub target_app_path: String,
    pub refine_port: u16,
    pub enabled: bool,
    pub health: Option<ClusterHealth>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClusterHealth {
    pub status: String,
    pub checked_at: Timestamp,
    pub details: Option<JsonObject>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RemoteRunResult {
    pub node_id: String,
    pub command: String,
    pub remote_command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
}

pub fn valid_cluster_node_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

pub fn valid_ssh_host(host: &str) -> bool {
    !host.is_empty() && !host.contains('@')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_node_id_validation_matches_spec() {
        assert!(valid_cluster_node_id("node-1"));
        assert!(valid_cluster_node_id("1_node"));
        assert!(!valid_cluster_node_id(""));
        assert!(!valid_cluster_node_id("Node"));
        assert!(!valid_cluster_node_id("-node"));
        assert!(!valid_cluster_node_id("node.example"));
    }

    #[test]
    fn ssh_host_is_not_user_at_host() {
        assert!(valid_ssh_host("example.com"));
        assert!(!valid_ssh_host("user@example.com"));
    }
}

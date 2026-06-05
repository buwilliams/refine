use std::fs;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::supervisor::errors::{RefineError, RefineResult};

use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(in crate::surfaces::web_server) struct NodeRegistryDocument {
    pub(in crate::surfaces::web_server) nodes: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(in crate::surfaces::web_server) struct ClusterRegistryDocument {
    pub(in crate::surfaces::web_server) nodes: Vec<Value>,
    pub(in crate::surfaces::web_server) updated_at: String,
}

pub(in crate::surfaces::web_server) fn load_node_registry(
    durable_root: &Path,
) -> RefineResult<NodeRegistryDocument> {
    let path = durable_root.join("nodes.json");
    if !path.exists() {
        return Ok(NodeRegistryDocument {
            nodes: vec![default_node("default", "Default", false)],
        });
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read node registry {}: {error}",
            path.display()
        ))
    })?;
    let mut registry: NodeRegistryDocument = serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse node registry {}: {error}",
            path.display()
        ))
    })?;
    if !registry
        .nodes
        .iter()
        .any(|node| node_id_value(node) == "default")
    {
        registry
            .nodes
            .insert(0, default_node("default", "Default", false));
    }
    Ok(registry)
}

pub(in crate::surfaces::web_server) fn save_node_registry(
    durable_root: &Path,
    registry: &NodeRegistryDocument,
) -> RefineResult<()> {
    write_json_atomically_web(&durable_root.join("nodes.json"), &json!(registry))
}

pub(in crate::surfaces::web_server) fn load_active_node_id(
    durable_root: &Path,
) -> RefineResult<String> {
    let path = durable_root.join("active-node.json");
    if !path.exists() {
        return Ok("default".to_string());
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read active node {}: {error}",
            path.display()
        ))
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse active node {}: {error}",
            path.display()
        ))
    })?;
    Ok(value
        .get("active_node_id")
        .and_then(|value| value.as_str())
        .unwrap_or("default")
        .to_string())
}

pub(in crate::surfaces::web_server) fn save_active_node_id(
    durable_root: &Path,
    node_id: &str,
) -> RefineResult<()> {
    write_json_atomically_web(
        &durable_root.join("active-node.json"),
        &json!({
            "active_node_id": node_id,
            "updated_at": now_timestamp_web()
        }),
    )
}

pub(in crate::surfaces::web_server) fn default_node(
    id: &str,
    display_name: &str,
    active: bool,
) -> Value {
    let now = now_timestamp_web();
    json!({
        "id": id,
        "display_name": display_name,
        "archived": false,
        "active": active,
        "created_at": now,
        "updated_at": now
    })
}

pub(in crate::surfaces::web_server) fn unique_node_id(
    registry: &NodeRegistryDocument,
    display_name: &str,
) -> String {
    let base = slug_id(display_name, "node");
    if !registry
        .nodes
        .iter()
        .any(|node| node_id_value(node) == base)
    {
        return base;
    }
    for suffix in 2..1000 {
        let candidate = format!("{base}-{suffix}");
        if !registry
            .nodes
            .iter()
            .any(|node| node_id_value(node) == candidate)
        {
            return candidate;
        }
    }
    format!("{base}-{}", Utc::now().timestamp())
}

pub(in crate::surfaces::web_server) fn slug_id(value: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in value.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash && !slug.is_empty() {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        fallback.to_string()
    } else {
        slug
    }
}

pub(in crate::surfaces::web_server) fn node_id_value(node: &Value) -> &str {
    node.get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

pub(in crate::surfaces::web_server) fn node_archived(node: &Value) -> bool {
    node.get("archived")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub(in crate::surfaces::web_server) fn load_cluster_registry(
    durable_root: &Path,
) -> RefineResult<ClusterRegistryDocument> {
    let path = durable_root.join("cluster.json");
    if !path.exists() {
        return Ok(ClusterRegistryDocument {
            nodes: Vec::new(),
            updated_at: now_timestamp_web(),
        });
    }
    let bytes = fs::read(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read cluster registry {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse cluster registry {}: {error}",
            path.display()
        ))
    })
}

pub(in crate::surfaces::web_server) fn save_cluster_registry(
    durable_root: &Path,
    registry: &ClusterRegistryDocument,
) -> RefineResult<()> {
    write_json_atomically_web(&durable_root.join("cluster.json"), &json!(registry))
}

pub(in crate::surfaces::web_server) fn cluster_response(
    registry: ClusterRegistryDocument,
) -> Value {
    json!({
        "nodes": registry.nodes,
        "maintenance": null,
        "enabled": !registry.nodes.is_empty(),
        "updated_at": registry.updated_at,
        "message": if registry.nodes.is_empty() {
            "No cluster nodes configured."
        } else {
            "Cluster nodes configured."
        }
    })
}

pub(in crate::surfaces::web_server) fn default_cluster_node(id: &str) -> Value {
    let now = now_timestamp_web();
    json!({
        "id": id,
        "display_name": id,
        "ssh_host": "",
        "ssh_port": 22,
        "refine_checkout": "~/refine",
        "target_app_path": "",
        "refine_port": 8080,
        "enabled": true,
        "health": null,
        "created_at": now,
        "updated_at": now
    })
}

pub(in crate::surfaces::web_server) fn cluster_node_id_value(node: &Value) -> &str {
    node.get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

pub(in crate::surfaces::web_server) fn cluster_node_id_from_path(path: &str) -> Option<String> {
    path.strip_prefix("/cluster/nodes/")
        .and_then(|rest| rest.split('/').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

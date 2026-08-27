use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::supervisor::coordination::{record_lock_key, with_record_lock};
use crate::model::JsonObject;
use crate::model::node::{Node, NodeDisplayNameAuthority, NodeRegistry};

pub const NODE_REGISTRY_FILE: &str = "nodes.json";
pub const ACTIVE_NODE_FILE: &str = "active-node.json";
/// Subdirectory of the live state tree that `refine/state` synchronization
/// excludes as node-local runtime data.
const RUNTIME_LOCAL_DIR: &str = "runtime";

mod identity;
use identity::identity_for_node;
pub use identity::{ActiveNodeIdentity, NodeIdentity};

/// Compare node identifiers. Ids are canonically lowercase (`clean_node_id`),
/// but fleet registries and stamped goal records predating that validation
/// carry ids like `BO2LNXIPSAPP01`; a byte-exact comparison makes such an
/// owner invisible to its own node, so authority checks match
/// case-insensitively.
pub fn node_ids_match(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

#[derive(Clone, Debug)]
pub struct FileNodeRegistryService {
    pub refine_dir: PathBuf,
    /// Pre-canonical location of the active selection (a daemon port root).
    /// Read only to migrate an existing selection into `active_path()`; every
    /// resolution and write uses the canonical path.
    pub active_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NodeUpdate {
    pub display_name: Option<String>,
    pub archived: Option<bool>,
}

impl FileNodeRegistryService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            active_root: None,
        }
    }

    pub fn with_active_root(
        refine_dir: impl Into<PathBuf>,
        active_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            active_root: Some(active_root.into()),
        }
    }

    pub fn registry_path(&self) -> PathBuf {
        self.refine_dir.join(NODE_REGISTRY_FILE)
    }

    /// Serialize a complete nodes.json read-validate-modify-persist transaction
    /// with settings writes and state hydration of the same durable record.
    pub(crate) fn with_registry_lock<T>(
        &self,
        action: impl FnOnce() -> RefineResult<T>,
    ) -> RefineResult<T> {
        let path = self.registry_path();
        let key = record_lock_key(&path);
        with_record_lock(&self.refine_dir, &key, action)
    }

    /// The single authoritative location of this machine's active-node
    /// selection. It lives under `runtime/`, the subtree `refine/state`
    /// synchronization excludes, because node identity is the one fact that
    /// must never replicate between fleet nodes: a synced selection pins every
    /// node to one identity and the ownership guards then refuse all other
    /// nodes' work. Every constructor resolves the same path, so daemon and
    /// daemonless CLI can never disagree about who the active node is.
    pub fn active_path(&self) -> PathBuf {
        self.refine_dir
            .join(RUNTIME_LOCAL_DIR)
            .join(ACTIVE_NODE_FILE)
    }

    /// Where a pre-canonical daemon recorded the selection, if this service
    /// was given that root. Used only as a one-time migration source.
    fn legacy_active_path(&self) -> Option<PathBuf> {
        self.active_root
            .as_ref()
            .map(|root| root.join(ACTIVE_NODE_FILE))
    }

    pub fn active_node_id(&self) -> RefineResult<String> {
        self.active_identity().map(|identity| identity.id)
    }

    pub fn list_response(&self) -> RefineResult<serde_json::Value> {
        let registry = self.load_registry()?;
        let active_identity = self.active_identity_for_nodes(&registry.nodes)?;
        let active_node_id = &active_identity.id;
        let nodes = node_values_with_active(&registry.nodes, active_node_id);
        Ok(json!({
            "nodes": nodes,
            "active_node_id": active_node_id,
            "active_node": active_identity.display_name,
            "diagnostics": active_identity.diagnostics
        }))
    }

    pub fn list_with_counts_response(
        &self,
        counts: BTreeMap<String, BTreeMap<String, usize>>,
    ) -> RefineResult<serde_json::Value> {
        let registry = self.load_registry()?;
        let active_identity = self.active_identity_for_nodes(&registry.nodes)?;
        let active_node_id = &active_identity.id;
        let nodes = node_values_with_active(&registry.nodes, active_node_id);
        Ok(json!({
            "active_node_id": active_node_id,
            "active_node": active_identity.display_name,
            "nodes": nodes,
            "counts": counts,
            "diagnostics": active_identity.diagnostics
        }))
    }

    pub fn show(&self, id: &str) -> RefineResult<serde_json::Value> {
        let registry = self.load_registry()?;
        let active_node_id = self.active_identity_for_nodes(&registry.nodes)?.id;
        let Some(node) = registry.nodes.iter().find(|node| node.id == id) else {
            return Err(RefineError::NotFound(format!("node {id} was not found")));
        };
        Ok(json!({
            "node": node_value(node, node.id == active_node_id),
            "active": node.id == active_node_id
        }))
    }

    pub fn create(&self, id: &str) -> RefineResult<serde_json::Value> {
        let id = clean_node_id(id)?;
        self.with_registry_lock(|| {
            let mut registry = self.load_registry()?;
            if registry.nodes.iter().any(|node| node.id == id) {
                return Err(RefineError::Conflict(format!("node {id} already exists")));
            }
            let now = now_timestamp();
            registry.nodes.push(Node {
                id: id.clone(),
                display_name: id.clone(),
                display_name_authority: Some(NodeDisplayNameAuthority::System),
                created_at: now.clone(),
                updated_at: now,
                settings: JsonObject::new(),
                enabled: true,
                ssh_host: String::new(),
                ssh_user: String::new(),
                ssh_identity_path: String::new(),
                ssh_port: 22,
                refine_checkout: "~/refine".to_string(),
                target_app_path: String::new(),
                refine_port: 8082,
                health: None,
                archived: false,
            });
            self.save_registry(&registry)
        })?;
        self.show(&id)
    }

    pub fn create_with_display_name(&self, display_name: &str) -> RefineResult<Node> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            return Err(RefineError::InvalidInput(
                "display_name is required".to_string(),
            ));
        }
        self.with_registry_lock(|| {
            let mut registry = self.load_registry()?;
            let id = unique_node_id(&registry, display_name);
            let now = now_timestamp();
            let node = Node {
                id,
                display_name: display_name.to_string(),
                display_name_authority: Some(NodeDisplayNameAuthority::User),
                created_at: now.clone(),
                updated_at: now,
                settings: JsonObject::new(),
                enabled: true,
                ssh_host: String::new(),
                ssh_user: String::new(),
                ssh_identity_path: String::new(),
                ssh_port: 22,
                refine_checkout: "~/refine".to_string(),
                target_app_path: String::new(),
                refine_port: 8082,
                health: None,
                archived: false,
            };
            registry.nodes.push(node.clone());
            self.save_registry(&registry)?;
            Ok(node)
        })
    }

    pub fn activate(&self, id: &str) -> RefineResult<serde_json::Value> {
        self.with_registry_lock(|| {
            let registry = self.load_registry()?;
            if !registry.active_node_allowed(id) {
                return Err(RefineError::NotFound(format!(
                    "node {id} was not found or is archived"
                )));
            }
            self.save_active_node_id(id)
        })?;
        self.list_response()
    }

    pub fn archive(&self, id: &str) -> RefineResult<serde_json::Value> {
        self.with_registry_lock(|| {
            let mut registry = self.load_registry()?;
            let active_node_id = self.active_identity_for_nodes(&registry.nodes)?.id;
            if id == active_node_id {
                return Err(RefineError::Conflict(
                    "active node cannot be archived".to_string(),
                ));
            }
            let Some(node) = registry.nodes.iter_mut().find(|node| node.id == id) else {
                return Err(RefineError::NotFound(format!("node {id} was not found")));
            };
            node.archived = true;
            node.updated_at = now_timestamp();
            self.save_registry(&registry)
        })?;
        self.show(id)
    }

    pub fn update(&self, id: &str, update: NodeUpdate) -> RefineResult<Node> {
        self.with_registry_lock(|| {
            let mut registry = self.load_registry()?;
            let active_node_id = self.active_identity_for_nodes(&registry.nodes)?.id;
            let Some(node) = registry.nodes.iter_mut().find(|node| node.id == id) else {
                return Err(RefineError::NotFound(format!("node {id} was not found")));
            };
            if let Some(display_name) = update.display_name {
                let display_name = display_name.trim();
                if display_name.is_empty() {
                    return Err(RefineError::InvalidInput(
                        "display name cannot be empty".to_string(),
                    ));
                }
                node.display_name = display_name.to_string();
                node.display_name_authority = Some(NodeDisplayNameAuthority::User);
            }
            if let Some(archived) = update.archived {
                if archived && id == active_node_id {
                    return Err(RefineError::Conflict(
                        "active node cannot be archived".to_string(),
                    ));
                }
                node.archived = archived;
            }
            node.updated_at = now_timestamp();
            let node = node.clone();
            self.save_registry(&registry)?;
            Ok(node)
        })
    }

    pub fn rename(&self, id: &str, name: &str) -> RefineResult<serde_json::Value> {
        let name = name.trim();
        if name.is_empty() {
            return Err(RefineError::InvalidInput(
                "display name cannot be empty".to_string(),
            ));
        }
        self.with_registry_lock(|| {
            let mut registry = self.load_registry()?;
            let Some(node) = registry.nodes.iter_mut().find(|node| node.id == id) else {
                return Err(RefineError::NotFound(format!("node {id} was not found")));
            };
            node.display_name = name.to_string();
            node.display_name_authority = Some(NodeDisplayNameAuthority::User);
            node.updated_at = now_timestamp();
            self.save_registry(&registry)
        })?;
        self.show(id)
    }

    pub fn settings(&self, id: &str) -> RefineResult<serde_json::Value> {
        let registry = self.load_registry()?;
        let Some(node) = registry.nodes.iter().find(|node| node.id == id) else {
            return Err(RefineError::NotFound(format!("node {id} was not found")));
        };
        Ok(json!({
            "node_id": id,
            "settings": node.settings
        }))
    }

    pub fn ensure_transfer_target(&self, id: &str) -> RefineResult<()> {
        let registry = self.load_registry()?;
        if registry.active_node_allowed(id) {
            Ok(())
        } else {
            Err(RefineError::NotFound(format!(
                "node {id} was not found or is archived"
            )))
        }
    }

    pub(crate) fn load_registry(&self) -> RefineResult<NodeRegistry> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(NodeRegistry {
                nodes: vec![default_node("default", "Default")],
            });
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read node registry {}: {error}",
                path.display()
            ))
        })?;
        let mut registry = serde_json::from_slice::<NodeRegistry>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse node registry {}: {error}",
                path.display()
            ))
        })?;
        if !registry.nodes.iter().any(|node| node.id == "default") {
            registry.nodes.insert(0, default_node("default", "Default"));
        }
        // Registries are plain serde-parsed, so hand-edited entries bypass
        // `clean_node_id` and ids like `BO2LNXIPSAPP01` reach every byte-exact
        // comparison. Normalize to the canonical lowercase form unless that
        // would collide with an id the registry already holds.
        let mut taken: std::collections::BTreeSet<String> =
            registry.nodes.iter().map(|node| node.id.clone()).collect();
        for node in &mut registry.nodes {
            let normalized = node.id.trim().to_ascii_lowercase();
            if normalized != node.id && !normalized.is_empty() && !taken.contains(&normalized) {
                taken.insert(normalized.clone());
                node.id = normalized;
            }
        }
        Ok(registry)
    }

    pub(crate) fn save_registry(&self, registry: &NodeRegistry) -> RefineResult<()> {
        write_json(&self.registry_path(), registry)
    }

    fn save_active_node_id(&self, id: &str) -> RefineResult<()> {
        write_json(
            &self.active_path(),
            &json!({
                "active_node_id": id,
                "refine_dir": self.refine_dir.display().to_string(),
                "updated_at": now_timestamp()
            }),
        )?;
        // Retire superseded copies so they can neither be migrated later nor
        // read by a pre-canonical binary that would resurrect a stale identity.
        // The root-level copy is the one git sync used to replicate fleet-wide.
        let _ = fs::remove_file(self.refine_dir.join(ACTIVE_NODE_FILE));
        if let Some(legacy) = self.legacy_active_path() {
            let _ = fs::remove_file(legacy);
        }
        Ok(())
    }
}

pub fn detached_nodes_response(counts: BTreeMap<String, BTreeMap<String, usize>>) -> Value {
    json!({
        "active_node_id": "default",
        "active_node": "Default",
        "nodes": [node_value(&default_node("default", "Default"), true)],
        "counts": counts
    })
}

fn default_node(id: &str, display_name: &str) -> Node {
    let now = now_timestamp();
    Node {
        id: id.to_string(),
        display_name: display_name.to_string(),
        display_name_authority: Some(NodeDisplayNameAuthority::System),
        created_at: now.clone(),
        updated_at: now,
        settings: JsonObject::new(),
        enabled: true,
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_identity_path: String::new(),
        ssh_port: 22,
        refine_checkout: "~/refine".to_string(),
        target_app_path: String::new(),
        refine_port: 8082,
        health: None,
        archived: false,
    }
}

fn node_values_with_active(nodes: &[Node], active_node_id: &str) -> Vec<Value> {
    nodes
        .iter()
        .map(|node| node_value(node, node.id == active_node_id))
        .collect()
}

fn node_value(node: &Node, active: bool) -> Value {
    let identity = identity_for_node(node);
    json!({
        "id": node.id,
        "display_name": identity.display_name,
        "registry_display_name": identity.registry_display_name,
        "display_name_authority": node.display_name_authority,
        "identity_diagnostics": identity.diagnostics,
        "enabled": node.enabled,
        "ssh_host": node.ssh_host,
        "ssh_user": node.ssh_user,
        "ssh_identity_path": node.ssh_identity_path,
        "ssh_port": node.ssh_port,
        "refine_checkout": node.refine_checkout,
        "target_app_path": node.target_app_path,
        "refine_port": node.refine_port,
        "health": node.health,
        "archived": node.archived,
        "active": active,
        "created_at": node.created_at,
        "updated_at": node.updated_at
    })
}

fn unique_node_id(registry: &NodeRegistry, display_name: &str) -> String {
    let base = slug_id(display_name, "node");
    if !registry.nodes.iter().any(|node| node.id == base) {
        return base;
    }
    for suffix in 2..1000 {
        let candidate = format!("{base}-{suffix}");
        if !registry.nodes.iter().any(|node| node.id == candidate) {
            return candidate;
        }
    }
    format!("{base}-{}", Utc::now().timestamp())
}

fn slug_id(value: &str, fallback: &str) -> String {
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

fn clean_node_id(id: &str) -> RefineResult<String> {
    let id = id.trim();
    if id.is_empty()
        || !id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        return Err(RefineError::InvalidInput(
            "node id must be lowercase alphanumeric, underscore, or hyphen".to_string(),
        ));
    }
    Ok(id.to_string())
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create node registry directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let encoded = serde_json::to_string_pretty(value).map_err(|error| {
        RefineError::Serialization(format!("failed to encode node registry: {error}"))
    })?;
    // Temp-and-rename: a crash mid-write on a bare `fs::write` truncates the
    // node registry, and concurrent readers could observe the partial file.
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temp, format!("{encoded}\n")).map_err(|error| {
        RefineError::Io(format!(
            "failed to write node registry {}: {error}",
            temp.display()
        ))
    })?;
    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        RefineError::Io(format!(
            "failed to commit node registry {}: {error}",
            path.display()
        ))
    })
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests;

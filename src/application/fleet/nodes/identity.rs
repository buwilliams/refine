use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::FileNodeRegistryService;
use crate::error::{RefineError, RefineResult};
use crate::model::node::{
    ActiveNodeSelection, Node, NodeDisplayNameAuthority, NodeIdentityDiagnostic,
};

pub const DEFAULT_NODE_ID: &str = "default";
pub const DEFAULT_NODE_DISPLAY_NAME: &str = "Default";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NodeIdentity {
    pub id: String,
    pub display_name: String,
    pub registry_display_name: String,
    pub diagnostics: Vec<NodeIdentityDiagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActiveNodeIdentity {
    pub id: String,
    pub display_name: String,
    pub diagnostics: Vec<NodeIdentityDiagnostic>,
}

impl FileNodeRegistryService {
    /// Resolves the machine-local active selection against the attached project's
    /// shared registry. The durable node id is authoritative; a human label is
    /// used only when its provenance cannot misrepresent that id.
    pub fn active_identity(&self) -> RefineResult<ActiveNodeIdentity> {
        let registry = self.load_registry()?;
        self.active_identity_for_nodes(&registry.nodes)
    }

    pub(super) fn active_identity_for_nodes(
        &self,
        nodes: &[Node],
    ) -> RefineResult<ActiveNodeIdentity> {
        let (active_node_id, mut diagnostics) = self.resolve_active_selection(nodes)?;
        let node = nodes
            .iter()
            .find(|node| node.id == active_node_id)
            .or_else(|| nodes.iter().find(|node| node.id == DEFAULT_NODE_ID));
        let identity = node.map(identity_for_node).unwrap_or_else(default_identity);
        diagnostics.extend(identity.diagnostics);
        Ok(ActiveNodeIdentity {
            id: active_node_id,
            display_name: identity.display_name,
            diagnostics,
        })
    }

    pub fn node_identity(&self, id: &str) -> RefineResult<NodeIdentity> {
        let registry = self.load_registry()?;
        registry
            .nodes
            .iter()
            .find(|node| node.id == id)
            .map(identity_for_node)
            .ok_or_else(|| RefineError::NotFound(format!("node {id} was not found")))
    }

    pub fn node_identities(&self) -> RefineResult<BTreeMap<String, NodeIdentity>> {
        Ok(self
            .load_registry()?
            .nodes
            .iter()
            .map(|node| (node.id.clone(), identity_for_node(node)))
            .collect())
    }

    fn resolve_active_selection(
        &self,
        nodes: &[Node],
    ) -> RefineResult<(String, Vec<NodeIdentityDiagnostic>)> {
        let canonical = self.active_path();
        let mut diagnostics = Vec::new();
        // The canonical machine-local selection is the only authority. A
        // pre-canonical daemon port root is read once as a migration source;
        // a root-level file in the synchronized state tree is never read —
        // git sync replicated exactly such a file fleet-wide, pinning every
        // node to one machine's identity.
        let (path, migrating) = if canonical.exists() {
            (canonical, false)
        } else if let Some(legacy) = self.legacy_active_path().filter(|path| path.exists()) {
            (legacy, true)
        } else {
            if self.refine_dir.join(super::ACTIVE_NODE_FILE).exists() {
                diagnostics.push(synced_selection_diagnostic());
            }
            return Ok((DEFAULT_NODE_ID.to_string(), diagnostics));
        };
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read active node {}: {error}",
                path.display()
            ))
        })?;
        let value = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse active node {}: {error}",
                path.display()
            ))
        })?;

        let selected =
            if let Ok(selection) = serde_json::from_value::<ActiveNodeSelection>(value.clone()) {
                if !same_refine_dir(&self.refine_dir, Path::new(&selection.refine_dir)) {
                    diagnostics.push(project_mismatch_diagnostic(
                        &selection.active_node_id,
                        &selection.refine_dir,
                        &self.refine_dir,
                    ));
                    DEFAULT_NODE_ID.to_string()
                } else {
                    selection.active_node_id
                }
            } else {
                let legacy_id = value
                    .get("active_node_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .unwrap_or(DEFAULT_NODE_ID)
                    .to_string();
                if legacy_id != DEFAULT_NODE_ID {
                    diagnostics.push(legacy_selection_diagnostic(&legacy_id));
                    DEFAULT_NODE_ID.to_string()
                } else {
                    legacy_id
                }
            };
        let selected = selected.trim().to_ascii_lowercase();

        if nodes
            .iter()
            .any(|node| node.id == selected && !node.archived)
        {
            if migrating && diagnostics.is_empty() {
                // Adopt the port-root selection into the canonical location so
                // every later resolution — daemonless CLI included — agrees.
                // Best-effort: resolution must stay read-only on failure.
                let _ = self.save_active_node_id(&selected);
            }
            return Ok((selected, diagnostics));
        }
        diagnostics.push(invalid_selection_diagnostic(&selected));
        Ok((DEFAULT_NODE_ID.to_string(), diagnostics))
    }
}

pub(super) fn identity_for_node(node: &Node) -> NodeIdentity {
    let ambiguous_default = node.id == DEFAULT_NODE_ID
        && node.display_name.trim() != DEFAULT_NODE_DISPLAY_NAME
        && node.display_name_authority != Some(NodeDisplayNameAuthority::User);
    let diagnostics = if ambiguous_default {
        vec![legacy_default_label_diagnostic(&node.display_name)]
    } else {
        Vec::new()
    };
    NodeIdentity {
        id: node.id.clone(),
        display_name: if ambiguous_default {
            DEFAULT_NODE_DISPLAY_NAME.to_string()
        } else {
            node.display_name.clone()
        },
        registry_display_name: node.display_name.clone(),
        diagnostics,
    }
}

fn default_identity() -> NodeIdentity {
    NodeIdentity {
        id: DEFAULT_NODE_ID.to_string(),
        display_name: DEFAULT_NODE_DISPLAY_NAME.to_string(),
        registry_display_name: DEFAULT_NODE_DISPLAY_NAME.to_string(),
        diagnostics: Vec::new(),
    }
}

fn legacy_default_label_diagnostic(label: &str) -> NodeIdentityDiagnostic {
    NodeIdentityDiagnostic {
        code: "ambiguous_legacy_default_display_name".to_string(),
        message: format!(
            "The shared legacy default node label {label:?} has no authority metadata, so Refine is displaying the canonical label \"Default\" instead. Goal ownership remains node id \"default\"."
        ),
        recovery_action: "Run `refine node rename default \"Default\"` to restore the canonical label, or rename `default` to the intended label to explicitly confirm it.".to_string(),
    }
}

fn project_mismatch_diagnostic(
    node_id: &str,
    selected_refine_dir: &str,
    current_refine_dir: &Path,
) -> NodeIdentityDiagnostic {
    NodeIdentityDiagnostic {
        code: "active_node_selection_project_mismatch".to_string(),
        message: format!(
            "Ignored active node {node_id:?} because its runtime-local selection belongs to {selected_refine_dir}, not {}. This project is using node id \"default\".",
            current_refine_dir.display()
        ),
        recovery_action: "Run `refine node activate <node-id>` while this project is attached to establish its local active identity.".to_string(),
    }
}

fn synced_selection_diagnostic() -> NodeIdentityDiagnostic {
    NodeIdentityDiagnostic {
        code: "synced_active_node_selection_ignored".to_string(),
        message: "Ignored an active-node selection at the synchronized state root: that location replicates between fleet nodes, so it cannot identify this machine. This project is using node id \"default\" until a local selection is recorded.".to_string(),
        recovery_action: "Run `refine node activate <node-id>` on this machine to record its machine-local active identity.".to_string(),
    }
}

fn legacy_selection_diagnostic(node_id: &str) -> NodeIdentityDiagnostic {
    NodeIdentityDiagnostic {
        code: "legacy_unscoped_active_node_selection".to_string(),
        message: format!(
            "Ignored legacy runtime-local active node {node_id:?} because the selection does not identify its project. This project is using node id \"default\"."
        ),
        recovery_action: "Run `refine node activate <node-id>` while this project is attached to record a project-scoped local selection.".to_string(),
    }
}

fn invalid_selection_diagnostic(node_id: &str) -> NodeIdentityDiagnostic {
    NodeIdentityDiagnostic {
        code: "invalid_active_node_selection".to_string(),
        message: format!(
            "Ignored active node {node_id:?} because that node is missing or archived in the attached project's registry. This project is using node id \"default\"."
        ),
        recovery_action: "Create or restore the intended node, then run `refine node activate <node-id>`; otherwise activate `default`.".to_string(),
    }
}

fn same_refine_dir(current: &Path, selected: &Path) -> bool {
    comparable_path(current) == comparable_path(selected)
}

fn comparable_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

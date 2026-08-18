use std::fs;
use std::path::{Path, PathBuf};

use crate::application::fleet::nodes::{FileNodeRegistryService, NodeUpdate};
use crate::application::work_items::FileWorkItemService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner,
};
use crate::infrastructure::process::supervisor::security::FileSecurityService;
use crate::model::fleet::{
    Fleet, FleetHealth, RemoteRunResult, valid_node_id, valid_ssh_host, valid_ssh_user,
};
use crate::model::node::{Node, NodeRegistry};

// The legacy registry keeps its pre-rename on-disk name so existing synced
// state still migrates into the node registry.
pub const LEGACY_CLUSTER_REGISTRY_FILE: &str = "cluster.json";

pub const FLEET_RUNBOOK_PATH: &str = "docs/runbooks/manage-fleet.md";

/// Seed prompt for a fleet-management agent session: the runbook carries the
/// questions to ask and the CLI contract; the request carries the user's goal.
pub fn fleet_manage_prompt(checkout_path: &Path, request: &str) -> String {
    format!(
        "Manage this Refine fleet. Read {runbook} in the Refine checkout at {checkout} and \
         follow it: ask the user the questions the runbook calls for before acting, then carry \
         the request out with the documented commands. User request: {request}",
        runbook = FLEET_RUNBOOK_PATH,
        checkout = checkout_path.display(),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetBootstrapRequest {
    pub node_id: String,
    pub ssh_host: String,
    pub ssh_user: String,
    pub ssh_identity_path: String,
    pub ssh_port: u16,
    pub refine_checkout: String,
    pub target_app_path: String,
    pub refine_port: u16,
    pub dry_run: bool,
}

pub trait FleetService {
    fn registry(&self) -> RefineResult<Fleet>;
    fn transfer(&self, goal_or_feature_id: &str, node_id: &str) -> RefineResult<()>;
    fn sync(&self) -> RefineResult<()>;
    fn run_remote(&self, node_id: &str, command: &str) -> RefineResult<RemoteRunResult>;
    fn maintenance(&self, active: bool, reason: Option<String>) -> RefineResult<Fleet>;
}

#[derive(Clone, Debug)]
pub struct FileFleetService {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NodeRemoteUpdate {
    pub display_name: Option<String>,
    pub ssh_host: Option<String>,
    pub ssh_user: Option<String>,
    pub ssh_identity_path: Option<String>,
    pub ssh_port: Option<u64>,
    pub refine_checkout: Option<String>,
    pub target_app_path: Option<String>,
    pub refine_port: Option<u64>,
    pub enabled: Option<bool>,
}

impl FileFleetService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: None,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: Some(runtime_root.into()),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.refine_dir.join(LEGACY_CLUSTER_REGISTRY_FILE)
    }

    pub(crate) fn nodes(&self) -> FileNodeRegistryService {
        FileNodeRegistryService::new(&self.refine_dir)
    }

    pub fn list_response(&self) -> RefineResult<serde_json::Value> {
        let fleet = self.registry()?;
        self.identity_safe_fleet_response(fleet)
    }

    pub fn show(&self, id: &str) -> RefineResult<serde_json::Value> {
        // Preserve the legacy fleet migration side effect before projecting the
        // node through the shared identity contract.
        self.registry()?;
        let shown = self.nodes().show(id)?;
        Ok(serde_json::json!({"node": shown["node"]}))
    }

    pub fn add_node(&self, id: &str) -> RefineResult<serde_json::Value> {
        self.nodes().with_registry_lock(|| self.add_node_locked(id))
    }

    fn add_node_locked(&self, id: &str) -> RefineResult<serde_json::Value> {
        if !valid_node_id(id) {
            return Err(RefineError::InvalidInput(
                "node id must be lowercase alphanumeric, underscore, or hyphen".to_string(),
            ));
        }
        let mut registry = self.load_node_registry_with_legacy_fleet()?;
        if registry
            .nodes
            .iter()
            .any(|node| node.id == id && !node.archived)
        {
            return Err(RefineError::Conflict(format!("node {id} already exists")));
        }
        registry.nodes.push(default_node(id));
        self.save_nodes(&registry)?;
        self.identity_safe_fleet_response(self.fleet_from_registry(registry))
    }

    pub fn upsert_node(
        &self,
        id: &str,
        update: NodeRemoteUpdate,
    ) -> RefineResult<serde_json::Value> {
        self.nodes()
            .with_registry_lock(|| self.upsert_node_locked(id, update))
    }

    fn upsert_node_locked(
        &self,
        id: &str,
        update: NodeRemoteUpdate,
    ) -> RefineResult<serde_json::Value> {
        let id = id.trim();
        if !valid_node_id(id) {
            return Err(RefineError::InvalidInput(
                "node id must be lowercase alphanumeric, underscore, or hyphen".to_string(),
            ));
        }
        let mut registry = self.load_node_registry_with_legacy_fleet()?;
        let existing_index = registry.nodes.iter().position(|node| node.id == id);
        let mut node = existing_index
            .and_then(|index| registry.nodes.get(index).cloned())
            .unwrap_or_else(|| default_node(id));
        if let Some(display_name) = update.display_name {
            node.display_name = display_name.trim().to_string();
            node.display_name_authority = Some(crate::model::node::NodeDisplayNameAuthority::User);
        }
        if let Some(ssh_host) = update.ssh_host {
            let ssh_host = ssh_host.trim();
            if !valid_ssh_host(ssh_host) {
                return Err(RefineError::InvalidInput(
                    "ssh_host must be a host without user@ prefix".to_string(),
                ));
            }
            node.ssh_host = ssh_host.to_string();
        }
        if let Some(ssh_user) = update.ssh_user {
            let ssh_user = ssh_user.trim();
            if !valid_ssh_user(ssh_user) {
                return Err(RefineError::InvalidInput(
                    "ssh_user may only contain letters, numbers, dot, underscore, and hyphen"
                        .to_string(),
                ));
            }
            node.ssh_user = ssh_user.to_string();
        }
        if let Some(identity_path) = update.ssh_identity_path {
            node.ssh_identity_path = identity_path.trim().to_string();
        }
        if let Some(ssh_port) = update.ssh_port {
            node.ssh_port = port_or_default(ssh_port, 22);
        }
        if let Some(refine_port) = update.refine_port {
            node.refine_port = port_or_default(refine_port, 8082);
        }
        if let Some(refine_checkout) = update.refine_checkout {
            node.refine_checkout = refine_checkout.trim().to_string();
        }
        if let Some(target_app_path) = update.target_app_path {
            node.target_app_path = target_app_path.trim().to_string();
        }
        if let Some(enabled) = update.enabled {
            node.enabled = enabled;
        }
        node.archived = false;
        node.updated_at = now_timestamp();
        if let Some(index) = existing_index {
            registry.nodes[index] = node;
        } else {
            registry.nodes.push(node);
        }
        self.save_nodes(&registry)?;
        self.identity_safe_fleet_response(self.fleet_from_registry(registry))
    }

    pub fn bootstrap_node_response(
        &self,
        node_id: &str,
        dry_run: bool,
    ) -> RefineResult<serde_json::Value> {
        let request = self
            .nodes()
            .with_registry_lock(|| self.bootstrap_node_request_locked(node_id, dry_run))?;
        let request_snapshot = request.clone();
        let security = self.security()?;
        let result = bootstrap_remote_node_with_runtime(
            request,
            security.runtime_root,
            security.allowed_commands.iter().cloned(),
        )?;
        self.nodes().with_registry_lock(|| {
            self.settle_bootstrap_node_response_locked(&request_snapshot, result)
        })
    }

    fn bootstrap_node_request_locked(
        &self,
        node_id: &str,
        dry_run: bool,
    ) -> RefineResult<FleetBootstrapRequest> {
        let registry = self.load_node_registry_with_legacy_fleet()?;
        let Some(node) = registry
            .nodes
            .iter()
            .find(|node| node.id == node_id && !node.archived)
        else {
            return Err(RefineError::NotFound(format!(
                "node {node_id} was not found"
            )));
        };
        Ok(FleetBootstrapRequest {
            node_id: node_id.to_string(),
            ssh_host: node.ssh_host.clone(),
            ssh_user: node.ssh_user.clone(),
            ssh_identity_path: node.ssh_identity_path.clone(),
            ssh_port: node.ssh_port,
            refine_checkout: node.refine_checkout.clone(),
            target_app_path: node.target_app_path.clone(),
            refine_port: node.refine_port,
            dry_run,
        })
    }

    fn settle_bootstrap_node_response_locked(
        &self,
        request: &FleetBootstrapRequest,
        result: RemoteRunResult,
    ) -> RefineResult<serde_json::Value> {
        // Reload after the external bootstrap so the health settlement merges
        // into the latest registry instead of holding the registry lock across
        // SSH or overwriting settings and node edits made while it ran.
        let mut registry = self.load_node_registry_with_legacy_fleet()?;
        let Some(index) = registry
            .nodes
            .iter()
            .position(|node| node.id == request.node_id && !node.archived)
        else {
            return Err(RefineError::Conflict(format!(
                "node {} was removed or archived while bootstrap was running",
                request.node_id
            )));
        };
        let node = &registry.nodes[index];
        if node.ssh_host != request.ssh_host
            || node.ssh_user != request.ssh_user
            || node.ssh_identity_path != request.ssh_identity_path
            || node.ssh_port != request.ssh_port
            || node.refine_checkout != request.refine_checkout
            || node.target_app_path != request.target_app_path
            || node.refine_port != request.refine_port
        {
            return Err(RefineError::Conflict(format!(
                "node {} bootstrap settings changed while bootstrap was running",
                request.node_id
            )));
        }
        let mut details = serde_json::Map::new();
        details.insert("bootstrap".to_string(), serde_json::json!(result.clone()));
        registry.nodes[index].health = Some(FleetHealth {
            status: if result.ok { "ready" } else { "failed" }.to_string(),
            checked_at: now_timestamp(),
            details: Some(details),
        });
        registry.nodes[index].updated_at = now_timestamp();
        self.save_nodes(&registry)?;
        let fleet = self.fleet_from_registry(registry);
        Ok(serde_json::json!({
            "ok": result.ok,
            "node_id": request.node_id,
            "dry_run": request.dry_run,
            "result": result,
            "fleet": self.identity_safe_fleet_response(fleet)?
        }))
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> RefineResult<serde_json::Value> {
        self.nodes()
            .with_registry_lock(|| self.set_enabled_locked(id, enabled))
    }

    fn set_enabled_locked(&self, id: &str, enabled: bool) -> RefineResult<serde_json::Value> {
        let mut registry = self.load_node_registry_with_legacy_fleet()?;
        let Some(node) = registry
            .nodes
            .iter_mut()
            .find(|node| node.id == id && !node.archived)
        else {
            return Err(RefineError::NotFound(format!("node {id} was not found")));
        };
        node.enabled = enabled;
        node.updated_at = now_timestamp();
        self.save_nodes(&registry)?;
        self.identity_safe_fleet_response(self.fleet_from_registry(registry))
    }

    pub fn remove_node(&self, id: &str) -> RefineResult<serde_json::Value> {
        let update = NodeUpdate {
            display_name: None,
            archived: Some(true),
        };
        self.nodes().update(id, update)?;
        self.list_response()
    }

    pub fn run_remote_response(
        &self,
        node_id: &str,
        command: &str,
    ) -> RefineResult<serde_json::Value> {
        let result = self.run_remote(node_id, command)?;
        Ok(serde_json::json!({
            "ok": result.ok,
            "result": result
        }))
    }

    /// Distribute is the mechanism for moving work between nodes: it
    /// reassigns ownership of eligible Goals across enabled, healthy nodes.
    /// With `to`, all eligible Goals fill that one node; with `converge`,
    /// reviewable Goals move home to the given review node instead.
    pub fn distribute_response(
        &self,
        to: Option<&str>,
        converge: bool,
        dry_run: bool,
    ) -> RefineResult<serde_json::Value> {
        let fleet = self.registry()?;
        if converge && to.is_none() {
            return Err(RefineError::InvalidInput(
                "converge requires a target review node (--to)".to_string(),
            ));
        }
        let targets: Vec<String> = match to {
            Some(node_id) => {
                validate_remote_node_enabled(&fleet, node_id)?;
                vec![node_id.to_string()]
            }
            None => fleet
                .nodes
                .iter()
                .filter(|node| node.enabled && node_health_allows_distribution(node))
                .map(|node| node.id.clone())
                .collect(),
        };
        let result = FileWorkItemService::new(&self.refine_dir)
            .distribute_goals_across_nodes(&targets, converge, dry_run)?;
        Ok(serde_json::json!({
            "ok": true,
            "distribute": result
        }))
    }

    pub fn maintenance_response(&self) -> RefineResult<serde_json::Value> {
        let fleet = self.maintenance(true, None)?;
        Ok(serde_json::json!({
            "ok": true,
            "maintenance": {
                "active": true,
                "updated_at": fleet.updated_at
            },
            "fleet": fleet
        }))
    }

    fn save_nodes(&self, registry: &NodeRegistry) -> RefineResult<()> {
        self.nodes().save_registry(registry)
    }

    fn identity_safe_fleet_response(&self, fleet: Fleet) -> RefineResult<serde_json::Value> {
        let identities = self.nodes().node_identities()?;
        let mut value = fleet_response(fleet);
        if let Some(nodes) = value
            .get_mut("nodes")
            .and_then(serde_json::Value::as_array_mut)
        {
            for node in nodes {
                let Some(id) = node.get("id").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let Some(identity) = identities.get(id) else {
                    continue;
                };
                let Some(object) = node.as_object_mut() else {
                    continue;
                };
                object.insert(
                    "display_name".to_string(),
                    serde_json::json!(identity.display_name),
                );
                object.insert(
                    "registry_display_name".to_string(),
                    serde_json::json!(identity.registry_display_name),
                );
                object.insert(
                    "identity_diagnostics".to_string(),
                    serde_json::json!(identity.diagnostics),
                );
            }
        }
        Ok(value)
    }

    pub(crate) fn load_node_registry_with_legacy_fleet(&self) -> RefineResult<NodeRegistry> {
        self.nodes()
            .with_registry_lock(|| self.load_node_registry_with_legacy_fleet_locked())
    }

    fn load_node_registry_with_legacy_fleet_locked(&self) -> RefineResult<NodeRegistry> {
        let mut registry = self.nodes().load_registry()?;
        let Some(legacy) = self.load_legacy_fleet()? else {
            return Ok(registry);
        };

        let mut changed = false;
        for legacy_node in legacy.nodes {
            if let Some(node) = registry
                .nodes
                .iter_mut()
                .find(|node| node.id == legacy_node.id)
            {
                changed |= merge_legacy_node(node, legacy_node);
            } else {
                registry.nodes.push(legacy_node);
                changed = true;
            }
        }
        if changed {
            self.save_nodes(&registry)?;
        }
        Ok(registry)
    }

    fn load_legacy_fleet(&self) -> RefineResult<Option<Fleet>> {
        let path = self.path();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read legacy fleet registry {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<Fleet>(&bytes)
            .map(Some)
            .map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse legacy fleet registry {}: {error}",
                    path.display()
                ))
            })
    }

    fn fleet_from_registry(&self, registry: NodeRegistry) -> Fleet {
        let updated_at = registry
            .nodes
            .iter()
            .map(|node| node.updated_at.clone())
            .max()
            .unwrap_or_else(now_timestamp);
        Fleet {
            nodes: registry
                .nodes
                .into_iter()
                .filter(|node| !node.archived)
                .collect(),
            updated_at,
        }
    }
}

impl FleetService for FileFleetService {
    fn registry(&self) -> RefineResult<Fleet> {
        let registry = self.load_node_registry_with_legacy_fleet()?;
        Ok(self.fleet_from_registry(registry))
    }

    fn transfer(&self, _goal_or_feature_id: &str, node_id: &str) -> RefineResult<()> {
        validate_remote_node_enabled(&self.registry()?, node_id)
    }

    fn sync(&self) -> RefineResult<()> {
        self.registry().map(|_| ())
    }

    fn run_remote(&self, node_id: &str, command: &str) -> RefineResult<RemoteRunResult> {
        let fleet = self.registry()?;
        validate_remote_node_enabled(&fleet, node_id)?;
        let Some(node) = fleet.nodes.iter().find(|node| node.id == node_id) else {
            return Err(RefineError::NotFound(format!(
                "node {node_id} was not found"
            )));
        };
        if !valid_ssh_host(&node.ssh_host) {
            return Err(RefineError::InvalidInput(
                "ssh_host must be configured before running remote commands".to_string(),
            ));
        }
        let remote_command = command.trim().to_string();
        if remote_command.is_empty() {
            return Err(RefineError::InvalidInput("command is required".to_string()));
        }
        let security = self.security()?;
        security.authorize_host_command("fleet", &remote_command)?;
        let known_hosts_path = security.runtime_root.join("fleet-known_hosts");
        let command = ssh_display_command(
            node.ssh_port,
            &node.ssh_user,
            &node.ssh_host,
            &node.ssh_identity_path,
            &remote_command,
            Some(&known_hosts_path),
        )?;
        let ssh = ssh_process_command(
            node.ssh_port,
            &node.ssh_user,
            &node.ssh_host,
            &node.ssh_identity_path,
            &remote_command,
            Some(&known_hosts_path),
        )?;
        let output = FileProcessSupervisor::with_allowed_commands(
            security.runtime_root,
            security.allowed_commands.iter().cloned(),
        )
        .run_to_completion(ManagedProcessSpec {
            owner: ProcessOwner::Maintenance,
            command: ssh.program,
            args: ssh.args,
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: Some(remote_command.clone()),
            sensitive: false,
            metadata: Default::default(),
        })?;
        Ok(RemoteRunResult {
            node_id: node_id.to_string(),
            command,
            remote_command,
            exit_code: output.process.exit_code,
            stdout: output.stdout.trim().to_string(),
            stderr: output.stderr.trim().to_string(),
            ok: output.success(),
        })
    }

    fn maintenance(&self, _active: bool, _reason: Option<String>) -> RefineResult<Fleet> {
        self.registry()
    }
}

impl FileFleetService {
    fn security(&self) -> RefineResult<FileSecurityService> {
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or_else(|| self.refine_dir.join("runtime"));
        FileSecurityService::from_project_settings(runtime_root, &self.refine_dir)
    }
}

mod remote;

#[cfg(test)]
use super::node_sync::{
    FleetNodeDaemonClient, NODE_SYNC_DISABLED, NODE_SYNC_FAILED, NODE_SYNC_LOCAL,
    NODE_SYNC_PENDING_UPGRADE, NODE_SYNC_QUEUED, NODE_SYNC_UNREACHABLE, NODE_SYNC_UNSUPPORTED_GIT,
    NodeDaemonReply,
};
pub use remote::{bootstrap_remote_node, validate_remote_node_enabled};

use remote::*;

#[cfg(test)]
mod tests;

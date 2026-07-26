use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::cluster::{
    Cluster, ClusterHealth, RemoteRunResult, valid_node_id, valid_ssh_host, valid_ssh_user,
};
use crate::model::node::{Node, NodeRegistry};
use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::security::FileSecurityService;
use crate::tools::product::nodes::{FileNodeRegistryService, NodeUpdate};
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::{
    WORKFLOW_AUTOMATION_STATE_FILE, WorkflowAutomationState, WorkflowClaimState,
};

pub const CLUSTER_REGISTRY_FILE: &str = "cluster.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterBootstrapRequest {
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

pub trait ClusterService {
    fn registry(&self) -> RefineResult<Cluster>;
    fn transfer(&self, goal_or_feature_id: &str, node_id: &str) -> RefineResult<()>;
    fn sync(&self) -> RefineResult<()>;
    fn run_remote(&self, node_id: &str, command: &str) -> RefineResult<RemoteRunResult>;
    fn maintenance(&self, active: bool, reason: Option<String>) -> RefineResult<Cluster>;
}

#[derive(Clone, Debug)]
pub struct FileClusterService {
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

impl FileClusterService {
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
        self.refine_dir.join(CLUSTER_REGISTRY_FILE)
    }

    fn nodes(&self) -> FileNodeRegistryService {
        FileNodeRegistryService::new(&self.refine_dir)
    }

    pub fn list_response(&self) -> RefineResult<serde_json::Value> {
        let cluster = self.registry()?;
        Ok(cluster_response(cluster))
    }

    pub fn show(&self, id: &str) -> RefineResult<serde_json::Value> {
        let cluster = self.registry()?;
        let Some(node) = cluster.nodes.iter().find(|node| node.id == id) else {
            return Err(RefineError::NotFound(format!("node {id} was not found")));
        };
        Ok(serde_json::json!({"node": node}))
    }

    pub fn add_node(&self, id: &str) -> RefineResult<serde_json::Value> {
        if !valid_node_id(id) {
            return Err(RefineError::InvalidInput(
                "node id must be lowercase alphanumeric, underscore, or hyphen".to_string(),
            ));
        }
        let mut registry = self.load_node_registry_with_legacy_cluster()?;
        if registry
            .nodes
            .iter()
            .any(|node| node.id == id && !node.archived)
        {
            return Err(RefineError::Conflict(format!("node {id} already exists")));
        }
        registry.nodes.push(default_node(id));
        self.save_nodes(&registry)?;
        Ok(cluster_response(self.cluster_from_registry(registry)))
    }

    pub fn upsert_node(
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
        let mut registry = self.load_node_registry_with_legacy_cluster()?;
        let existing_index = registry.nodes.iter().position(|node| node.id == id);
        let mut node = existing_index
            .and_then(|index| registry.nodes.get(index).cloned())
            .unwrap_or_else(|| default_node(id));
        if let Some(display_name) = update.display_name {
            node.display_name = display_name.trim().to_string();
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
        Ok(cluster_response(self.cluster_from_registry(registry)))
    }

    pub fn bootstrap_node_response(
        &self,
        node_id: &str,
        dry_run: bool,
    ) -> RefineResult<serde_json::Value> {
        let mut registry = self.load_node_registry_with_legacy_cluster()?;
        let Some(index) = registry
            .nodes
            .iter()
            .position(|node| node.id == node_id && !node.archived)
        else {
            return Err(RefineError::NotFound(format!(
                "node {node_id} was not found"
            )));
        };
        let node = registry.nodes[index].clone();
        let request = ClusterBootstrapRequest {
            node_id: node_id.to_string(),
            ssh_host: node.ssh_host,
            ssh_user: node.ssh_user,
            ssh_identity_path: node.ssh_identity_path,
            ssh_port: node.ssh_port,
            refine_checkout: node.refine_checkout,
            target_app_path: node.target_app_path,
            refine_port: node.refine_port,
            dry_run,
        };
        let security = self.security()?;
        let result = bootstrap_remote_node_with_runtime(
            request,
            security.runtime_root,
            security.allowed_commands.iter().cloned(),
        )?;
        let mut details = serde_json::Map::new();
        details.insert("bootstrap".to_string(), serde_json::json!(result.clone()));
        registry.nodes[index].health = Some(ClusterHealth {
            status: if result.ok { "ready" } else { "failed" }.to_string(),
            checked_at: now_timestamp(),
            details: Some(details),
        });
        registry.nodes[index].updated_at = now_timestamp();
        self.save_nodes(&registry)?;
        let cluster = self.cluster_from_registry(registry);
        Ok(serde_json::json!({
            "ok": result.ok,
            "node_id": node_id,
            "dry_run": dry_run,
            "result": result,
            "cluster": cluster_response(cluster)
        }))
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> RefineResult<serde_json::Value> {
        let mut registry = self.load_node_registry_with_legacy_cluster()?;
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
        Ok(cluster_response(self.cluster_from_registry(registry)))
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
        let cluster = self.registry()?;
        if converge && to.is_none() {
            return Err(RefineError::InvalidInput(
                "converge requires a target review node (--to)".to_string(),
            ));
        }
        let targets: Vec<String> = match to {
            Some(node_id) => {
                validate_remote_node_enabled(&cluster, node_id)?;
                vec![node_id.to_string()]
            }
            None => cluster
                .nodes
                .iter()
                .filter(|node| node.enabled && node_health_allows_distribution(node))
                .map(|node| node.id.clone())
                .collect(),
        };
        let claimed = self.active_claim_goal_ids();
        let result = FileWorkItemService::new(&self.refine_dir)
            .distribute_goals_across_nodes(&targets, converge, &claimed, dry_run)?;
        Ok(serde_json::json!({
            "ok": true,
            "distribute": result
        }))
    }

    /// Goals with an active claim are pinned to their node; distribution only
    /// moves unclaimed work. Claims live in runtime state, so this is empty
    /// when no runtime root is configured.
    fn active_claim_goal_ids(&self) -> BTreeSet<String> {
        let Some(runtime_root) = &self.runtime_root else {
            return BTreeSet::new();
        };
        let path = runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE);
        let Ok(bytes) = fs::read(&path) else {
            return BTreeSet::new();
        };
        let Ok(state) = serde_json::from_slice::<WorkflowAutomationState>(&bytes) else {
            return BTreeSet::new();
        };
        state
            .claims
            .into_iter()
            .filter(|claim| {
                matches!(
                    claim.state,
                    WorkflowClaimState::Claimed | WorkflowClaimState::Running
                )
            })
            .map(|claim| claim.goal_id)
            .collect()
    }

    pub fn maintenance_response(&self) -> RefineResult<serde_json::Value> {
        let cluster = self.maintenance(true, None)?;
        Ok(serde_json::json!({
            "ok": true,
            "maintenance": {
                "active": true,
                "updated_at": cluster.updated_at
            },
            "cluster": cluster
        }))
    }

    fn save_nodes(&self, registry: &NodeRegistry) -> RefineResult<()> {
        self.nodes().save_registry(registry)
    }

    fn load_node_registry_with_legacy_cluster(&self) -> RefineResult<NodeRegistry> {
        let mut registry = self.nodes().load_registry()?;
        let Some(legacy) = self.load_legacy_cluster()? else {
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

    fn load_legacy_cluster(&self) -> RefineResult<Option<Cluster>> {
        let path = self.path();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read legacy cluster registry {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<Cluster>(&bytes)
            .map(Some)
            .map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse legacy cluster registry {}: {error}",
                    path.display()
                ))
            })
    }

    fn cluster_from_registry(&self, registry: NodeRegistry) -> Cluster {
        let updated_at = registry
            .nodes
            .iter()
            .map(|node| node.updated_at.clone())
            .max()
            .unwrap_or_else(now_timestamp);
        Cluster {
            nodes: registry
                .nodes
                .into_iter()
                .filter(|node| !node.archived)
                .collect(),
            updated_at,
        }
    }
}

impl ClusterService for FileClusterService {
    fn registry(&self) -> RefineResult<Cluster> {
        let registry = self.load_node_registry_with_legacy_cluster()?;
        Ok(self.cluster_from_registry(registry))
    }

    fn transfer(&self, _goal_or_feature_id: &str, node_id: &str) -> RefineResult<()> {
        validate_remote_node_enabled(&self.registry()?, node_id)
    }

    fn sync(&self) -> RefineResult<()> {
        self.registry().map(|_| ())
    }

    fn run_remote(&self, node_id: &str, command: &str) -> RefineResult<RemoteRunResult> {
        let cluster = self.registry()?;
        validate_remote_node_enabled(&cluster, node_id)?;
        let Some(node) = cluster.nodes.iter().find(|node| node.id == node_id) else {
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
        security.authorize_host_command("cluster", &remote_command)?;
        let known_hosts_path = security.runtime_root.join("cluster-known_hosts");
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

    fn maintenance(&self, _active: bool, _reason: Option<String>) -> RefineResult<Cluster> {
        self.registry()
    }
}

impl FileClusterService {
    fn security(&self) -> RefineResult<FileSecurityService> {
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or_else(|| self.refine_dir.join("runtime"));
        FileSecurityService::from_project_settings(runtime_root, &self.refine_dir)
    }
}

mod remote;

pub use remote::{bootstrap_remote_node, validate_remote_node_enabled};

use remote::*;

#[cfg(test)]
mod tests;

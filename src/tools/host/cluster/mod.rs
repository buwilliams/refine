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

/// Health is reported, not assumed: nodes without a recorded health check are
/// distributable (a fleet of one never runs bootstrap), but nodes that last
/// reported failed or deprovisioned are not.
fn node_health_allows_distribution(node: &Node) -> bool {
    node.health
        .as_ref()
        .map(|health| health.status != "failed" && health.status != "deprovisioned")
        .unwrap_or(true)
}

pub fn validate_remote_node_enabled(cluster: &Cluster, node_id: &str) -> RefineResult<()> {
    if !valid_node_id(node_id) {
        return Err(RefineError::InvalidInput(format!(
            "invalid node id {node_id}"
        )));
    }

    let Some(node) = cluster.nodes.iter().find(|node| node.id == node_id) else {
        return Err(RefineError::NotFound(format!(
            "node {node_id} was not found"
        )));
    };

    if node.enabled {
        Ok(())
    } else {
        Err(RefineError::Conflict(format!("node {node_id} is disabled")))
    }
}

pub fn bootstrap_remote_node(request: ClusterBootstrapRequest) -> RefineResult<RemoteRunResult> {
    bootstrap_remote_node_with_runtime(
        request,
        PathBuf::from("run/cluster-processes"),
        Vec::<String>::new(),
    )
}

fn bootstrap_remote_node_with_runtime(
    request: ClusterBootstrapRequest,
    runtime_root: impl Into<PathBuf>,
    allowed_commands: impl IntoIterator<Item = impl Into<String>>,
) -> RefineResult<RemoteRunResult> {
    let runtime_root = runtime_root.into();
    if !valid_node_id(&request.node_id) {
        return Err(RefineError::InvalidInput(format!(
            "invalid node id {}",
            request.node_id
        )));
    }
    if !valid_ssh_host(&request.ssh_host) {
        return Err(RefineError::InvalidInput(
            "ssh_host must be a host without user@ prefix".to_string(),
        ));
    }
    if request.ssh_port == 0 {
        return Err(RefineError::InvalidInput(
            "ssh_port must be greater than zero".to_string(),
        ));
    }
    let remote_command = bootstrap_remote_command(
        &request.refine_checkout,
        &request.target_app_path,
        request.refine_port,
    );
    let known_hosts_path = runtime_root.join("cluster-known_hosts");
    let command = ssh_display_command(
        request.ssh_port,
        &request.ssh_user,
        &request.ssh_host,
        &request.ssh_identity_path,
        &remote_command,
        Some(&known_hosts_path),
    )?;
    if request.dry_run {
        return Ok(RemoteRunResult {
            node_id: request.node_id,
            command,
            remote_command,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            ok: true,
        });
    }
    let ssh = ssh_process_command(
        request.ssh_port,
        &request.ssh_user,
        &request.ssh_host,
        &request.ssh_identity_path,
        &remote_command,
        Some(&known_hosts_path),
    )?;
    let output = FileProcessSupervisor::with_allowed_commands(runtime_root, allowed_commands)
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
        node_id: request.node_id,
        command,
        remote_command,
        exit_code: output.process.exit_code,
        stdout: output.stdout.trim().to_string(),
        stderr: output.stderr.trim().to_string(),
        ok: output.success(),
    })
}

fn bootstrap_remote_command(
    refine_checkout: &str,
    target_app_path: &str,
    refine_port: u16,
) -> String {
    let checkout = if refine_checkout.trim().is_empty() {
        "~/refine"
    } else {
        refine_checkout.trim()
    };
    let target = target_app_path.trim();
    let mut command = format!(
        "mkdir -p {checkout} && cd {checkout} && test -d .git && git pull --ff-only && printf 'refine_port={refine_port}\\n'"
    );
    if !target.is_empty() {
        command.push_str(&format!(" && test -d {}", shell_word(target)));
    }
    command
}

fn ssh_destination(user: &str, host: &str) -> RefineResult<String> {
    let user = user.trim();
    if !valid_ssh_user(user) {
        return Err(RefineError::InvalidInput(
            "ssh_user may only contain letters, numbers, dot, underscore, and hyphen".to_string(),
        ));
    }
    if user.is_empty() {
        Ok(host.to_string())
    } else {
        Ok(format!("{user}@{host}"))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HostCommand {
    program: String,
    args: Vec<String>,
}

fn ssh_process_command(
    port: u16,
    user: &str,
    host: &str,
    identity_path: &str,
    remote_command: &str,
    known_hosts_path: Option<&Path>,
) -> RefineResult<HostCommand> {
    validate_ssh_prerequisites(identity_path)?;
    let destination = ssh_destination(user, host)?;
    let mut args = ssh_common_args(port, known_hosts_path);
    let identity_path = identity_path.trim();
    if !identity_path.is_empty() {
        args.push("-i".to_string());
        args.push(identity_path.to_string());
    }
    args.push(destination);
    args.push(remote_command.to_string());
    Ok(HostCommand {
        program: "ssh".to_string(),
        args,
    })
}

fn validate_ssh_prerequisites(identity_path: &str) -> RefineResult<()> {
    ensure_ssh_binary_available()?;
    let identity_path = identity_path.trim();
    if identity_path.is_empty() {
        return Ok(());
    }
    let path = expand_identity_path(identity_path)?;
    if path.is_file() {
        return Ok(());
    }
    Err(RefineError::InvalidInput(format!(
        "ssh identity file {} was not found",
        path.display()
    )))
}

fn ensure_ssh_binary_available() -> RefineResult<()> {
    Ok(())
}

fn expand_identity_path(identity_path: &str) -> RefineResult<PathBuf> {
    if identity_path == "~" || identity_path.starts_with("~/") {
        let Some(home) = std::env::var_os("HOME") else {
            return Err(RefineError::InvalidInput(
                "ssh identity path uses ~ but HOME is not set".to_string(),
            ));
        };
        let mut path = PathBuf::from(home);
        if identity_path.len() > 2 {
            path.push(&identity_path[2..]);
        }
        return Ok(path);
    }
    if identity_path.starts_with('~') {
        return Err(RefineError::InvalidInput(
            "ssh identity path must use an absolute path, relative path, or ~/path".to_string(),
        ));
    }
    Ok(PathBuf::from(identity_path))
}

fn ssh_display_command(
    port: u16,
    user: &str,
    host: &str,
    identity_path: &str,
    remote_command: &str,
    known_hosts_path: Option<&Path>,
) -> RefineResult<String> {
    let mut parts = vec!["ssh".to_string()];
    parts.extend(
        ssh_common_args(port, known_hosts_path)
            .into_iter()
            .map(|part| {
                if known_hosts_path.is_some() && part.contains('/') {
                    shell_word(&part)
                } else {
                    part
                }
            }),
    );
    let identity_path = identity_path.trim();
    if !identity_path.is_empty() {
        parts.push("-i".to_string());
        parts.push(shell_word(identity_path));
    }
    parts.push(shell_word(&ssh_destination(user, host)?));
    parts.push(shell_word(remote_command));
    Ok(parts.join(" "))
}

fn ssh_common_args(port: u16, known_hosts_path: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        port.to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=10".to_string(),
        "-o".to_string(),
        "ServerAliveInterval=5".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=2".to_string(),
    ];
    if let Some(path) = known_hosts_path {
        args.extend([
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            "-o".to_string(),
            "LogLevel=ERROR".to_string(),
            "-o".to_string(),
            format!("UserKnownHostsFile={}", path.display()),
        ]);
    }
    args
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn default_node(id: &str) -> Node {
    let now = now_timestamp();
    Node {
        id: id.to_string(),
        display_name: id.to_string(),
        settings: Default::default(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_identity_path: String::new(),
        ssh_port: 22,
        refine_checkout: "~/refine".to_string(),
        target_app_path: String::new(),
        refine_port: 8082,
        enabled: true,
        health: None,
        created_at: now.clone(),
        updated_at: now,
        archived: false,
    }
}

fn merge_legacy_node(node: &mut Node, legacy: Node) -> bool {
    let before = node.clone();
    if node.display_name == node.id && !legacy.display_name.trim().is_empty() {
        node.display_name = legacy.display_name;
    }
    node.ssh_host = legacy.ssh_host;
    node.ssh_user = legacy.ssh_user;
    node.ssh_identity_path = legacy.ssh_identity_path;
    node.ssh_port = legacy.ssh_port;
    node.refine_checkout = legacy.refine_checkout;
    node.target_app_path = legacy.target_app_path;
    node.refine_port = legacy.refine_port;
    node.enabled = legacy.enabled;
    node.health = legacy.health;
    node.archived = false;
    if *node == before {
        return false;
    }
    node.updated_at = now_timestamp();
    true
}

fn port_or_default(value: u64, default: u16) -> u16 {
    if value == 0 {
        return default;
    }
    u16::try_from(value).unwrap_or(default)
}

fn cluster_response(cluster: Cluster) -> serde_json::Value {
    serde_json::json!({
        "nodes": cluster.nodes,
        "maintenance": null,
        "enabled": !cluster.nodes.is_empty(),
        "updated_at": cluster.updated_at,
        "message": if cluster.nodes.is_empty() {
            "No nodes configured."
        } else {
            "Nodes configured."
        }
    })
}

fn now_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::supervisor::config::FileSettingsService;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn bootstrap_remote_node_builds_dry_run_ssh_command() {
        let result = bootstrap_remote_node(ClusterBootstrapRequest {
            node_id: "node-1".to_string(),
            ssh_host: "example.com".to_string(),
            ssh_user: "deploy".to_string(),
            ssh_identity_path: "~/.ssh/refine_ed25519".to_string(),
            ssh_port: 2222,
            refine_checkout: "~/refine".to_string(),
            target_app_path: "/srv/app".to_string(),
            refine_port: 8081,
            dry_run: true,
        })
        .unwrap();
        assert!(result.ok);
        assert_eq!(result.exit_code, None);
        assert!(result.command.contains("ssh -p 2222"));
        assert!(result.command.contains("-o BatchMode=yes"));
        assert!(result.command.contains("-o ConnectTimeout=10"));
        assert!(result.command.contains("-o ServerAliveCountMax=2"));
        assert!(
            result
                .command
                .contains("-o StrictHostKeyChecking=accept-new")
        );
        assert!(result.command.contains("-o LogLevel=ERROR"));
        assert!(
            result
                .command
                .contains("-o 'UserKnownHostsFile=run/cluster-processes/cluster-known_hosts'")
        );
        assert!(result.command.contains("-i '~/.ssh/refine_ed25519'"));
        assert!(result.command.contains("'deploy@example.com'"));
        assert!(result.remote_command.contains("refine_port=8081"));
        assert!(result.remote_command.contains("/srv/app"));
    }

    #[test]
    fn bootstrap_remote_node_rejects_user_at_host() {
        let error = bootstrap_remote_node(ClusterBootstrapRequest {
            node_id: "node-1".to_string(),
            ssh_host: "user@example.com".to_string(),
            ssh_user: String::new(),
            ssh_identity_path: String::new(),
            ssh_port: 22,
            refine_checkout: String::new(),
            target_app_path: String::new(),
            refine_port: 8082,
            dry_run: true,
        })
        .unwrap_err();
        assert!(matches!(error, RefineError::InvalidInput(_)));
    }

    #[test]
    fn ssh_preflight_reports_missing_identity_file() {
        let temp_root = unique_temp_dir("cluster-ssh-preflight");
        let missing_identity = temp_root.join("missing_ed25519");

        let error = validate_ssh_prerequisites(missing_identity.to_str().unwrap()).unwrap_err();

        assert!(matches!(error, RefineError::InvalidInput(_)));
        assert!(error.to_string().contains("ssh identity file"));
    }

    #[test]
    fn ssh_command_uses_existing_identity_file() {
        let temp_root = unique_temp_dir("cluster-ssh-command");
        fs::create_dir_all(&temp_root).unwrap();
        let identity = temp_root.join("id_ed25519");
        fs::write(&identity, "").unwrap();

        let command = ssh_process_command(
            2222,
            "deploy",
            "example.com",
            identity.to_str().unwrap(),
            "printf ok",
            None,
        )
        .unwrap();

        let args = command.args;
        assert!(args.contains(&"BatchMode=yes".to_string()));
        assert!(args.contains(&"ConnectTimeout=10".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&identity.display().to_string()));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_cluster_service_manages_node_lifecycle() {
        let temp_root = unique_temp_dir("cluster");
        let refine_dir = temp_root.join(".refine");
        let service = FileClusterService::new(&refine_dir);

        assert_eq!(service.list_response().unwrap()["enabled"], true);
        service.add_node("node-1").unwrap();
        service.set_enabled("node-1", false).unwrap();
        assert_eq!(service.show("node-1").unwrap()["node"]["enabled"], false);
        service.set_enabled("node-1", true).unwrap();
        service.transfer("GOAL1", "node-1").unwrap();
        service.sync().unwrap();
        service.maintenance_response().unwrap();
        service.remove_node("node-1").unwrap();
        assert!(
            service
                .registry()
                .unwrap()
                .nodes
                .iter()
                .all(|node| node.id != "node-1")
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_cluster_service_migrates_legacy_cluster_json_to_nodes() {
        let temp_root = unique_temp_dir("cluster-legacy-migration");
        let refine_dir = temp_root.join(".refine");
        fs::create_dir_all(&refine_dir).unwrap();
        fs::write(
            refine_dir.join("cluster.json"),
            serde_json::json!({
                "nodes": [{
                    "id": "node-1",
                    "display_name": "Legacy Node",
                    "ssh_host": "example.com",
                    "ssh_user": "deploy",
                    "ssh_identity_path": "~/.ssh/refine_ed25519",
                    "ssh_port": 2222,
                    "refine_checkout": "/srv/refine",
                    "target_app_path": "/srv/app",
                    "refine_port": 18081,
                    "enabled": true,
                    "health": null,
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z"
                }],
                "updated_at": "2026-01-01T00:00:00Z"
            })
            .to_string(),
        )
        .unwrap();

        let service = FileClusterService::new(&refine_dir);
        let response = service.list_response().unwrap();
        let migrated_node = response["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == "node-1")
            .unwrap();
        assert_eq!(migrated_node["ssh_host"], "example.com");
        assert_eq!(migrated_node["ssh_port"], 2222);
        let nodes_path = refine_dir.join("nodes.json");
        let first_nodes = fs::read_to_string(&nodes_path).unwrap();

        service.list_response().unwrap();
        let second_nodes = fs::read_to_string(&nodes_path).unwrap();
        assert_eq!(first_nodes, second_nodes);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_cluster_service_authorizes_remote_run_commands() {
        let temp_root = unique_temp_dir("cluster-security");
        let refine_dir = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&refine_dir)
            .update(&serde_json::json!({"allowed_commands": "printf"}))
            .unwrap();
        let service = FileClusterService::with_runtime_root(&refine_dir, &runtime_root);
        service
            .upsert_node(
                "node-1",
                NodeRemoteUpdate {
                    ssh_host: Some("example.com".to_string()),
                    ssh_user: Some("deploy".to_string()),
                    ssh_identity_path: Some("~/.ssh/refine_ed25519".to_string()),
                    enabled: Some(true),
                    ..NodeRemoteUpdate::default()
                },
            )
            .unwrap();

        let denied = service.run_remote_response("node-1", "rm -rf target");

        assert!(matches!(denied, Err(RefineError::Unauthorized(_))));
        let audit = fs::read_to_string(runtime_root.join("security-audit.jsonl")).unwrap();
        assert!(audit.contains("\"outcome\":\"denied\""));

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn distribute_targets_only_enabled_healthy_nodes() {
        let temp_root = unique_temp_dir("cluster-distribute");
        let refine_dir = temp_root.join(".refine");
        let service = FileClusterService::new(&refine_dir);
        service.add_node("worker-up").unwrap();
        service.add_node("worker-down").unwrap();
        service.add_node("worker-broken").unwrap();
        service.set_enabled("worker-down", false).unwrap();
        {
            let registry_service = FileNodeRegistryService::new(&refine_dir);
            let mut registry = registry_service.load_registry().unwrap();
            let broken = registry
                .nodes
                .iter_mut()
                .find(|node| node.id == "worker-broken")
                .unwrap();
            broken.health = Some(ClusterHealth {
                status: "failed".to_string(),
                checked_at: now_timestamp(),
                details: None,
            });
            registry_service.save_registry(&registry).unwrap();
        }
        crate::tools::product::work_items::FileWorkItemService::new(&refine_dir)
            .create_goal_summary("Distributable", Some("GOAL1"))
            .unwrap();

        let response = service.distribute_response(None, false, true).unwrap();
        let node_ids: Vec<&str> = response["distribute"]["node_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert!(node_ids.contains(&"default"));
        assert!(node_ids.contains(&"worker-up"));
        assert!(!node_ids.contains(&"worker-down"));
        assert!(!node_ids.contains(&"worker-broken"));

        let converge_error = service.distribute_response(None, true, true).unwrap_err();
        assert!(matches!(converge_error, RefineError::InvalidInput(_)));

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }
}

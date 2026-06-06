use std::fs;
use std::path::{Path, PathBuf};

use crate::core::host::process_supervision::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner,
};
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::core::supervisor::security::FileSecurityService;
use crate::model::cluster::{
    Cluster, ClusterHealth, ClusterNode, RemoteRunResult, valid_cluster_node_id, valid_ssh_host,
    valid_ssh_user,
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
    fn transfer(&self, gap_or_feature_id: &str, node_id: &str) -> RefineResult<()>;
    fn sync(&self) -> RefineResult<()>;
    fn run_remote(&self, node_id: &str, command: &str) -> RefineResult<RemoteRunResult>;
    fn maintenance(&self, active: bool, reason: Option<String>) -> RefineResult<Cluster>;
}

#[derive(Clone, Debug)]
pub struct FileClusterRegistryService {
    pub durable_root: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClusterNodeUpdate {
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

impl FileClusterRegistryService {
    pub fn new(durable_root: impl Into<PathBuf>) -> Self {
        Self {
            durable_root: durable_root.into(),
            runtime_root: None,
        }
    }

    pub fn with_runtime_root(
        durable_root: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            durable_root: durable_root.into(),
            runtime_root: Some(runtime_root.into()),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.durable_root.join(CLUSTER_REGISTRY_FILE)
    }

    pub fn list_response(&self) -> RefineResult<serde_json::Value> {
        let cluster = self.registry()?;
        Ok(cluster_response(cluster))
    }

    pub fn show(&self, id: &str) -> RefineResult<serde_json::Value> {
        let cluster = self.registry()?;
        let Some(node) = cluster.nodes.iter().find(|node| node.id == id) else {
            return Err(RefineError::NotFound(format!(
                "cluster node {id} was not found"
            )));
        };
        Ok(serde_json::json!({"node": node}))
    }

    pub fn add_node(&self, id: &str) -> RefineResult<serde_json::Value> {
        if !valid_cluster_node_id(id) {
            return Err(RefineError::InvalidInput(
                "cluster node id must be lowercase alphanumeric, underscore, or hyphen".to_string(),
            ));
        }
        let mut cluster = self.registry()?;
        if cluster.nodes.iter().any(|node| node.id == id) {
            return Err(RefineError::Conflict(format!(
                "cluster node {id} already exists"
            )));
        }
        cluster.nodes.push(default_cluster_node(id));
        cluster.updated_at = now_timestamp();
        self.save(&cluster)?;
        Ok(cluster_response(cluster))
    }

    pub fn upsert_node(
        &self,
        id: &str,
        update: ClusterNodeUpdate,
    ) -> RefineResult<serde_json::Value> {
        let id = id.trim();
        if !valid_cluster_node_id(id) {
            return Err(RefineError::InvalidInput(
                "cluster node id must be lowercase alphanumeric, underscore, or hyphen".to_string(),
            ));
        }
        let mut cluster = self.registry()?;
        let existing_index = cluster.nodes.iter().position(|node| node.id == id);
        let mut node = existing_index
            .and_then(|index| cluster.nodes.get(index).cloned())
            .unwrap_or_else(|| default_cluster_node(id));
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
            node.refine_port = port_or_default(refine_port, 8080);
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
        node.updated_at = now_timestamp();
        if let Some(index) = existing_index {
            cluster.nodes[index] = node;
        } else {
            cluster.nodes.push(node);
        }
        cluster.updated_at = now_timestamp();
        self.save(&cluster)?;
        Ok(cluster_response(cluster))
    }

    pub fn bootstrap_node_response(
        &self,
        node_id: &str,
        dry_run: bool,
    ) -> RefineResult<serde_json::Value> {
        let mut cluster = self.registry()?;
        let Some(index) = cluster.nodes.iter().position(|node| node.id == node_id) else {
            return Err(RefineError::NotFound(format!(
                "cluster node {node_id} was not found"
            )));
        };
        let node = cluster.nodes[index].clone();
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
        cluster.nodes[index].health = Some(ClusterHealth {
            status: if result.ok { "ready" } else { "failed" }.to_string(),
            checked_at: now_timestamp(),
            details: Some(details),
        });
        cluster.updated_at = now_timestamp();
        self.save(&cluster)?;
        Ok(serde_json::json!({
            "ok": result.ok,
            "node_id": node_id,
            "dry_run": dry_run,
            "result": result,
            "cluster": cluster_response(cluster)
        }))
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> RefineResult<serde_json::Value> {
        let mut cluster = self.registry()?;
        let Some(node) = cluster.nodes.iter_mut().find(|node| node.id == id) else {
            return Err(RefineError::NotFound(format!(
                "cluster node {id} was not found"
            )));
        };
        node.enabled = enabled;
        node.updated_at = now_timestamp();
        cluster.updated_at = now_timestamp();
        self.save(&cluster)?;
        Ok(cluster_response(cluster))
    }

    pub fn remove_node(&self, id: &str) -> RefineResult<serde_json::Value> {
        let mut cluster = self.registry()?;
        let before = cluster.nodes.len();
        cluster.nodes.retain(|node| node.id != id);
        if cluster.nodes.len() == before {
            return Err(RefineError::NotFound(format!(
                "cluster node {id} was not found"
            )));
        }
        cluster.updated_at = now_timestamp();
        self.save(&cluster)?;
        Ok(cluster_response(cluster))
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

    pub fn sync_response(&self) -> RefineResult<serde_json::Value> {
        let cluster = self.registry()?;
        Ok(serde_json::json!({
            "ok": true,
            "synced": cluster.nodes.iter().filter(|node| node.enabled).count(),
            "cluster": cluster
        }))
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

    fn save(&self, cluster: &Cluster) -> RefineResult<()> {
        write_json(&self.path(), cluster)
    }
}

impl ClusterService for FileClusterRegistryService {
    fn registry(&self) -> RefineResult<Cluster> {
        let path = self.path();
        if !path.exists() {
            return Ok(Cluster {
                nodes: Vec::new(),
                updated_at: now_timestamp(),
            });
        }
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read cluster registry {}: {error}",
                path.display()
            ))
        })?;
        serde_json::from_slice::<Cluster>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse cluster registry {}: {error}",
                path.display()
            ))
        })
    }

    fn transfer(&self, _gap_or_feature_id: &str, node_id: &str) -> RefineResult<()> {
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
                "cluster node {node_id} was not found"
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
        self.security()?
            .authorize_host_command("cluster", &remote_command)?;
        let command = ssh_display_command(
            node.ssh_port,
            &node.ssh_user,
            &node.ssh_host,
            &node.ssh_identity_path,
            &remote_command,
        )?;
        let ssh = ssh_process_command(
            node.ssh_port,
            &node.ssh_user,
            &node.ssh_host,
            &node.ssh_identity_path,
            &remote_command,
        )?;
        let security = self.security()?;
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
        let mut cluster = self.registry()?;
        cluster.updated_at = now_timestamp();
        self.save(&cluster)?;
        Ok(cluster)
    }
}

impl FileClusterRegistryService {
    fn security(&self) -> RefineResult<FileSecurityService> {
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or_else(|| self.durable_root.join("runtime"));
        FileSecurityService::from_project_settings(runtime_root, &self.durable_root)
    }
}

pub fn validate_remote_node_enabled(cluster: &Cluster, node_id: &str) -> RefineResult<()> {
    if !valid_cluster_node_id(node_id) {
        return Err(RefineError::InvalidInput(format!(
            "invalid cluster node id {node_id}"
        )));
    }

    let Some(node) = cluster.nodes.iter().find(|node| node.id == node_id) else {
        return Err(RefineError::NotFound(format!(
            "cluster node {node_id} was not found"
        )));
    };

    if node.enabled {
        Ok(())
    } else {
        Err(RefineError::Conflict(format!(
            "cluster node {node_id} is disabled"
        )))
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
    if !valid_cluster_node_id(&request.node_id) {
        return Err(RefineError::InvalidInput(format!(
            "invalid cluster node id {}",
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
    let command = ssh_display_command(
        request.ssh_port,
        &request.ssh_user,
        &request.ssh_host,
        &request.ssh_identity_path,
        &remote_command,
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
) -> RefineResult<HostCommand> {
    validate_ssh_prerequisites(identity_path)?;
    let destination = ssh_destination(user, host)?;
    let mut args = ssh_common_args(port);
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
) -> RefineResult<String> {
    let mut parts = vec!["ssh".to_string()];
    parts.extend(ssh_common_args(port));
    let identity_path = identity_path.trim();
    if !identity_path.is_empty() {
        parts.push("-i".to_string());
        parts.push(shell_word(identity_path));
    }
    parts.push(shell_word(&ssh_destination(user, host)?));
    parts.push(shell_word(remote_command));
    Ok(parts.join(" "))
}

fn ssh_common_args(port: u16) -> Vec<String> {
    vec![
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
    ]
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn default_cluster_node(id: &str) -> ClusterNode {
    let now = now_timestamp();
    ClusterNode {
        id: id.to_string(),
        display_name: id.to_string(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_identity_path: String::new(),
        ssh_port: 22,
        refine_checkout: "~/refine".to_string(),
        target_app_path: String::new(),
        refine_port: 8080,
        enabled: true,
        health: None,
        created_at: now.clone(),
        updated_at: now,
    }
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
            "No cluster nodes configured."
        } else {
            "Cluster nodes configured."
        }
    })
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create cluster registry directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let encoded = serde_json::to_string_pretty(value).map_err(|error| {
        RefineError::Serialization(format!("failed to encode cluster registry: {error}"))
    })?;
    fs::write(path, format!("{encoded}\n")).map_err(|error| {
        RefineError::Io(format!(
            "failed to write cluster registry {}: {error}",
            path.display()
        ))
    })
}

fn now_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::supervisor::config::FileSettingsService;
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
            refine_port: 8080,
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
    fn file_cluster_registry_manages_node_lifecycle() {
        let temp_root = unique_temp_dir("cluster");
        let durable_root = temp_root.join(".refine");
        let service = FileClusterRegistryService::new(&durable_root);

        assert_eq!(service.list_response().unwrap()["enabled"], false);
        service.add_node("node-1").unwrap();
        service.set_enabled("node-1", false).unwrap();
        assert_eq!(service.show("node-1").unwrap()["node"]["enabled"], false);
        service.set_enabled("node-1", true).unwrap();
        service.transfer("GAP1", "node-1").unwrap();
        service.sync().unwrap();
        service.maintenance_response().unwrap();
        service.remove_node("node-1").unwrap();
        assert_eq!(service.registry().unwrap().nodes.len(), 0);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_cluster_registry_authorizes_remote_run_commands() {
        let temp_root = unique_temp_dir("cluster-security");
        let durable_root = temp_root.join(".refine");
        let runtime_root = temp_root.join("run/8080");
        FileSettingsService::new(&durable_root)
            .update(&serde_json::json!({"allowed_commands": "printf"}))
            .unwrap();
        let service = FileClusterRegistryService::with_runtime_root(&durable_root, &runtime_root);
        service
            .upsert_node(
                "node-1",
                ClusterNodeUpdate {
                    ssh_host: Some("example.com".to_string()),
                    ssh_user: Some("deploy".to_string()),
                    ssh_identity_path: Some("~/.ssh/refine_ed25519".to_string()),
                    enabled: Some(true),
                    ..ClusterNodeUpdate::default()
                },
            )
            .unwrap();

        let denied = service.run_remote_response("node-1", "rm -rf target");

        assert!(matches!(denied, Err(RefineError::Unauthorized(_))));
        let audit = fs::read_to_string(runtime_root.join("security-audit.jsonl")).unwrap();
        assert!(audit.contains("\"outcome\":\"denied\""));

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

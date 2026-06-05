use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::cluster::{
    Cluster, ClusterHealth, ClusterNode, RemoteRunResult, valid_cluster_node_id, valid_ssh_host,
};

pub const CLUSTER_REGISTRY_FILE: &str = "cluster.json";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClusterBootstrapRequest {
    pub node_id: String,
    pub ssh_host: String,
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
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ClusterNodeUpdate {
    pub display_name: Option<String>,
    pub ssh_host: Option<String>,
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
        let result = bootstrap_remote_node(ClusterBootstrapRequest {
            node_id: node_id.to_string(),
            ssh_host: node.ssh_host,
            ssh_port: node.ssh_port,
            refine_checkout: node.refine_checkout,
            target_app_path: node.target_app_path,
            refine_port: node.refine_port,
            dry_run,
        })?;
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
        let output = Command::new("ssh")
            .arg("-p")
            .arg(node.ssh_port.to_string())
            .arg(&node.ssh_host)
            .arg(&remote_command)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run ssh command: {error}")))?;
        Ok(RemoteRunResult {
            node_id: node_id.to_string(),
            command: format!(
                "ssh -p {} {} {}",
                node.ssh_port,
                shell_word(&node.ssh_host),
                shell_word(&remote_command)
            ),
            remote_command,
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ok: output.status.success(),
        })
    }

    fn maintenance(&self, _active: bool, _reason: Option<String>) -> RefineResult<Cluster> {
        let mut cluster = self.registry()?;
        cluster.updated_at = now_timestamp();
        self.save(&cluster)?;
        Ok(cluster)
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
    let command = format!(
        "ssh -p {} {} {}",
        request.ssh_port,
        shell_word(&request.ssh_host),
        shell_word(&remote_command)
    );
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
    let output = Command::new("ssh")
        .arg("-p")
        .arg(request.ssh_port.to_string())
        .arg(&request.ssh_host)
        .arg(&remote_command)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run ssh bootstrap: {error}")))?;
    Ok(RemoteRunResult {
        node_id: request.node_id,
        command,
        remote_command,
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ok: output.status.success(),
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

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn default_cluster_node(id: &str) -> ClusterNode {
    let now = now_timestamp();
    ClusterNode {
        id: id.to_string(),
        display_name: id.to_string(),
        ssh_host: String::new(),
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
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn bootstrap_remote_node_builds_dry_run_ssh_command() {
        let result = bootstrap_remote_node(ClusterBootstrapRequest {
            node_id: "node-1".to_string(),
            ssh_host: "example.com".to_string(),
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
        assert!(result.remote_command.contains("refine_port=8081"));
        assert!(result.remote_command.contains("/srv/app"));
    }

    #[test]
    fn bootstrap_remote_node_rejects_user_at_host() {
        let error = bootstrap_remote_node(ClusterBootstrapRequest {
            node_id: "node-1".to_string(),
            ssh_host: "user@example.com".to_string(),
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

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}

use super::*;

/// Health is reported, not assumed: nodes without a recorded health check are
/// distributable (a fleet of one never runs bootstrap), but nodes that last
/// reported failed or deprovisioned are not.
pub(super) fn node_health_allows_distribution(node: &Node) -> bool {
    node.health
        .as_ref()
        .map(|health| health.status != "failed" && health.status != "deprovisioned")
        .unwrap_or(true)
}

pub fn validate_remote_node_enabled(fleet: &Fleet, node_id: &str) -> RefineResult<()> {
    if !valid_node_id(node_id) {
        return Err(RefineError::InvalidInput(format!(
            "invalid node id {node_id}"
        )));
    }

    let Some(node) = fleet.nodes.iter().find(|node| node.id == node_id) else {
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

pub fn bootstrap_remote_node(request: FleetBootstrapRequest) -> RefineResult<RemoteRunResult> {
    bootstrap_remote_node_with_runtime(
        request,
        PathBuf::from("run/fleet-processes"),
        Vec::<String>::new(),
    )
}

pub(super) fn bootstrap_remote_node_with_runtime(
    request: FleetBootstrapRequest,
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
    let known_hosts_path = runtime_root.join("fleet-known_hosts");
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

pub(super) fn bootstrap_remote_command(
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

pub(super) fn ssh_destination(user: &str, host: &str) -> RefineResult<String> {
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
pub(super) struct HostCommand {
    pub(super) program: String,
    pub(super) args: Vec<String>,
}

pub(super) fn ssh_process_command(
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

pub(super) fn validate_ssh_prerequisites(identity_path: &str) -> RefineResult<()> {
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

pub(super) fn ensure_ssh_binary_available() -> RefineResult<()> {
    Ok(())
}

pub(super) fn expand_identity_path(identity_path: &str) -> RefineResult<PathBuf> {
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

pub(super) fn ssh_display_command(
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

pub(super) fn ssh_common_args(port: u16, known_hosts_path: Option<&Path>) -> Vec<String> {
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

pub(super) fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn default_node(id: &str) -> Node {
    let now = now_timestamp();
    Node {
        id: id.to_string(),
        display_name: id.to_string(),
        display_name_authority: Some(crate::model::node::NodeDisplayNameAuthority::System),
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

pub(super) fn merge_legacy_node(node: &mut Node, legacy: Node) -> bool {
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

pub(super) fn port_or_default(value: u64, default: u16) -> u16 {
    if value == 0 {
        return default;
    }
    u16::try_from(value).unwrap_or(default)
}

pub(super) fn fleet_response(fleet: Fleet) -> serde_json::Value {
    serde_json::json!({
        "nodes": fleet.nodes,
        "maintenance": null,
        "enabled": !fleet.nodes.is_empty(),
        "updated_at": fleet.updated_at,
        "message": if fleet.nodes.is_empty() {
            "No nodes configured."
        } else {
            "Nodes configured."
        }
    })
}

pub(super) fn now_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

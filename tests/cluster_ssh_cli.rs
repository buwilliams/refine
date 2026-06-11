mod support;

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use support::integration::IntegrationFixture;

#[test]
#[ignore = "requires docker compose and OpenSSH client tools"]
fn cluster_cli_runs_commands_over_real_ssh_container() {
    let ssh = DockerSshFixture::start();
    let fixture = IntegrationFixture::start("cluster-ssh");

    let settings = fixture.api_json(
        "PATCH",
        "/api/settings",
        json!({"allowed_commands": "printf\nsh\nmkdir"}),
    );
    assert_eq!(
        settings["settings"]["allowed_commands"],
        "printf\nsh\nmkdir"
    );

    let add = fixture.run_refine(&["cluster", "add-node", "docker-ssh"]);
    fixture.assert_success("cluster add-node docker-ssh", &add);

    let ssh_port = ssh.port.to_string();
    let identity = ssh.identity.display().to_string();
    let edit = fixture.run_refine(&[
        "cluster",
        "edit-node",
        "docker-ssh",
        "--display-name",
        "Docker SSH",
        "--ssh-host",
        "127.0.0.1",
        "--ssh-user",
        "refine",
        "--ssh-identity-path",
        &identity,
        "--ssh-port",
        &ssh_port,
        "--refine-checkout",
        "/tmp/refine",
        "--target-app-path",
        "/tmp/app",
        "--refine-port",
        "18081",
    ]);
    fixture.assert_success("cluster edit-node docker-ssh", &edit);

    let show = fixture.run_refine(&["cluster", "show", "docker-ssh"]);
    fixture.assert_success("cluster show docker-ssh", &show);
    let node = fixture.json_stdout(&show);
    assert_eq!(node["node"]["ssh_host"], "127.0.0.1");
    assert_eq!(node["node"]["ssh_user"], "refine");
    assert_eq!(node["node"]["ssh_port"], ssh.port);

    let dry_bootstrap = fixture.run_refine(&["cluster", "bootstrap", "docker-ssh", "--dry-run"]);
    fixture.assert_success("cluster bootstrap docker-ssh dry-run", &dry_bootstrap);
    let dry_bootstrap_payload = fixture.json_stdout(&dry_bootstrap);
    assert_eq!(
        dry_bootstrap_payload["ok"], true,
        "{dry_bootstrap_payload:#}"
    );
    assert_eq!(
        dry_bootstrap_payload["dry_run"], true,
        "{dry_bootstrap_payload:#}"
    );
    assert!(
        dry_bootstrap_payload["result"]["command"]
            .as_str()
            .unwrap_or_default()
            .contains("refine@127.0.0.1"),
        "{dry_bootstrap_payload:#}"
    );
    assert_eq!(
        node_in_cluster_payload(&dry_bootstrap_payload, "docker-ssh")["health"]["status"],
        "ready",
        "{dry_bootstrap_payload:#}"
    );

    fixture.assert_success(
        "cluster disable-node docker-ssh",
        &fixture.run_refine(&["cluster", "disable-node", "docker-ssh"]),
    );
    let disabled_run = fixture.run_refine(&["cluster", "run", "docker-ssh", "printf disabled"]);
    assert!(
        !disabled_run.status.success(),
        "cluster run unexpectedly succeeded for disabled node"
    );
    assert!(
        String::from_utf8_lossy(&disabled_run.stderr).contains("disabled"),
        "stderr:\n{}",
        String::from_utf8_lossy(&disabled_run.stderr)
    );
    let disabled_gap = fixture.create_gap("disabled cluster transfer gap");
    let disabled_transfer =
        fixture.run_refine(&["cluster", "transfer", "docker-ssh", &disabled_gap]);
    assert!(
        !disabled_transfer.status.success(),
        "cluster transfer unexpectedly succeeded for disabled node"
    );
    assert!(
        String::from_utf8_lossy(&disabled_transfer.stderr).contains("disabled"),
        "stderr:\n{}",
        String::from_utf8_lossy(&disabled_transfer.stderr)
    );
    fixture.assert_success(
        "cluster enable-node docker-ssh",
        &fixture.run_refine(&["cluster", "enable-node", "docker-ssh"]),
    );

    let run = fixture.run_refine(&["cluster", "run", "docker-ssh", "printf cluster-ssh-ok"]);
    fixture.assert_success("cluster run docker-ssh", &run);
    let run_payload = fixture.json_stdout(&run);
    assert_eq!(run_payload["ok"], true, "{run_payload:#}");
    assert_eq!(run_payload["result"]["ok"], true, "{run_payload:#}");
    assert_eq!(run_payload["result"]["exit_code"], 0, "{run_payload:#}");
    assert_eq!(run_payload["result"]["stdout"], "cluster-ssh-ok");
    assert_eq!(run_payload["result"]["stderr"], "");
    assert!(
        run_payload["result"]["command"]
            .as_str()
            .unwrap_or_default()
            .contains("refine@127.0.0.1"),
        "{run_payload:#}"
    );
    assert!(
        fixture
            .runtime_root
            .join(fixture.port.to_string())
            .join("cluster-known_hosts")
            .is_file(),
        "cluster SSH should use runtime-scoped known_hosts"
    );

    let denied_settings = fixture.api_json(
        "PATCH",
        "/api/settings",
        json!({"allowed_commands": "printf"}),
    );
    assert_eq!(denied_settings["settings"]["allowed_commands"], "printf");
    let denied = fixture.run_refine(&["cluster", "run", "docker-ssh", "sh -c 'printf denied'"]);
    assert!(
        !denied.status.success(),
        "unauthorized cluster run unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&denied.stderr).contains("not authorized"),
        "stderr:\n{}",
        String::from_utf8_lossy(&denied.stderr)
    );
    let restored_settings = fixture.api_json(
        "PATCH",
        "/api/settings",
        json!({"allowed_commands": "printf\nsh\nmkdir"}),
    );
    assert_eq!(
        restored_settings["settings"]["allowed_commands"],
        "printf\nsh\nmkdir"
    );

    let empty_command = fixture.run_refine(&["cluster", "run", "docker-ssh", "   "]);
    assert!(
        !empty_command.status.success(),
        "empty cluster run unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&empty_command.stderr).contains("command is required"),
        "stderr:\n{}",
        String::from_utf8_lossy(&empty_command.stderr)
    );

    let failing = fixture.run_refine(&[
        "cluster",
        "run",
        "docker-ssh",
        "sh -c 'printf fail-out; printf fail-err >&2; exit 7'",
    ]);
    fixture.assert_success("cluster run docker-ssh nonzero remote", &failing);
    let failing_payload = fixture.json_stdout(&failing);
    assert_eq!(failing_payload["ok"], false, "{failing_payload:#}");
    assert_eq!(
        failing_payload["result"]["ok"], false,
        "{failing_payload:#}"
    );
    assert_eq!(
        failing_payload["result"]["exit_code"], 7,
        "{failing_payload:#}"
    );
    assert_eq!(failing_payload["result"]["stdout"], "fail-out");
    assert_eq!(failing_payload["result"]["stderr"], "fail-err");

    ssh.prepare_bootstrap_checkout();
    let bootstrap = fixture.run_refine(&["cluster", "bootstrap", "docker-ssh"]);
    fixture.assert_success("cluster bootstrap docker-ssh", &bootstrap);
    let bootstrap_payload = fixture.json_stdout(&bootstrap);
    assert_eq!(bootstrap_payload["ok"], true, "{bootstrap_payload:#}");
    assert_eq!(bootstrap_payload["dry_run"], false, "{bootstrap_payload:#}");
    assert_eq!(
        bootstrap_payload["result"]["exit_code"], 0,
        "{bootstrap_payload:#}"
    );
    assert!(
        bootstrap_payload["result"]["stdout"]
            .as_str()
            .unwrap_or_default()
            .contains("refine_port=18081"),
        "{bootstrap_payload:#}"
    );
    assert_eq!(
        node_in_cluster_payload(&bootstrap_payload, "docker-ssh")["health"]["status"],
        "ready",
        "{bootstrap_payload:#}"
    );

    let remove = fixture.run_refine(&["cluster", "remove-node", "docker-ssh"]);
    fixture.assert_success("cluster remove-node docker-ssh", &remove);
    let removed_show = fixture.run_refine(&["cluster", "show", "docker-ssh"]);
    assert!(
        !removed_show.status.success(),
        "removed node was still visible"
    );
}

struct DockerSshFixture {
    root: PathBuf,
    project: String,
    port: u16,
    identity: PathBuf,
}

impl DockerSshFixture {
    fn start() -> Self {
        let repo_root = repo_root();
        let root = repo_root
            .join("target/refine-integration/cluster-ssh")
            .join(unique_suffix());
        let key_dir = root.join("keys");
        fs::create_dir_all(&key_dir).expect("failed to create cluster ssh key dir");

        let identity = key_dir.join("id_ed25519");
        run_command(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(&identity),
            "generate SSH keypair for cluster fixture",
        );

        let port = free_loopback_port();
        let project = format!("refine-cluster-ssh-{}", unique_suffix());
        let compose_file = repo_root.join("tests/fixtures/cluster-ssh/compose.yml");
        let authorized_keys = identity.with_extension("pub");
        let fixture = Self {
            root,
            project,
            port,
            identity,
        };

        run_command(
            compose_command(&compose_file, &fixture.project)
                .args(["up", "-d", "--build", "--wait"])
                .env("REFINE_CLUSTER_SSH_PORT", fixture.port.to_string())
                .env(
                    "REFINE_CLUSTER_SSH_AUTHORIZED_KEYS",
                    authorized_keys.display().to_string(),
                ),
            "start cluster SSH docker compose fixture",
        );
        fixture
    }

    fn prepare_bootstrap_checkout(&self) {
        run_command(
            self.ssh_command().arg("sh").arg("-lc").arg(
                "set -eu; \
                     rm -rf /tmp/refine /tmp/app /tmp/refine-origin.git; \
                     mkdir -p /tmp/app; \
                     git init --bare /tmp/refine-origin.git >/dev/null; \
                     git clone /tmp/refine-origin.git /tmp/refine >/dev/null 2>&1; \
                     cd /tmp/refine; \
                     git config user.email refine@example.invalid; \
                     git config user.name Refine; \
                     printf bootstrap-fixture > README.md; \
                     git add README.md; \
                     git commit -m init >/dev/null; \
                     git push -u origin master >/dev/null",
            ),
            "prepare remote checkout for cluster bootstrap",
        );
    }

    fn ssh_command(&self) -> Command {
        let known_hosts = self.root.join("known_hosts");
        let mut command = Command::new("ssh");
        command.args([
            "-p",
            &self.port.to_string(),
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "LogLevel=ERROR",
            "-o",
            &format!("UserKnownHostsFile={}", known_hosts.display()),
            "-i",
            self.identity.to_str().unwrap(),
            "refine@127.0.0.1",
        ]);
        command
    }
}

impl Drop for DockerSshFixture {
    fn drop(&mut self) {
        let compose_file = repo_root().join("tests/fixtures/cluster-ssh/compose.yml");
        let _ = compose_command(&compose_file, &self.project)
            .args(["down", "--volumes", "--remove-orphans"])
            .output();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn compose_command(compose_file: &Path, project: &str) -> Command {
    let mut command = Command::new("docker");
    command
        .args(["compose", "-f"])
        .arg(compose_file)
        .args(["-p", project]);
    command
}

fn run_command(command: &mut Command, label: &str) {
    let output = output_command(command, label);
    assert!(
        output.status.success(),
        "{label} failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn output_command(command: &mut Command, label: &str) -> Output {
    command
        .output()
        .unwrap_or_else(|error| panic!("failed to {label}: {error}"))
}

fn repo_root() -> PathBuf {
    let mut current = std::env::current_dir().expect("failed to inspect cwd");
    loop {
        if current.join("docs/spec/rust-spec.md").is_file() {
            return current;
        }
        assert!(current.pop(), "failed to locate repository root");
    }
}

fn node_in_cluster_payload<'a>(
    payload: &'a serde_json::Value,
    node_id: &str,
) -> &'a serde_json::Value {
    payload["cluster"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == node_id)
        .unwrap()
}

fn free_loopback_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind ephemeral port");
    listener
        .local_addr()
        .expect("failed to inspect ephemeral port")
        .port()
}

fn unique_suffix() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{}-{millis}", std::process::id())
}

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
        json!({"allowed_commands": "printf\nsh"}),
    );
    assert_eq!(settings["settings"]["allowed_commands"], "printf\nsh");

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

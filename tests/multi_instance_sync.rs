use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

#[test]
#[ignore = "daemon-backed multi-instance test; run through `cargo run --manifest-path xtask/Cargo.toml -- test-multi-instance-sync`"]
fn two_daemons_sync_refine_state_through_shared_git_remote() {
    let root = temp_root("multi-instance-sync");
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    let app_a = root.join("app-a");
    let app_b = root.join("app-b");
    let runtime_a = root.join("run-a");
    let runtime_b = root.join("run-b");
    let artifacts = root.join("artifacts");

    seed_remote(&remote, &seed);
    git(
        &root,
        &["clone", remote.to_str().unwrap(), app_a.to_str().unwrap()],
    );
    git(
        &root,
        &["clone", remote.to_str().unwrap(), app_b.to_str().unwrap()],
    );
    configure_repo(&app_a);
    configure_repo(&app_b);

    let mut instance_a = RefineInstance::start("a", &runtime_a, &app_a, &artifacts);
    let mut instance_b = RefineInstance::start("b", &runtime_b, &app_b, &artifacts);

    let first_label = format!("multi-instance first {}", now_millis());
    let first_gap = instance_a.create_gap(&first_label);
    assert!(!first_gap.is_empty());
    commit_and_push_refine_state(&app_a, "instance A adds first gap");

    let sync_b = instance_b.api_json("POST", "/api/project/sync", json!({}));
    assert_eq!(sync_b["ok"], true, "{sync_b:#}");
    assert_eq!(sync_b["git_sync"]["attempted"], true, "{sync_b:#}");
    assert_eq!(sync_b["gap_count"], 1, "{sync_b:#}");
    instance_b.assert_gap_visible(&first_label);

    let second_label = format!("multi-instance second {}", now_millis());
    let second_gap = instance_b.create_gap(&second_label);
    assert!(!second_gap.is_empty());
    commit_and_push_refine_state(&app_b, "instance B adds second gap");

    let sync_a = instance_a.api_json("POST", "/api/project/sync", json!({}));
    assert_eq!(sync_a["ok"], true, "{sync_a:#}");
    assert_eq!(sync_a["git_sync"]["attempted"], true, "{sync_a:#}");
    assert_eq!(sync_a["gap_count"], 2, "{sync_a:#}");
    instance_a.assert_gap_visible(&first_label);
    instance_a.assert_gap_visible(&second_label);

    instance_b.stop();
    instance_a.stop();
    let _ = fs::remove_dir_all(root);
}

struct RefineInstance {
    port: u16,
    runtime_root: PathBuf,
    app_root: PathBuf,
    artifact_root: PathBuf,
    child: Option<Child>,
}

impl RefineInstance {
    fn start(name: &str, runtime_root: &Path, app_root: &Path, artifacts: &Path) -> Self {
        let repo_root = repo_root();
        let static_root = repo_root.join("src/surfaces/web/static");
        let artifact_root = artifacts.join(name);
        fs::create_dir_all(&artifact_root).unwrap();
        fs::create_dir_all(runtime_root).unwrap();
        let port = free_port();
        stop_daemon(port, runtime_root);

        let stdout = fs::File::create(artifact_root.join("daemon.stdout.log")).unwrap();
        let stderr = fs::File::create(artifact_root.join("daemon.stderr.log")).unwrap();
        let mut child = Command::new(refine_bin())
            .args([
                "system",
                "start",
                "--foreground",
                "--port",
                &port.to_string(),
                "--runtime-root",
                runtime_root.to_str().unwrap(),
                "--static-root",
                static_root.to_str().unwrap(),
            ])
            .current_dir(&repo_root)
            .envs(instance_env(port, runtime_root, app_root))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("failed to start refine daemon");

        wait_for_daemon(port, &mut child, &artifact_root);
        let instance = Self {
            port,
            runtime_root: runtime_root.to_path_buf(),
            app_root: app_root.to_path_buf(),
            artifact_root,
            child: Some(child),
        };
        let attach = instance.run_refine(&["project", "attach", app_root.to_str().unwrap()]);
        assert_success("project attach", &attach);
        instance
    }

    fn api_json(&self, method: &str, path: &str, body: Value) -> Value {
        let bytes = http_json(self.port, method, path, Some(body))
            .unwrap_or_else(|error| panic!("{method} {path} failed: {error}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{method} {path} returned invalid JSON: {error}"))
    }

    fn create_gap(&self, label: &str) -> String {
        let payload = self.api_json(
            "POST",
            "/api/gaps",
            json!({
                "reporter": "multi-instance",
                "actual": format!("{label} actual"),
                "target": format!("{label} target"),
                "priority": "high"
            }),
        );
        let id = payload["gap"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(!id.is_empty(), "{payload:#}");
        id
    }

    fn assert_gap_visible(&self, label: &str) {
        let path = format!("/api/gaps?q={}&node=all&limit=50", query_component(label));
        let payload = self.api_json("GET", &path, json!({}));
        let gaps = payload["gaps"].as_array().cloned().unwrap_or_default();
        assert!(
            gaps.iter()
                .any(|gap| gap["name"].as_str().unwrap_or_default().contains(label)),
            "{label} was not visible in instance on port {}\n{payload:#}",
            self.port
        );
    }

    fn run_refine(&self, args: &[&str]) -> Output {
        let output = Command::new(refine_bin())
            .args(args)
            .current_dir(repo_root())
            .envs(instance_env(self.port, &self.runtime_root, &self.app_root))
            .output()
            .unwrap_or_else(|error| panic!("failed to run refine {args:?}: {error}"));
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.artifact_root.join("cli-transcript.log"))
            .unwrap();
        let _ = writeln!(file, "$ refine {}", args.join(" "));
        let _ = writeln!(file, "status: {}", output.status);
        let _ = writeln!(file, "stdout:\n{}", String::from_utf8_lossy(&output.stdout));
        let _ = writeln!(file, "stderr:\n{}", String::from_utf8_lossy(&output.stderr));
        output
    }

    fn stop(&mut self) {
        stop_daemon(self.port, &self.runtime_root);
        if let Some(mut child) = self.child.take() {
            for _ in 0..30 {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for RefineInstance {
    fn drop(&mut self) {
        self.stop();
    }
}

fn seed_remote(remote: &Path, seed: &Path) {
    fs::create_dir_all(seed.parent().unwrap()).unwrap();
    git(
        seed.parent().unwrap(),
        &["init", "--bare", remote.to_str().unwrap()],
    );
    fs::create_dir_all(seed).unwrap();
    git(seed, &["init", "-q"]);
    configure_repo(seed);
    fs::write(seed.join("README.md"), "# Multi-instance sync test app\n").unwrap();
    fs::write(
        seed.join("app.py"),
        "def health() -> str:\n    return \"ok\"\n",
    )
    .unwrap();
    git(seed, &["add", "README.md", "app.py"]);
    git(
        seed,
        &["commit", "-q", "-m", "Initialize multi-instance sync app"],
    );
    git(seed, &["branch", "-M", "main"]);
    git(seed, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(seed, &["push", "-u", "origin", "main"]);
}

fn commit_and_push_refine_state(app_root: &Path, message: &str) {
    git(app_root, &["add", ".refine", ":(exclude).refine/runtime"]);
    git(app_root, &["commit", "-q", "-m", message]);
    git(app_root, &["push", "origin", "main"]);
}

fn configure_repo(root: &Path) {
    git(
        root,
        &["config", "user.email", "refine-sync@example.invalid"],
    );
    git(root, &["config", "user.name", "Refine Sync Test"]);
}

fn stop_daemon(port: u16, runtime_root: &Path) {
    let _ = Command::new(refine_bin())
        .args([
            "system",
            "stop",
            "--port",
            &port.to_string(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
        ])
        .current_dir(repo_root())
        .envs(instance_env(port, runtime_root, Path::new("")))
        .output();
}

fn wait_for_daemon(port: u16, child: &mut Child, artifact_root: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("failed to poll daemon") {
            panic!("daemon exited before readiness with status {status}");
        }
        if http_json(port, "GET", "/system/version", None).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "daemon did not become ready at http://127.0.0.1:{port}; see {}",
        artifact_root.display()
    );
}

fn instance_env(port: u16, runtime_root: &Path, app_root: &Path) -> Vec<(String, String)> {
    let mut env = vec![
        ("REFINE_TEST_PORT".to_string(), port.to_string()),
        ("REFINE_DAEMON_PORT".to_string(), port.to_string()),
        (
            "REFINE_TEST_RUNTIME_ROOT".to_string(),
            runtime_root.display().to_string(),
        ),
    ];
    if !app_root.as_os_str().is_empty() {
        env.push((
            "REFINE_TEST_APP_ROOT".to_string(),
            app_root.display().to_string(),
        ));
    }
    env
}

fn http_json(port: u16, method: &str, path: &str, body: Option<Value>) -> Result<Vec<u8>, String> {
    let body = body
        .map(|value| serde_json::to_vec(&value).map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or_default();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Refine-API-Version: 1\r\nIdempotency-Key: multi-instance-{}\r\n\r\n",
        body.len(),
        now_millis()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| error.to_string())?;
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "response missing headers".to_string())?;
    let status = String::from_utf8_lossy(&response[..split])
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    if !status.contains(" 200 ") && !status.contains(" 201 ") {
        return Err(format!(
            "{status}\n{}",
            String::from_utf8_lossy(&response[split + 4..])
        ));
    }
    Ok(response[split + 4..].to_vec())
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {}: {error}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed in {}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn temp_root(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "refine-{prefix}-{}-{}",
        std::process::id(),
        now_millis()
    ))
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn query_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push_str("%20"),
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

fn refine_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_refine"))
}

fn repo_root() -> PathBuf {
    let mut current = std::env::current_dir().expect("failed to inspect cwd");
    loop {
        if current.join("docs/spec/rust-integration-spec.md").is_file() {
            return current;
        }
        assert!(current.pop(), "failed to locate repository root");
    }
}

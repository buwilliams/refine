use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_TEST_PORT: u16 = 18080;

pub struct IntegrationFixture {
    pub port: u16,
    pub repo_root: PathBuf,
    pub runtime_root: PathBuf,
    pub app_root: PathBuf,
    pub artifact_root: PathBuf,
    binary: PathBuf,
    daemon: Option<Child>,
    agent_automation_enabled: bool,
}

impl IntegrationFixture {
    #[allow(dead_code)]
    pub fn start(suite: &str) -> Self {
        Self::start_with_automation(suite, false)
    }

    #[allow(dead_code)]
    pub fn start_with_agent_automation(suite: &str) -> Self {
        Self::start_with_automation(suite, true)
    }

    fn start_with_automation(suite: &str, agent_automation_enabled: bool) -> Self {
        let repo_root = repo_root();
        let port = test_port();
        let runtime_root = env_path("REFINE_TEST_RUNTIME_ROOT")
            .unwrap_or_else(|| repo_root.join("target/refine-integration/run"));
        let app_root = env_path("REFINE_TEST_APP_ROOT")
            .unwrap_or_else(|| default_app_root(&repo_root, suite, port));
        let artifact_root = repo_root
            .join("target/refine-integration/artifacts")
            .join(format!("{suite}-{port}"));
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_refine"));
        let static_root = repo_root.join("src/surfaces/web/static");

        let mut fixture = Self {
            port,
            repo_root,
            runtime_root,
            app_root,
            artifact_root,
            binary,
            daemon: None,
            agent_automation_enabled,
        };
        fixture.reset_paths();
        fixture.ensure_test_app();
        fixture.stop_stale_daemon();
        fixture.start_daemon(&static_root);
        fixture.attach_app();
        fixture.wait_for_attached_project();
        fixture
    }

    pub fn run_refine(&self, args: &[&str]) -> Output {
        let output = Command::new(&self.binary)
            .args(args)
            .current_dir(&self.repo_root)
            .envs(self.env())
            .output()
            .unwrap_or_else(|error| panic!("failed to run refine {args:?}: {error}"));
        self.record_command(args, &output);
        output
    }

    pub fn json_stdout(&self, output: &Output) -> serde_json::Value {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for marker in ['{', '['] {
            if let Some(index) = stdout.find(marker)
                && let Ok(value) = serde_json::from_str(&stdout[index..])
            {
                return value;
            }
        }
        panic!(
            "stdout was not JSON\nstdout:\n{}\nstderr:\n{}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub fn assert_success(&self, label: &str, output: &Output) {
        assert!(
            output.status.success(),
            "{label} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub fn create_goal(&self, name: &str) -> String {
        let output = self.run_refine(&["goal", "create", name]);
        self.assert_success("goal create", &output);
        let value = self.json_stdout(&output);
        value["goal"]["id"]
            .as_str()
            .expect("goal create should return goal.id")
            .to_string()
    }

    #[allow(dead_code)]
    pub fn goal_field(&self, id: &str, field: &str) -> serde_json::Value {
        let output = self.run_refine(&["goal", "show", id]);
        self.assert_success("goal show", &output);
        self.json_stdout(&output)["goal"][field].clone()
    }

    pub fn api_json(&self, method: &str, path: &str, body: serde_json::Value) -> serde_json::Value {
        let response = http_json(self.port, method, path, Some(body))
            .unwrap_or_else(|error| panic!("{method} {path} failed: {error}"));
        serde_json::from_slice(&response)
            .unwrap_or_else(|error| panic!("{method} {path} returned invalid JSON: {error}"))
    }

    #[allow(dead_code)]
    pub fn create_git_app(&self, name: &str) -> PathBuf {
        let app_root = self.app_workspace_root().join(name);
        ensure_git_app_at(&app_root, name);
        app_root
    }

    pub fn app_workspace_root(&self) -> PathBuf {
        self.app_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.repo_root.join("target/refine-integration/apps"))
    }

    fn reset_paths(&self) {
        let _ = fs::remove_dir_all(&self.artifact_root);
        fs::create_dir_all(&self.artifact_root).expect("failed to create artifact root");
        self.copy_runtime_diagnostics("before-reset");
        let _ = fs::remove_dir_all(&self.runtime_root);
        self.cleanup_test_app_worktrees();
        let _ = fs::remove_dir_all(&self.app_root);
    }

    fn ensure_test_app(&self) {
        ensure_git_app_at(&self.app_root, "rust-test-app");
    }

    fn start_daemon(&mut self, static_root: &Path) {
        fs::create_dir_all(&self.runtime_root).expect("failed to create runtime root");
        let stdout = File::create(self.artifact_root.join("daemon.stdout.log")).unwrap();
        let stderr = File::create(self.artifact_root.join("daemon.stderr.log")).unwrap();
        let child = Command::new(&self.binary)
            .args([
                "system",
                "start",
                "--foreground",
                "--port",
                &self.port.to_string(),
                "--runtime-root",
                &self.runtime_root.display().to_string(),
                "--static-root",
                &static_root.display().to_string(),
            ])
            .current_dir(&self.repo_root)
            .envs(self.env())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("failed to start foreground daemon");
        self.daemon = Some(child);
        self.wait_for_daemon();
    }

    fn attach_app(&self) {
        let app = self.app_root.display().to_string();
        let output = self.run_refine(&["project", "attach", &app]);
        self.assert_success("project attach", &output);
    }

    fn wait_for_daemon(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            if let Some(child) = &mut self.daemon
                && let Some(status) = child.try_wait().expect("failed to poll daemon")
            {
                panic!("daemon exited before readiness with status {status}");
            }
            if http_get(self.port, "/system/version").is_ok() && self.daemon_status_ready() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "daemon did not become ready at http://127.0.0.1:{}; see {}",
            self.port,
            self.artifact_root.display()
        );
    }

    fn daemon_status_ready(&self) -> bool {
        let path = self
            .runtime_root
            .join(self.port.to_string())
            .join("daemon-status.json");
        let Ok(bytes) = fs::read(path) else {
            return false;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return false;
        };
        value["port"].as_u64() == Some(self.port.into())
            && value["daemon_healthy"].as_bool() == Some(true)
    }

    fn wait_for_attached_project(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(body) = http_get(self.port, "/api/project/status")
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&body)
                && value["attached"].as_bool() == Some(true)
            {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("project did not report attached at port {}", self.port);
    }

    fn stop_stale_daemon(&self) {
        let _ = Command::new(&self.binary)
            .args([
                "system",
                "stop",
                "--port",
                &self.port.to_string(),
                "--runtime-root",
                &self.runtime_root.display().to_string(),
            ])
            .current_dir(&self.repo_root)
            .envs(self.env())
            .output();
    }

    fn stop_daemon(&mut self) {
        let _ = self.run_refine(&[
            "system",
            "stop",
            "--port",
            &self.port.to_string(),
            "--runtime-root",
            &self.runtime_root.display().to_string(),
        ]);
        if let Some(mut child) = self.daemon.take() {
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

    fn cleanup_test_app_worktrees(&self) {
        let Some(parent) = self.app_root.parent() else {
            return;
        };
        let Some(name) = self.app_root.file_name().and_then(|value| value.to_str()) else {
            return;
        };
        let prefix = format!("{name}-");
        let Ok(entries) = fs::read_dir(parent) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == self.app_root {
                continue;
            }
            let matches_prefix = path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with(&prefix));
            if matches_prefix && path.is_dir() {
                let _ = fs::remove_dir_all(path);
            }
        }
    }

    fn env(&self) -> Vec<(String, String)> {
        let mut env = vec![
            ("REFINE_TEST_PORT".to_string(), self.port.to_string()),
            ("REFINE_DAEMON_PORT".to_string(), self.port.to_string()),
            (
                "REFINE_TEST_RUNTIME_ROOT".to_string(),
                self.runtime_root.display().to_string(),
            ),
            (
                "REFINE_TEST_APP_ROOT".to_string(),
                self.app_root.display().to_string(),
            ),
        ];
        if !self.agent_automation_enabled {
            env.push((
                "REFINE_AGENT_WORKFLOW_DISABLED".to_string(),
                "1".to_string(),
            ));
        }
        if let Ok(path) = std::env::var("REFINE_SMOKE_AI_PATH") {
            env.push(("REFINE_SMOKE_AI_PATH".to_string(), path));
        }
        env.push(("SMOKE_AI_EDIT_APP".to_string(), "1".to_string()));
        env
    }

    fn record_command(&self, args: &[&str], output: &Output) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.artifact_root.join("cli-transcript.log"))
            .unwrap();
        let _ = writeln!(file, "$ refine {}", args.join(" "));
        let _ = writeln!(file, "status: {}", output.status);
        let _ = writeln!(file, "stdout:\n{}", String::from_utf8_lossy(&output.stdout));
        let _ = writeln!(file, "stderr:\n{}", String::from_utf8_lossy(&output.stderr));
        let _ = writeln!(file);
    }

    fn copy_runtime_diagnostics(&self, label: &str) {
        let source = self.runtime_root.join(self.port.to_string());
        if source.is_dir() {
            let dest = self
                .artifact_root
                .join(format!("runtime-{label}-{}", timestamp_millis()));
            let _ = copy_dir_all(&source, &dest);
        }
    }
}

impl Drop for IntegrationFixture {
    fn drop(&mut self) {
        self.copy_runtime_diagnostics("teardown");
        self.stop_daemon();
        self.copy_runtime_diagnostics("after-stop");
        let _ = fs::remove_dir_all(&self.runtime_root);
        self.cleanup_test_app_worktrees();
        let _ = fs::remove_dir_all(&self.app_root);
    }
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

fn default_app_root(repo_root: &Path, suite: &str, port: u16) -> PathBuf {
    repo_root
        .join("target/refine-integration/apps")
        .join(format!("{suite}-{port}"))
        .join("rust-test-app")
}

fn ensure_git_app_at(app_root: &Path, name: &str) {
    fs::create_dir_all(app_root).expect("failed to create test app");
    fs::write(
        app_root.join("README.md"),
        format!("# Refine rust smoke target app\n\nDisposable target app `{name}` for the Rust integration suite.\n"),
    )
    .unwrap();
    fs::write(
        app_root.join("app.py"),
        "def health() -> str:\n    return \"ok\"\n",
    )
    .unwrap();
    fs::write(app_root.join(".gitignore"), "__pycache__/\n*.py[cod]\n").unwrap();
    if !app_root.join(".git").is_dir() {
        git(app_root, &["init", "-q"]);
    }
    git(
        app_root,
        &["config", "user.email", "refine-smoke@example.invalid"],
    );
    git(app_root, &["config", "user.name", "Refine Rust Smoke"]);
    git(app_root, &["add", "README.md", "app.py", ".gitignore"]);
    let diff = git_output(app_root, &["diff", "--cached", "--quiet", "--exit-code"]);
    if diff.status.code() == Some(1) {
        git(
            app_root,
            &[
                "commit",
                "-q",
                "-m",
                "Initialize refine rust smoke target app",
            ],
        );
    } else if !diff.status.success() {
        panic!(
            "git diff --cached failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&diff.stdout),
            String::from_utf8_lossy(&diff.stderr)
        );
    }
}

fn git(app_root: &Path, args: &[&str]) {
    let output = git_output(app_root, args);
    assert!(
        output.status.success(),
        "git {} failed\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output(app_root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(app_root)
        .output()
        .expect("failed to run git")
}

fn test_port() -> u16 {
    std::env::var("REFINE_TEST_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_TEST_PORT)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn http_get(port: u16, path: &str) -> Result<Vec<u8>, String> {
    http_json(port, "GET", path, None)
}

fn http_json(
    port: u16,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<Vec<u8>, String> {
    let body = body
        .map(|value| serde_json::to_vec(&value).map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or_default();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nX-Refine-API-Version: 1\r\nIdempotency-Key: test-{}\r\n\r\n",
        body.len(),
        timestamp_millis()
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
    if !status.contains(" 200 ") {
        return Err(status);
    }
    Ok(response[split + 4..].to_vec())
}

fn copy_dir_all(source: &Path, dest: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest_path)?;
        } else if ty.is_file() {
            let _ = fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

fn timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

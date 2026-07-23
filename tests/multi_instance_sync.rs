use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use refine::model::workflow::GoalStatus;
use refine::process::subprocess::{FileProcessSupervisor, ProcessSupervisor};
use refine::process::supervisor::operations::FileOperationRegistry;
use refine::tools::host::project_layout::prepare_refine_dir;
use refine::tools::product::merging::FileMergerService;
use refine::tools::product::work_items::FileWorkItemService;
use refine::workflow::{WorkflowAutomation, WorkflowEngine};

static IDEMPOTENCY_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    let first_goal = instance_a.create_goal(&first_label);
    assert!(!first_goal.is_empty());
    instance_b.wait_for_goal(&first_label);

    let second_label = format!("multi-instance second {}", now_millis());
    let second_goal = instance_b.create_goal(&second_label);
    assert!(!second_goal.is_empty());
    instance_a.wait_for_goal(&first_label);
    instance_a.wait_for_goal(&second_label);

    instance_b.stop();
    instance_a.stop();
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "production Ready Merge child process used by the multi-instance gate"]
fn ready_merge_child_process() {
    if std::env::var("REFINE_READY_MERGE_CHILD").ok().as_deref() != Some("1") {
        return;
    }
    let runtime_root = PathBuf::from(std::env::var("REFINE_CHILD_RUNTIME").unwrap());
    let refine_dir = PathBuf::from(std::env::var("REFINE_CHILD_STATE").unwrap());
    let repo = PathBuf::from(std::env::var("REFINE_CHILD_REPO").unwrap());
    let goal_id = std::env::var("REFINE_CHILD_GOAL").unwrap();
    let claim_id = std::env::var("REFINE_CHILD_CLAIM").unwrap();
    let execution_id = std::env::var("REFINE_CHILD_EXECUTION").unwrap();
    let branch = std::env::var("REFINE_CHILD_BRANCH").unwrap();
    let candidate = std::env::var("REFINE_CHILD_CANDIDATE").unwrap();
    let output_path = PathBuf::from(std::env::var("REFINE_CHILD_OUTPUT").unwrap());
    let outcome = FileMergerService::with_target_root(&runtime_root, &refine_dir, &repo)
        .integrate_workflow_candidate(
            &goal_id,
            0,
            &claim_id,
            &execution_id,
            "default",
            &branch,
            &candidate,
            "origin",
        );
    let value = match outcome {
        Ok(integration) => json!({"ok": true, "integration": integration}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    };
    fs::write(output_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
#[ignore = "multi-process Ready Merge cancellation/replacement gate; run through xtask"]
fn ready_merge_multi_process_cancellation_replacement_retry_is_exactly_once() {
    let fixture = ready_merge_fixture("ready-merge-process-cancel");
    install_slow_main_hook(&fixture.remote);
    let first_output = fixture.root.join("first.json");
    let mut first = spawn_ready_merge_child(&fixture, &fixture.execution_id, &first_output);
    wait_for_workflow_git_process(&fixture.runtime_root, &fixture.execution_id, "push");

    fixture.automation().cancel(&fixture.execution_id).unwrap();
    assert!(first.wait().unwrap().success());
    let cancelled = read_json(&first_output);
    assert_eq!(cancelled["ok"], false, "{cancelled:#}");
    let operations = fs::read_dir(fixture.runtime_root.join("operations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .map(|entry| read_json(&entry.path()))
        .collect::<Vec<_>>();
    assert!(
        operations.iter().any(|operation| {
            operation["owner"] == "merger:GOAL1:1" && operation["state"] == "Cancelled"
        }),
        "{operations:#?}"
    );
    assert!(!git_succeeds(
        &fixture.repo,
        &[
            "merge-base",
            "--is-ancestor",
            &fixture.candidate,
            "origin/main",
        ],
    ));

    fs::remove_file(fixture.remote.join("hooks/pre-receive")).unwrap();
    let replacement_execution = fixture.automation().retry(&fixture.execution_id).unwrap();
    let stale_output = fixture.root.join("stale.json");
    assert!(
        spawn_ready_merge_child(&fixture, &fixture.execution_id, &stale_output)
            .wait()
            .unwrap()
            .success()
    );
    let stale = read_json(&stale_output);
    assert_eq!(stale["ok"], false, "{stale:#}");
    assert!(
        stale["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no longer owns"),
        "{stale:#}"
    );

    let retry_output = fixture.root.join("retry.json");
    assert!(
        spawn_ready_merge_child(&fixture, &replacement_execution, &retry_output)
            .wait()
            .unwrap()
            .success()
    );
    assert_eq!(read_json(&retry_output)["ok"], true);
    let repeat_output = fixture.root.join("repeat.json");
    assert!(
        spawn_ready_merge_child(&fixture, &replacement_execution, &repeat_output)
            .wait()
            .unwrap()
            .success()
    );
    assert_eq!(read_json(&repeat_output)["ok"], true);
    assert!(git_succeeds(
        &fixture.repo,
        &[
            "merge-base",
            "--is-ancestor",
            &fixture.candidate,
            "origin/main",
        ],
    ));
    let audit = fs::read_to_string(fixture.repo.join(".git/refine-audit.jsonl")).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| line.contains("\"action\":\"merge_commit_no_ff\""))
            .count(),
        1
    );
    let _ = fs::remove_dir_all(&fixture.root);
}

#[test]
#[ignore = "multi-process Ready Merge restart recovery gate; run through xtask"]
fn ready_merge_multi_process_restart_recovery_preserves_exactly_once() {
    let fixture = ready_merge_fixture("ready-merge-process-restart");
    install_slow_main_hook(&fixture.remote);
    let interrupted_output = fixture.root.join("interrupted.json");
    let mut interrupted =
        spawn_ready_merge_child(&fixture, &fixture.execution_id, &interrupted_output);
    wait_for_workflow_git_process(&fixture.runtime_root, &fixture.execution_id, "push");
    interrupted.kill().unwrap();
    let _ = interrupted.wait();

    let recovered = FileOperationRegistry::new(&fixture.runtime_root)
        .recover_active_supervised()
        .unwrap();
    assert!(
        recovered.iter().any(|operation| {
            operation.owner == "merger:GOAL1:1"
                && format!("{:?}", operation.state).to_lowercase() == "interrupted"
        }),
        "{recovered:#?}"
    );
    FileProcessSupervisor::new(&fixture.runtime_root)
        .recover()
        .unwrap();
    fs::remove_file(fixture.remote.join("hooks/pre-receive")).unwrap();
    let replacement_execution = fixture.automation().retry(&fixture.execution_id).unwrap();
    let retry_output = fixture.root.join("restart-retry.json");
    assert!(
        spawn_ready_merge_child(&fixture, &replacement_execution, &retry_output)
            .wait()
            .unwrap()
            .success()
    );
    assert_eq!(read_json(&retry_output)["ok"], true);
    assert!(git_succeeds(
        &fixture.repo,
        &[
            "merge-base",
            "--is-ancestor",
            &fixture.candidate,
            "origin/main",
        ],
    ));
    let audit = fs::read_to_string(fixture.repo.join(".git/refine-audit.jsonl")).unwrap();
    assert_eq!(
        audit
            .lines()
            .filter(|line| line.contains("\"action\":\"merge_commit_no_ff\""))
            .count(),
        1
    );
    let _ = fs::remove_dir_all(&fixture.root);
}

struct ReadyMergeFixture {
    root: PathBuf,
    remote: PathBuf,
    repo: PathBuf,
    refine_dir: PathBuf,
    runtime_root: PathBuf,
    branch: String,
    candidate: String,
    claim_id: String,
    execution_id: String,
}

impl ReadyMergeFixture {
    fn automation(&self) -> WorkflowEngine {
        WorkflowEngine::with_target_root(&self.runtime_root, &self.repo)
    }
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
        let settings = instance.api_json(
            "PATCH",
            "/api/settings",
            json!({
                "state_sync_debounce_seconds": "1",
                "project_update_pulse_interval_seconds": "1"
            }),
        );
        assert_eq!(
            settings["settings"]["state_sync_debounce_seconds"], "1",
            "{settings:#}"
        );
        instance
    }

    fn api_json(&self, method: &str, path: &str, body: Value) -> Value {
        let bytes = http_json(self.port, method, path, Some(body))
            .unwrap_or_else(|error| panic!("{method} {path} failed: {error}"));
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{method} {path} returned invalid JSON: {error}"))
    }

    fn create_goal(&self, label: &str) -> String {
        let payload = self.api_json(
            "POST",
            "/api/goals",
            json!({
                "reporter": "multi-instance",
                "prompt": format!("{label} prompt"),
                "priority": "high"
            }),
        );
        let id = payload["goal"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(!id.is_empty(), "{payload:#}");
        id
    }

    fn wait_for_goal(&self, label: &str) {
        let path = format!("/api/goals?q={}&node=all&limit=50", query_component(label));
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last_payload = Value::Null;
        while Instant::now() < deadline {
            last_payload = self.api_json("GET", &path, json!({}));
            let visible = last_payload["goals"].as_array().is_some_and(|goals| {
                goals
                    .iter()
                    .any(|goal| goal["name"].as_str().unwrap_or_default().contains(label))
            });
            if visible {
                return;
            }
            thread::sleep(Duration::from_millis(200));
        }
        panic!(
            "{label} was not reconciled automatically on port {}\n{last_payload:#}",
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
        unique_idempotency_key()
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

fn ready_merge_fixture(prefix: &str) -> ReadyMergeFixture {
    let root = temp_root(prefix);
    let remote = root.join("remote.git");
    let repo = root.join("repo");
    let worktree = root.join("candidate");
    let runtime_root = root.join("run/8080");
    fs::create_dir_all(&root).unwrap();
    git(
        &root,
        &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
    );
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    configure_repo(&repo);
    fs::write(repo.join("app.txt"), "base\n").unwrap();
    git(&repo, &["add", "app.txt"]);
    git(&repo, &["commit", "-m", "base"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "-u", "origin", "main"]);
    let base_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let refine_dir = prepare_refine_dir(&repo).unwrap();
    let branch = "refine/GOAL1/round-1".to_string();
    git(
        &repo,
        &["worktree", "add", "-b", &branch, worktree.to_str().unwrap()],
    );
    configure_repo(&worktree);
    fs::write(worktree.join("feature.txt"), "candidate\n").unwrap();
    git(&worktree, &["add", "feature.txt"]);
    git(&worktree, &["commit", "-m", "candidate"]);
    git(&worktree, &["push", "-u", "origin", &branch]);
    let candidate = git_stdout(&worktree, &["rev-parse", "HEAD"]);

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("GOAL1", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Buddy", "Implement")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::InProgress)
        .unwrap();
    work_items
        .update_goal_git_refs("GOAL1", &branch, "main", &base_commit, Some(&candidate))
        .unwrap();
    work_items
        .update_goal_round_evaluation_summary("GOAL1", 0, &json!({"workflow_git_remote": "origin"}))
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL1", GoalStatus::ReadyMerge)
        .unwrap();
    let automation = WorkflowEngine::with_target_root(&runtime_root, &repo);
    let claim_id = automation.claim("GOAL1").unwrap();
    let execution_id = automation.start_claim(&claim_id).unwrap();

    ReadyMergeFixture {
        root,
        remote,
        repo,
        refine_dir,
        runtime_root,
        branch,
        candidate,
        claim_id,
        execution_id,
    }
}

fn spawn_ready_merge_child(
    fixture: &ReadyMergeFixture,
    execution_id: &str,
    output_path: &Path,
) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ready_merge_child_process",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("REFINE_READY_MERGE_CHILD", "1")
        .env("REFINE_CHILD_RUNTIME", &fixture.runtime_root)
        .env("REFINE_CHILD_STATE", &fixture.refine_dir)
        .env("REFINE_CHILD_REPO", &fixture.repo)
        .env("REFINE_CHILD_GOAL", "GOAL1")
        .env("REFINE_CHILD_CLAIM", &fixture.claim_id)
        .env("REFINE_CHILD_EXECUTION", execution_id)
        .env("REFINE_CHILD_BRANCH", &fixture.branch)
        .env("REFINE_CHILD_CANDIDATE", &fixture.candidate)
        .env("REFINE_CHILD_OUTPUT", output_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn install_slow_main_hook(remote: &Path) {
    let hook = remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do\n  test \"$ref\" != refs/heads/main || sleep 10\ndone\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).unwrap();
    }
}

fn wait_for_workflow_git_process(runtime_root: &Path, execution_id: &str, command: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let processes = runtime_root.join("processes");
        if let Ok(entries) = fs::read_dir(processes)
            && entries.filter_map(Result::ok).any(|entry| {
                entry.path().extension().and_then(|ext| ext.to_str()) == Some("json")
                    && fs::read(entry.path()).ok().is_some_and(|bytes| {
                        serde_json::from_slice::<Value>(&bytes)
                            .ok()
                            .and_then(|process| {
                                process
                                    .get("details")
                                    .and_then(Value::as_str)
                                    .and_then(|details| serde_json::from_str::<Value>(details).ok())
                            })
                            .is_some_and(|details| {
                                details["execution_id"] == execution_id
                                    && details["git_command"] == command
                            })
                    })
            })
        {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("managed Git {command} process for {execution_id} was not observed");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn git_stdout(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert_success(&format!("git {}", args.join(" ")), &output);
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_succeeds(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap()
        .status
        .success()
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

fn unique_idempotency_key() -> String {
    let sequence = IDEMPOTENCY_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{sequence}", now_millis())
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

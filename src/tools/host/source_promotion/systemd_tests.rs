use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::*;

#[test]
#[ignore = "requires an available systemd user manager; platform-gated rollback integration evidence"]
fn real_systemd_failed_identity_replaces_candidate_with_prior_executable() {
    if !systemd_user_manager_available() {
        eprintln!("SKIP: systemd user manager is unavailable");
        return;
    }

    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let build = Command::new("cargo")
        .args(["build", "--bin", "refine"])
        .current_dir(&repository_root)
        .status()
        .unwrap();
    assert!(
        build.success(),
        "failed to build the production Refine binary"
    );
    let refine = repository_root.join("target/debug/refine");
    let root = test_directory("real-systemd-source-rollback");
    let runtime_root = root.join("run");
    let source = root.join("source");
    let origin = root.join("origin.git");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let unit_name = format!("refine-{port}.service");
    let config_root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .expect("HOME or XDG_CONFIG_HOME is required for systemd integration");
    let unit_path = config_root.join("systemd/user").join(&unit_name);
    let marker = std::env::temp_dir().join(format!("refine-candidate-{port}.pid"));
    let _cleanup = RealSystemdRollbackCleanup {
        unit_name: unit_name.clone(),
        unit_path: unit_path.clone(),
        marker: marker.clone(),
        root: root.clone(),
    };

    initialize_failing_candidate_repository(&source, &origin);
    let command_env = [
        ("XDG_STATE_HOME", root.join("state").display().to_string()),
        ("XDG_CACHE_HOME", root.join("cache").display().to_string()),
        ("REFINE_LAUNCH_MODE", "binary".to_string()),
        ("REFINE_LAUNCH_EXECUTABLE", refine.display().to_string()),
        ("REFINE_DAEMON_PORT", port.to_string()),
    ];
    let install = run_refine_command(
        &refine,
        &[
            "system",
            "service-install",
            "--port",
            &port.to_string(),
            "--runtime-root",
            &runtime_root.display().to_string(),
        ],
        &command_env,
    );
    assert_command_succeeded("system service-install", &install);
    wait_for_reachable(port, Duration::from_secs(20));

    let pause = run_refine_command(&refine, &["workflow", "pause"], &command_env);
    assert_command_succeeded("workflow pause", &pause);
    let queue = run_refine_command(
        &refine,
        &[
            "system",
            "source-promote",
            "--checkout",
            &source.display().to_string(),
            "--port",
            &port.to_string(),
            "--runtime-root",
            &runtime_root.display().to_string(),
        ],
        &command_env,
    );
    assert_command_succeeded("system source-promote", &queue);

    let operation_path = runtime_root
        .join(port.to_string())
        .join(SOURCE_PROMOTION_STATE_FILE);
    let operation = wait_for_operation(&operation_path, Duration::from_secs(180));
    assert_eq!(
        operation.status, "failed",
        "identity mismatch must retain the original promotion failure: {operation:?}"
    );
    assert_eq!(operation.stage, "restart_daemon");
    assert!(
        operation.error.as_deref().is_some_and(|error| {
            error.contains("deliberately-wrong-candidate-identity")
                && error.contains("canonicalize")
        }),
        "original identity failure was not retained: {operation:?}"
    );
    assert_eq!(operation.rollback_succeeded, Some(true), "{operation:?}");
    assert_eq!(
        operation.registration_rollback_succeeded,
        Some(true),
        "{operation:?}"
    );

    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.source_restored, Some(true));
    assert_eq!(evidence.registration_restored, Some(true));
    assert_eq!(evidence.registration_verified, Some(true));
    assert!(evidence.replacement_attempted);
    assert_eq!(evidence.replacement_succeeded, Some(true));
    assert_eq!(evidence.reachability, "reachable");
    assert_eq!(evidence.identity_matches, Some(true));
    assert!(evidence.errors.is_empty(), "{evidence:?}");

    let prior = fs::canonicalize(&refine).unwrap();
    assert_eq!(
        fs::canonicalize(evidence.registered_executable.as_ref().unwrap()).unwrap(),
        prior
    );
    assert_eq!(
        fs::canonicalize(evidence.observed_executable.as_ref().unwrap()).unwrap(),
        prior
    );
    let unit = fs::read_to_string(&unit_path).unwrap();
    assert!(
        unit.contains(&refine.display().to_string()),
        "restored unit did not target prior executable: {unit}"
    );
    let live = live_daemon_executable(port).unwrap();
    assert_eq!(fs::canonicalize(live).unwrap(), prior);
    assert_eq!(
        crate::process::supervisor::lifecycle::http_reachability_probe(port),
        crate::process::supervisor::lifecycle::DaemonReachability::Reachable
    );

    let candidate_pid = fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    assert!(
        !Path::new(&format!("/proc/{candidate_pid}")).exists(),
        "candidate process {candidate_pid} remained live after verified rollback"
    );
}

fn systemd_user_manager_available() -> bool {
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_for_operation(path: &Path, timeout: Duration) -> SourcePromotionOperation {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(operation) = serde_json::from_slice::<SourcePromotionOperation>(&bytes)
            && matches!(operation.status.as_str(), "succeeded" | "failed")
        {
            return operation;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for source promotion {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct RealSystemdRollbackCleanup {
    unit_name: String,
    unit_path: PathBuf,
    marker: PathBuf,
    root: PathBuf,
}

impl Drop for RealSystemdRollbackCleanup {
    fn drop(&mut self) {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", &self.unit_name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_file(&self.unit_path);
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = fs::remove_file(&self.marker);
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn initialize_failing_candidate_repository(source: &Path, origin: &Path) {
    fs::create_dir_all(source.join("src")).unwrap();
    git_ok(source, &["init", "--quiet", "."]).unwrap();
    git_ok(source, &["config", "user.email", "refine-test@example.com"]).unwrap();
    git_ok(source, &["config", "user.name", "Refine Test"]).unwrap();
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"refine\"\nversion = \"0.0.1\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(source.join("src/main.rs"), candidate_daemon_source(false)).unwrap();
    Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(source)
        .status()
        .unwrap();
    git_ok(source, &["add", "."]).unwrap();
    git_ok(source, &["commit", "--quiet", "-m", "prior"]).unwrap();
    let prior = git_text(source, &["rev-parse", "HEAD"]).unwrap();
    git_ok(
        origin.parent().unwrap(),
        &["init", "--quiet", "--bare", origin.to_str().unwrap()],
    )
    .unwrap();
    git_ok(
        source,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    )
    .unwrap();
    git_ok(source, &["push", "--quiet", "-u", "origin", "HEAD:main"]).unwrap();
    fs::write(source.join("src/main.rs"), candidate_daemon_source(true)).unwrap();
    git_ok(source, &["add", "src/main.rs"]).unwrap();
    git_ok(
        source,
        &["commit", "--quiet", "-m", "identity-failing candidate"],
    )
    .unwrap();
    git_ok(source, &["push", "--quiet", "origin", "HEAD:main"]).unwrap();
    git_ok(source, &["reset", "--quiet", "--hard", &prior]).unwrap();
}

fn candidate_daemon_source(fail_identity: bool) -> String {
    let reported_executable = if fail_identity {
        "\"/deliberately-wrong-candidate-identity\".to_string()"
    } else {
        "std::env::current_exe().unwrap().display().to_string()"
    };
    let marker = if fail_identity {
        r#"std::fs::write(
            std::env::temp_dir().join(format!("refine-candidate-{port}.pid")),
            std::process::id().to_string(),
        ).unwrap();"#
    } else {
        ""
    };
    format!(
        r#"use std::io::{{Read, Write}};
use std::net::{{TcpListener, TcpStream}};
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {{
    let args: Vec<String> = std::env::args().collect();
    let port = args.windows(2).find(|pair| pair[0] == "--port")
        .and_then(|pair| pair[1].parse::<u16>().ok()).expect("port");
    if args.iter().any(|arg| arg == "--foreground") {{
        {marker}
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        for stream in listener.incoming() {{
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            let executable = {reported_executable}
                .replace('\\', "\\\\").replace('"', "\\\"");
            let body = format!("{{{{\"product\":\"refine\",\"version\":\"candidate\",\"executable_path\":\"{{}}\"}}}}", executable);
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {{}}\r\nConnection: close\r\n\r\n{{}}", body.len(), body);
            stream.write_all(response.as_bytes()).unwrap();
        }}
        return;
    }}
    let unit = format!("refine-{{port}}.service");
    let status = Command::new("systemctl").args(["--user", "start", &unit]).status().unwrap();
    if !status.success() {{ std::process::exit(1); }}
    for _ in 0..200 {{
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {{ return; }}
        thread::sleep(Duration::from_millis(25));
    }}
    std::process::exit(2);
}}
"#
    )
}

fn test_directory(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("refine-{label}-{}", Uuid::new_v4()))
}

fn run_refine_command(refine: &Path, args: &[&str], environment: &[(&str, String)]) -> Output {
    let mut command = Command::new(refine);
    command.args(args);
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().unwrap()
}

fn assert_command_succeeded(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_for_reachable(port: u16, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while crate::process::supervisor::lifecycle::http_reachability_probe(port)
        != crate::process::supervisor::lifecycle::DaemonReachability::Reachable
    {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for daemon on {port}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

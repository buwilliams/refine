use super::*;

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an available systemd user manager; platform-gated source-promotion integration evidence"]
fn real_systemd_installed_provider_upgrade_runs_registered_candidate_identity() {
    use std::net::{Ipv4Addr, TcpListener};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    if !Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
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
    let root = test_directory("real-systemd-source-promotion");
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
    let _cleanup = RealSystemdPromotionCleanup {
        unit_name: unit_name.clone(),
        unit_path: unit_path.clone(),
        root: root.clone(),
    };

    initialize_candidate_source_repository(&source, &origin);
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
    let service =
        FileSourcePromotionService::new(&source, runtime_root.join(port.to_string()), port);
    let check = service.request_update_check_with(true, &refine).unwrap();
    assert!(check.check.in_flight);
    let check_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = service.inspect_cached().unwrap();
        if !status.check.in_flight {
            assert!(status.check.failure.is_none(), "{status:?}");
            assert!(status.source.update_available, "{status:?}");
            break;
        }
        assert!(
            Instant::now() < check_deadline,
            "source check did not settle"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let provider = root.join("smoke-ai");
    fs::write(
        &provider,
        "#!/bin/sh\nprompt=$1\ncommand_for() { printf '%s\\n' \"$prompt\" | sed -n \"/ --action $1$/p\" | head -1; }\nfor action in inspect pause-admission observe-work refresh-source; do command=$(command_for \"$action\"); sh -c \"$command\" || exit 1; done\ncommand=$(command_for prepare-candidate)\nprepared=false\nfor attempt in $(seq 1 100); do if sh -c \"$command\"; then prepared=true; break; fi; sleep 0.1; done\n[ \"$prepared\" = true ] || exit 1\ncommand=$(command_for handoff-promotion)\nsh -c \"$command\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous_provider = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }
    let queued = service.queue_agent_with("smoke-ai", &refine).unwrap();
    assert_eq!(queued.agent_provider.as_deref(), Some("smoke-ai"));

    let operation_path = runtime_root
        .join(port.to_string())
        .join(SOURCE_PROMOTION_STATE_FILE);
    let deadline = Instant::now() + Duration::from_secs(120);
    let operation: SourcePromotionOperation = loop {
        if let Ok(bytes) = fs::read(&operation_path)
            && let Ok(operation) = serde_json::from_slice::<SourcePromotionOperation>(&bytes)
            && matches!(operation.status.as_str(), "succeeded" | "failed")
        {
            break operation;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for source promotion {}",
            operation_path.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        operation.status, "succeeded",
        "source promotion failed: {operation:?}"
    );
    assert_eq!(operation.service_manager.as_deref(), Some("systemd_user"));
    assert!(operation.registration_updated);
    let candidate = PathBuf::from(operation.candidate_executable.as_ref().unwrap());
    let observed = PathBuf::from(operation.observed_executable.as_ref().unwrap());
    assert_eq!(
        fs::canonicalize(&candidate).unwrap(),
        fs::canonicalize(&observed).unwrap()
    );
    let unit = fs::read_to_string(&unit_path).unwrap();
    assert!(
        unit.contains(&candidate.display().to_string()),
        "installed unit did not select candidate: {unit}"
    );
    let live = live_daemon_executable(port).unwrap();
    assert_eq!(
        fs::canonicalize(live).unwrap(),
        fs::canonicalize(candidate).unwrap()
    );
    assert_eq!(operation.workflow_pause_restored, Some(true));
    assert_eq!(
        operation
            .reconciliation_evidence
            .as_ref()
            .and_then(|value| value.get("event_stream_available")),
        Some(&json!(true))
    );
    unsafe {
        match previous_provider {
            Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
            None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
        }
    }
}

#[cfg(target_os = "linux")]
struct RealSystemdPromotionCleanup {
    unit_name: String,
    unit_path: PathBuf,
    root: PathBuf,
}

#[cfg(target_os = "linux")]
impl Drop for RealSystemdPromotionCleanup {
    fn drop(&mut self) {
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", &self.unit_name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = fs::remove_file(&self.unit_path);
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(target_os = "linux")]
fn initialize_candidate_source_repository(source: &Path, origin: &Path) {
    fs::create_dir_all(source.join("src")).unwrap();
    git_ok(source, &["init", "--quiet", "."]).unwrap();
    git_ok(source, &["config", "user.email", "refine-test@example.com"]).unwrap();
    git_ok(source, &["config", "user.name", "Refine Test"]).unwrap();
    fs::write(
        source.join("Cargo.toml"),
        "[package]\nname = \"refine\"\nversion = \"0.0.1\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(source.join("src/main.rs"), candidate_daemon_source("prior")).unwrap();
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
    fs::write(
        source.join("src/main.rs"),
        candidate_daemon_source("candidate"),
    )
    .unwrap();
    git_ok(source, &["add", "src/main.rs"]).unwrap();
    git_ok(source, &["commit", "--quiet", "-m", "candidate"]).unwrap();
    git_ok(source, &["push", "--quiet", "origin", "HEAD:main"]).unwrap();
    git_ok(source, &["reset", "--quiet", "--hard", &prior]).unwrap();
}

#[cfg(target_os = "linux")]
fn candidate_daemon_source(identity: &str) -> String {
    format!(
        r#"use std::fs;
use std::io::{{Read, Write}};
use std::net::{{TcpListener, TcpStream}};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {{
    let args: Vec<String> = std::env::args().collect();
    let port = args.windows(2).find(|pair| pair[0] == "--port")
        .and_then(|pair| pair[1].parse::<u16>().ok()).expect("port");
    if args.iter().any(|arg| arg == "--foreground") {{
        let runtime_root = args.windows(2).find(|pair| pair[0] == "--runtime-root")
            .map(|pair| PathBuf::from(&pair[1])).expect("runtime root");
        let port_root = runtime_root.join(port.to_string());
        fs::create_dir_all(&port_root).unwrap();
        let status = format!("{{{{\"port\":{{}},\"daemon_healthy\":true,\"web_available\":true,\"worker_state\":\"idle\",\"target_app_state\":\"detached\",\"launch_mode\":\"binary\",\"active_operations\":[],\"degraded_integrations\":[]}}}}", port);
        fs::write(port_root.join("daemon-status.json"), status).unwrap();
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        for stream in listener.incoming() {{
            let mut stream = stream.unwrap();
            let mut request = [0_u8; 2048];
            let count = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..count]);
            if request.starts_with("GET /api/sse ") {{
                let body = "event: connected\ndata: {{\"ok\":true}}\n\n";
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {{}}\r\nConnection: close\r\n\r\n{{}}", body.len(), body);
                stream.write_all(response.as_bytes()).unwrap();
                continue;
            }}
            let executable = std::env::current_exe().unwrap().display().to_string()
                .replace('\\', "\\\\").replace('"', "\\\"");
            let body = format!("{{{{\"product\":\"refine\",\"version\":\"{identity}\",\"executable_path\":\"{{}}\"}}}}", executable);
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

#[cfg(target_os = "linux")]
fn run_refine_command(
    refine: &Path,
    args: &[&str],
    environment: &[(&str, String)],
) -> std::process::Output {
    let mut command = Command::new(refine);
    command.args(args);
    for (name, value) in environment {
        command.env(name, value);
    }
    command.output().unwrap()
}

#[cfg(target_os = "linux")]
fn assert_command_succeeded(label: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{label} failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "linux")]
fn wait_for_reachable(port: u16, timeout: std::time::Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while crate::process::supervisor::lifecycle::http_reachability_probe(port)
        != crate::process::supervisor::lifecycle::DaemonReachability::Reachable
    {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for daemon on {port}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

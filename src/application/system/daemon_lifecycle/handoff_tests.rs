use super::*;
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn handoff_labels_are_safe_for_systemd_and_launchd() {
    assert_eq!(
        sanitize_label("lifecycle:8082/source 1"),
        "lifecycle-8082-source-1"
    );
}

#[test]
fn handoff_receipt_identity_and_argument_fingerprint_are_attempt_scoped() {
    let first = RestartSafeHandoff {
        executable: PathBuf::from("/mock/refine"),
        args: vec!["helper".to_string(), "attempt-1".to_string()],
        cwd: PathBuf::from("/tmp"),
        label: "source-operation-attempt-1".to_string(),
    };
    let second = RestartSafeHandoff {
        args: vec!["helper".to_string(), "attempt-2".to_string()],
        label: "source-operation-attempt-2".to_string(),
        ..first.clone()
    };
    assert_ne!(
        handoff_argument_fingerprint(&first),
        handoff_argument_fingerprint(&second)
    );
    assert_eq!(
        handoff_mechanism_identity(&first, Some("systemd_user")).unwrap(),
        "source-operation-attempt-1"
    );
    assert_eq!(
        handoff_mechanism_identity(&first, Some("launchd_login_item")).unwrap(),
        "com.refine.source-operation-attempt-1"
    );
}

#[cfg(unix)]
#[test]
fn detached_submit_returns_observable_and_exactly_terminable_receipt() {
    let handoff = RestartSafeHandoff {
        executable: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), "sleep 30".to_string()],
        cwd: std::env::temp_dir(),
        label: format!("receipt-test-{}", std::process::id()),
    };
    let launcher = HostRestartSafeHandoffLauncher;
    let receipt = launcher.submit(&handoff, None).unwrap();
    assert_eq!(receipt.mechanism, "detached");
    assert!(receipt.pid.is_some());
    assert_eq!(
        launcher.observe(&receipt).unwrap(),
        HandoffObservation::Live
    );
    launcher.terminate(&receipt).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while launcher.observe(&receipt).unwrap() == HandoffObservation::Live
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_ne!(
        launcher.observe(&receipt).unwrap(),
        HandoffObservation::Live
    );
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an available systemd user manager; platform-gated integration evidence"]
fn real_systemd_handoff_survives_stopping_the_origin_service_group() {
    run_real_systemd_handoff_control("stop");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an available systemd user manager; platform-gated integration evidence"]
fn real_systemd_handoff_survives_restarting_the_origin_service_group() {
    run_real_systemd_handoff_control("restart");
}

#[cfg(target_os = "linux")]
fn run_real_systemd_handoff_control(action: &str) {
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

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "refine-systemd-handoff-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&root).unwrap();
    let marker = root.join("settled");
    let submitted = root.join("submitted");
    let controlled = root.join("controlled");
    let unit = format!("refine-handoff-origin-{}-{nonce}", std::process::id());
    let test_executable = std::env::current_exe().unwrap();
    let child_test =
        "application::system::daemon_lifecycle::handoff::tests::real_systemd_handoff_child";
    let status = Command::new("systemd-run")
        .args([
            "--user",
            "--quiet",
            "--property=Type=exec",
            &format!("--unit={unit}"),
            &format!(
                "--setenv=REFINE_SYSTEMD_HANDOFF_MARKER={}",
                marker.display()
            ),
            &format!(
                "--setenv=REFINE_SYSTEMD_HANDOFF_SUBMITTED={}",
                submitted.display()
            ),
            &format!(
                "--setenv=REFINE_SYSTEMD_HANDOFF_CONTROLLED={}",
                controlled.display()
            ),
            &format!("--setenv=REFINE_SYSTEMD_HANDOFF_ACTION={action}"),
            &format!("--setenv=REFINE_SYSTEMD_HANDOFF_UNIT={unit}"),
            "--",
        ])
        .arg(&test_executable)
        .args([child_test, "--exact", "--ignored", "--nocapture"])
        .status()
        .unwrap();
    assert!(status.success(), "failed to start origin service");
    wait_for_path(&submitted, Duration::from_secs(10));
    wait_for_path(&marker, Duration::from_secs(10));
    assert_eq!(fs::read_to_string(&marker).unwrap(), "settled");
    if action == "restart" {
        wait_for_unit_state(&unit, true, Duration::from_secs(10));
        let _ = Command::new("systemctl")
            .args(["--user", "stop", &unit])
            .status();
    } else {
        wait_for_unit_state(&unit, false, Duration::from_secs(10));
    }

    let _ = Command::new("systemctl")
        .args(["--user", "reset-failed", &unit])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "helper invoked only by the real systemd handoff integration test"]
fn real_systemd_handoff_child() {
    let Some(marker) = std::env::var_os("REFINE_SYSTEMD_HANDOFF_MARKER") else {
        return;
    };
    let submitted = PathBuf::from(
        std::env::var_os("REFINE_SYSTEMD_HANDOFF_SUBMITTED").expect("submitted marker is required"),
    );
    let controlled = PathBuf::from(
        std::env::var_os("REFINE_SYSTEMD_HANDOFF_CONTROLLED")
            .expect("controlled marker is required"),
    );
    if fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&controlled)
        .is_err()
    {
        std::thread::sleep(Duration::from_secs(30));
        return;
    }
    let marker = PathBuf::from(marker);
    let script = format!("sleep 1; printf settled > {}", marker.display());
    HostRestartSafeHandoffLauncher
        .launch(
            &RestartSafeHandoff {
                executable: PathBuf::from("/bin/sh"),
                args: vec!["-c".to_string(), script],
                cwd: marker.parent().unwrap().to_path_buf(),
                label: format!("refine-handoff-child-{}", uuid::Uuid::new_v4()),
            },
            Some("systemd_user"),
        )
        .unwrap();
    fs::write(submitted, "submitted").unwrap();
    let action =
        std::env::var("REFINE_SYSTEMD_HANDOFF_ACTION").expect("handoff action is required");
    let unit = std::env::var("REFINE_SYSTEMD_HANDOFF_UNIT").expect("handoff unit is required");
    let _ = Command::new("systemctl")
        .args(["--user", &action, &unit])
        .status();
    std::thread::sleep(Duration::from_secs(30));
}

#[cfg(target_os = "linux")]
fn wait_for_path(path: &Path, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "linux")]
fn wait_for_unit_state(unit: &str, active: bool, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let observed = Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", unit])
            .status()
            .is_ok_and(|status| status.success());
        if observed == active {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {unit} active={active}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

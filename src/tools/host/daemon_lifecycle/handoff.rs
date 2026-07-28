use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::process::supervisor::errors::{RefineError, RefineResult};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RestartSafeHandoff {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub label: String,
}

pub(crate) trait RestartSafeHandoffLauncher {
    fn launch(
        &self,
        handoff: &RestartSafeHandoff,
        service_manager: Option<&str>,
    ) -> RefineResult<()>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HostRestartSafeHandoffLauncher;

impl RestartSafeHandoffLauncher for HostRestartSafeHandoffLauncher {
    fn launch(
        &self,
        handoff: &RestartSafeHandoff,
        service_manager: Option<&str>,
    ) -> RefineResult<()> {
        match service_manager {
            Some("systemd_user") => launch_systemd_handoff(handoff),
            Some("launchd_login_item") => launch_launchd_handoff(handoff),
            Some(other) => Err(RefineError::Conflict(format!(
                "cannot create a restart-safe handoff for unsupported service manager {other}"
            ))),
            None => launch_detached_handoff(handoff),
        }
    }
}

fn launch_systemd_handoff(handoff: &RestartSafeHandoff) -> RefineResult<()> {
    let unit = sanitize_label(&handoff.label);
    let mut command = Command::new("systemd-run");
    command
        .args([
            "--user",
            "--quiet",
            "--collect",
            "--property=Type=exec",
            &format!("--unit={unit}"),
            "--",
        ])
        .arg("/usr/bin/env")
        .args(inherited_handoff_environment())
        .arg(&handoff.executable)
        .args(&handoff.args)
        .current_dir(&handoff.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    run_submission(command, "systemd transient service", handoff)
}

fn launch_launchd_handoff(handoff: &RestartSafeHandoff) -> RefineResult<()> {
    let label = format!("com.refine.{}", sanitize_label(&handoff.label));
    let mut command = Command::new("launchctl");
    command
        .args(["submit", "-l", &label, "--"])
        .arg("/usr/bin/env")
        .args(inherited_handoff_environment())
        .arg(&handoff.executable)
        .args(&handoff.args)
        .current_dir(&handoff.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    run_submission(command, "launchd submitted job", handoff)
}

fn run_submission(
    mut command: Command,
    mechanism: &str,
    handoff: &RestartSafeHandoff,
) -> RefineResult<()> {
    let status = command.status().map_err(|error| {
        RefineError::Io(format!(
            "failed to submit restart-safe handoff {} through {mechanism}: {error}",
            handoff.executable.display()
        ))
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(RefineError::Degraded(format!(
            "restart-safe handoff submission through {mechanism} failed with status {status}"
        )))
    }
}

fn launch_detached_handoff(handoff: &RestartSafeHandoff) -> RefineResult<()> {
    let mut command = Command::new(&handoff.executable);
    command
        .args(&handoff.args)
        .current_dir(&handoff.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_detached_session(&mut command);
    command.spawn().map(|_| ()).map_err(|error| {
        RefineError::Io(format!(
            "failed to launch detached handoff {}: {error}",
            handoff.executable.display()
        ))
    })
}

#[cfg(unix)]
fn configure_detached_session(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_detached_session(_command: &mut Command) {}

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn inherited_handoff_environment() -> Vec<String> {
    [
        "PATH",
        "HOME",
        "XDG_RUNTIME_DIR",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "DBUS_SESSION_BUS_ADDRESS",
        "SSH_AUTH_SOCK",
        "CARGO_HOME",
        "RUSTUP_HOME",
    ]
    .into_iter()
    .filter_map(|name| {
        std::env::var_os(name).map(|value| format!("{name}={}", value.to_string_lossy()))
    })
    .collect()
}

pub(crate) fn handoff_cwd(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
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
            "tools::host::daemon_lifecycle::handoff::tests::real_systemd_handoff_child";
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
            std::env::var_os("REFINE_SYSTEMD_HANDOFF_SUBMITTED")
                .expect("submitted marker is required"),
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
}

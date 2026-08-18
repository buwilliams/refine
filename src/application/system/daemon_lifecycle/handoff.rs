use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{RefineError, RefineResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RestartSafeHandoff {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct HandoffLaunchReceipt {
    pub mechanism: String,
    pub mechanism_identity: String,
    pub submitted_at: String,
    pub executable: String,
    pub argument_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_identity: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HandoffObservation {
    Live,
    Exited,
    IdentityMismatch(String),
    Ambiguous(String),
}

pub(crate) trait RestartSafeHandoffLauncher {
    fn launch(
        &self,
        handoff: &RestartSafeHandoff,
        service_manager: Option<&str>,
    ) -> RefineResult<()>;

    fn submit(
        &self,
        handoff: &RestartSafeHandoff,
        service_manager: Option<&str>,
    ) -> RefineResult<HandoffLaunchReceipt> {
        self.launch(handoff, service_manager)?;
        Ok(HandoffLaunchReceipt {
            mechanism: handoff_mechanism(service_manager)?.to_string(),
            mechanism_identity: handoff_mechanism_identity(handoff, service_manager)?,
            submitted_at: handoff_timestamp(),
            executable: handoff.executable.display().to_string(),
            argument_fingerprint: handoff_argument_fingerprint(handoff),
            pid: None,
            process_identity: None,
        })
    }

    fn observe(&self, receipt: &HandoffLaunchReceipt) -> RefineResult<HandoffObservation> {
        observe_receipt(receipt)
    }

    fn terminate(&self, receipt: &HandoffLaunchReceipt) -> RefineResult<()> {
        terminate_receipt(receipt)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HostRestartSafeHandoffLauncher;

impl RestartSafeHandoffLauncher for HostRestartSafeHandoffLauncher {
    fn launch(
        &self,
        handoff: &RestartSafeHandoff,
        service_manager: Option<&str>,
    ) -> RefineResult<()> {
        self.submit(handoff, service_manager).map(|_| ())
    }

    fn submit(
        &self,
        handoff: &RestartSafeHandoff,
        service_manager: Option<&str>,
    ) -> RefineResult<HandoffLaunchReceipt> {
        match service_manager {
            Some("systemd_user") => launch_systemd_handoff(handoff),
            Some("launchd_user_daemon") => launch_launchd_handoff(handoff, "launchd_user_daemon"),
            Some("launchd_login_item") => launch_launchd_handoff(handoff, "launchd_login_item"),
            Some(other) => Err(RefineError::Conflict(format!(
                "cannot create a restart-safe handoff for unsupported service manager {other}"
            ))),
            None => launch_detached_handoff(handoff),
        }
    }
}

fn launch_systemd_handoff(handoff: &RestartSafeHandoff) -> RefineResult<HandoffLaunchReceipt> {
    let unit = sanitize_label(&handoff.label);
    if let Some(pid) = systemd_main_pid(&unit)? {
        return Ok(receipt(handoff, "systemd_user", unit, Some(pid)));
    }
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
    run_submission(command, "systemd transient service", handoff)?;
    let pid = systemd_main_pid(&unit).ok().flatten();
    Ok(receipt(handoff, "systemd_user", unit, pid))
}

fn launch_launchd_handoff(
    handoff: &RestartSafeHandoff,
    mechanism: &'static str,
) -> RefineResult<HandoffLaunchReceipt> {
    let label = format!("com.refine.{}", sanitize_label(&handoff.label));
    if Command::new("launchctl")
        .args([
            "print",
            &format!("gui/{}/{}", unsafe { libc::geteuid() }, label),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return Ok(receipt(handoff, mechanism, label, None));
    }
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
    run_submission(command, "launchd submitted job", handoff)?;
    Ok(receipt(handoff, mechanism, label, None))
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

fn launch_detached_handoff(handoff: &RestartSafeHandoff) -> RefineResult<HandoffLaunchReceipt> {
    let mut command = Command::new(&handoff.executable);
    command
        .args(&handoff.args)
        .current_dir(&handoff.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_detached_session(&mut command);
    let child = command.spawn().map_err(|error| {
        RefineError::Io(format!(
            "failed to launch detached handoff {}: {error}",
            handoff.executable.display()
        ))
    })?;
    Ok(receipt(
        handoff,
        "detached",
        format!("pid:{}", child.id()),
        Some(child.id()),
    ))
}

fn receipt(
    handoff: &RestartSafeHandoff,
    mechanism: &str,
    mechanism_identity: String,
    pid: Option<u32>,
) -> HandoffLaunchReceipt {
    HandoffLaunchReceipt {
        mechanism: mechanism.to_string(),
        mechanism_identity,
        submitted_at: handoff_timestamp(),
        executable: handoff.executable.display().to_string(),
        argument_fingerprint: handoff_argument_fingerprint(handoff),
        process_identity: pid.and_then(process_start_identity),
        pid,
    }
}

pub(crate) fn handoff_mechanism(service_manager: Option<&str>) -> RefineResult<&'static str> {
    match service_manager {
        Some("systemd_user") => Ok("systemd_user"),
        Some("launchd_user_daemon") => Ok("launchd_user_daemon"),
        Some("launchd_login_item") => Ok("launchd_login_item"),
        Some(other) => Err(RefineError::Conflict(format!(
            "cannot create a restart-safe handoff for unsupported service manager {other}"
        ))),
        None => Ok("detached"),
    }
}

pub(crate) fn handoff_mechanism_identity(
    handoff: &RestartSafeHandoff,
    service_manager: Option<&str>,
) -> RefineResult<String> {
    Ok(match handoff_mechanism(service_manager)? {
        "systemd_user" => sanitize_label(&handoff.label),
        "launchd_user_daemon" | "launchd_login_item" => {
            format!("com.refine.{}", sanitize_label(&handoff.label))
        }
        _ => format!("detached:{}", handoff_argument_fingerprint(handoff)),
    })
}

pub(crate) fn handoff_argument_fingerprint(handoff: &RestartSafeHandoff) -> String {
    let mut digest = Sha256::new();
    digest.update(handoff.executable.as_os_str().to_string_lossy().as_bytes());
    for arg in &handoff.args {
        digest.update([0]);
        digest.update(arg.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn handoff_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

fn observe_receipt(receipt: &HandoffLaunchReceipt) -> RefineResult<HandoffObservation> {
    match receipt.mechanism.as_str() {
        "systemd_user" => match systemd_main_pid(&receipt.mechanism_identity)? {
            Some(pid) if receipt.pid.is_none_or(|expected| expected == pid) => {
                Ok(HandoffObservation::Live)
            }
            Some(pid) => Ok(HandoffObservation::IdentityMismatch(format!(
                "systemd job {} now owns pid {pid}, expected {:?}",
                receipt.mechanism_identity, receipt.pid
            ))),
            None => Ok(HandoffObservation::Exited),
        },
        "launchd_user_daemon" | "launchd_login_item" => {
            let status = Command::new("launchctl")
                .args([
                    "print",
                    &format!(
                        "gui/{}/{}",
                        unsafe { libc::geteuid() },
                        receipt.mechanism_identity
                    ),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            Ok(if status.is_ok_and(|status| status.success()) {
                HandoffObservation::Live
            } else {
                HandoffObservation::Exited
            })
        }
        "detached" => observe_pid(receipt),
        other => Ok(HandoffObservation::Ambiguous(format!(
            "unsupported handoff receipt mechanism {other}"
        ))),
    }
}

fn observe_pid(receipt: &HandoffLaunchReceipt) -> RefineResult<HandoffObservation> {
    let Some(pid) = receipt.pid else {
        return Ok(HandoffObservation::Ambiguous(
            "detached handoff receipt has no pid".to_string(),
        ));
    };
    if unsafe { libc::kill(pid as i32, 0) } != 0 {
        return Ok(HandoffObservation::Exited);
    }
    #[cfg(target_os = "linux")]
    if std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .map(|(_, fields)| fields.starts_with('Z'))
        })
        == Some(true)
    {
        return Ok(HandoffObservation::Exited);
    }
    let actual = process_start_identity(pid);
    if receipt.process_identity.is_some() && actual != receipt.process_identity {
        return Ok(HandoffObservation::IdentityMismatch(format!(
            "pid {pid} start identity changed"
        )));
    }
    Ok(HandoffObservation::Live)
}

fn terminate_receipt(receipt: &HandoffLaunchReceipt) -> RefineResult<()> {
    match receipt.mechanism.as_str() {
        "systemd_user" => {
            let status = Command::new("systemctl")
                .args(["--user", "stop", &receipt.mechanism_identity])
                .status()
                .map_err(|error| {
                    RefineError::Io(format!("failed to stop handoff unit: {error}"))
                })?;
            if !status.success() {
                return Err(RefineError::Degraded(format!(
                    "failed to stop handoff unit {}: {status}",
                    receipt.mechanism_identity
                )));
            }
        }
        "launchd_user_daemon" | "launchd_login_item" => {
            let status = Command::new("launchctl")
                .args(["remove", &receipt.mechanism_identity])
                .status()
                .map_err(|error| {
                    RefineError::Io(format!("failed to remove handoff job: {error}"))
                })?;
            if !status.success() {
                return Err(RefineError::Degraded(format!(
                    "failed to remove handoff job {}: {status}",
                    receipt.mechanism_identity
                )));
            }
        }
        "detached" => {
            if matches!(observe_pid(receipt)?, HandoffObservation::Live)
                && let Some(pid) = receipt.pid
                && unsafe { libc::kill(pid as i32, libc::SIGTERM) } != 0
            {
                return Err(RefineError::Io(format!(
                    "failed to terminate exact detached handoff pid {pid}: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }
        other => {
            return Err(RefineError::Conflict(format!(
                "cannot terminate unsupported handoff mechanism {other}"
            )));
        }
    }
    Ok(())
}

fn systemd_main_pid(unit: &str) -> RefineResult<Option<u32>> {
    let output = Command::new("systemctl")
        .args(["--user", "show", unit, "--property=MainPID", "--value"])
        .output()
        .map_err(|error| RefineError::Io(format!("failed to observe systemd handoff: {error}")))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0))
}

#[cfg(target_os = "linux")]
fn process_start_identity(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    let start_time = after_name.split_whitespace().nth(19)?;
    Some(format!("linux-proc-start:{start_time}"))
}

#[cfg(not(target_os = "linux"))]
fn process_start_identity(_pid: u32) -> Option<String> {
    None
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
#[path = "handoff_tests.rs"]
mod tests;

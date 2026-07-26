use super::*;

pub(super) fn signal_os_process(
    pid: u32,
    signal: &str,
    process_group: bool,
) -> RefineResult<Option<String>> {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command.arg("/PID").arg(pid.to_string());
        if process_group {
            command.arg("/T");
        }
        if signal == "kill" {
            command.arg("/F");
        }
        let status = command.status().map_err(|error| {
            RefineError::Io(format!(
                "failed to signal process {pid} with taskkill: {error}"
            ))
        })?;
        if status.success() {
            Ok(None)
        } else {
            Ok(Some(format!(
                "taskkill returned {status}; process may already have exited"
            )))
        }
    }
    #[cfg(not(windows))]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        const SIGKILL: i32 = 9;
        let target = if process_group {
            -(pid as i32)
        } else {
            pid as i32
        };
        let os_signal = if signal == "kill" { SIGKILL } else { SIGTERM };
        if unsafe { kill(target, os_signal) } == 0 {
            Ok(None)
        } else {
            let error = std::io::Error::last_os_error();
            Ok(Some(format!(
                "kill signal {os_signal} returned {error}; process may already have exited"
            )))
        }
    }
}

pub(super) fn process_owns_group(process: &ManagedProcess) -> bool {
    process
        .details
        .as_deref()
        .and_then(|details| serde_json::from_str::<Value>(details).ok())
        .and_then(|details| {
            details
                .get("isolated_process_group")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

pub(super) fn pid_alive(pid: u32) -> RefineResult<bool> {
    #[cfg(windows)]
    {
        let output = Command::new("tasklist")
            .arg("/FI")
            .arg(format!("PID eq {pid}"))
            .output()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect process {pid} with tasklist: {error}"
                ))
            })?;
        Ok(String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
    }
    #[cfg(not(windows))]
    {
        if pid > i32::MAX as u32 {
            return Ok(false);
        }
        if unix_pid_is_zombie(pid)? {
            return Ok(false);
        }
        let status = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to inspect process {pid} with kill -0: {error}"
                ))
            })?;
        Ok(status.success())
    }
}

#[cfg(target_os = "linux")]
pub(super) fn unix_pid_is_zombie(pid: u32) -> RefineResult<bool> {
    let status_path = PathBuf::from(format!("/proc/{pid}/status"));
    let status = match fs::read_to_string(&status_path) {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to inspect process status {}: {error}",
                status_path.display()
            )));
        }
    };
    Ok(status
        .lines()
        .find_map(|line| line.strip_prefix("State:"))
        .is_some_and(|state| state.trim_start().starts_with('Z')))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn unix_pid_is_zombie(pid: u32) -> RefineResult<bool> {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect process state {pid} with ps: {error}"
            ))
        })?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_start()
        .starts_with('Z'))
}

pub(super) fn append_stream_file(output: &mut String, label: &str, path: &str) -> RefineResult<()> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(&path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read process {label} stream {}: {error}",
            path.display()
        ))
    })?;
    if text.trim().is_empty() {
        return Ok(());
    }
    output.push_str(&format!("== {label} ==\n"));
    output.push_str(&tail_text(&text, 16_000));
    if !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(())
}

pub(super) fn append_observed_stream_file(
    output: &mut String,
    label: &str,
    path: &str,
) -> RefineResult<bool> {
    let path = PathBuf::from(path);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read process {label} stream {}: {error}",
                path.display()
            )));
        }
    };
    if text.trim().is_empty() {
        return Ok(true);
    }
    output.push_str(&format!("== {label} ==\n"));
    output.push_str(&tail_text(&text, 16_000));
    if !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(true)
}

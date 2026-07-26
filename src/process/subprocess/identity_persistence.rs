use super::*;

pub(super) fn now_millis_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

/// Write JSON through a temp file and a rename so concurrent readers observe
/// either the previous content or the new content, never a truncated file.
/// Settings and registry records are read by the daemon while workflow threads
/// write them, so a plain `fs::write` lets a reader observe zero bytes.
pub(crate) fn write_json_atomically(path: &Path, encoded: &[u8], label: &str) -> RefineResult<()> {
    let Some(parent) = path.parent() else {
        return Err(RefineError::Io(format!(
            "failed to write {label} {}: path has no parent",
            path.display()
        )));
    };
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let tmp_path = parent.join(format!(".{file_name}.{}.tmp", new_process_id()));
    {
        let mut tmp = fs::File::create(&tmp_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create {label} temp file {}: {error}",
                tmp_path.display()
            ))
        })?;
        tmp.write_all(encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write {label} temp file {}: {error}",
                tmp_path.display()
            ))
        })?;
        tmp.sync_all().map_err(|error| {
            RefineError::Io(format!(
                "failed to sync {label} temp file {}: {error}",
                tmp_path.display()
            ))
        })?;
    }
    fs::rename(&tmp_path, path).map_err(|error| {
        let _ = fs::remove_file(&tmp_path);
        RefineError::Io(format!(
            "failed to write {label} {}: {error}",
            path.display()
        ))
    })
}

pub(super) fn new_process_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "proc-{}-{}-{}",
        now.as_millis(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OwnedProcessState {
    Alive,
    Exited,
    IdentityMismatch(Option<String>),
}

pub(super) fn confirmed_process_exit(
    process: &ManagedProcess,
    signal: &str,
    identity: &ManagedProcessIdentity,
    started: Instant,
) -> ConfirmedProcessExit {
    ConfirmedProcessExit {
        process_id: process.id.clone(),
        pid: process.pid,
        signal: signal.to_string(),
        os_identity: identity.os_identity.clone(),
        confirmed_exit: true,
        registry_retained_until_exit: true,
        registry_cleanup_completed: false,
        identity_cleanup_completed: false,
        waited_ms: started.elapsed().as_millis(),
    }
}

pub(super) fn remove_file_if_present(path: &Path, label: &str) -> RefineResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RefineError::Io(format!(
            "failed to remove {label} {}: {error}",
            path.display()
        ))),
    }
}

pub(super) fn process_identity_mismatch(
    process: &ManagedProcess,
    expected: &ManagedProcessIdentity,
    actual: Option<&str>,
) -> RefineError {
    RefineError::Conflict(format!(
        "managed process {} PID identity mismatch (expected {}, observed {}); termination was not requested, and its process record and identity evidence were retained for recovery",
        process.id,
        expected.os_identity.as_deref().unwrap_or("unavailable"),
        actual.unwrap_or("unavailable")
    ))
}

#[cfg(target_os = "linux")]
pub(super) fn os_process_identity(pid: u32) -> RefineResult<Option<String>> {
    let stat_path = PathBuf::from(format!("/proc/{pid}/stat"));
    let stat = match fs::read_to_string(&stat_path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to read process identity {}: {error}",
                stat_path.display()
            )));
        }
    };
    let end = stat.rfind(')').ok_or_else(|| {
        RefineError::Serialization(format!(
            "failed to parse process identity {}: missing command terminator",
            stat_path.display()
        ))
    })?;
    let start_ticks = stat[end + 1..].split_whitespace().nth(19).ok_or_else(|| {
        RefineError::Serialization(format!(
            "failed to parse process identity {}: missing start time",
            stat_path.display()
        ))
    })?;
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(Some(format!("linux:{boot_id}:{start_ticks}")))
}

#[cfg(all(unix, not(target_os = "linux")))]
pub(super) fn os_process_identity(pid: u32) -> RefineResult<Option<String>> {
    let output = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect process identity {pid} with ps: {error}"
            ))
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let started = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!started.is_empty()).then(|| format!("unix:{started}")))
}

#[cfg(windows)]
pub(super) fn os_process_identity(pid: u32) -> RefineResult<Option<String>> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).StartTime.Ticks"),
        ])
        .output()
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect process identity {pid} with PowerShell: {error}"
            ))
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let started = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!started.is_empty()).then(|| format!("windows:{started}")))
}

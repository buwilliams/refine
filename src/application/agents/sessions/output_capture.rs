//! Interruptible transcript capture and bounded final drain.
use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const PTY_EOF_RETRY_INITIAL: Duration = Duration::from_millis(20);
const PTY_EOF_RETRY_MAX: Duration = Duration::from_millis(500);

/// Move a failed session's transcript out of the artifact set that supervisor
/// cleanup deletes. Renamed in place (same directory, `.failed` suffix) so the
/// evidence stays node-local under the runtime tree.
pub(super) fn preserve_failed_transcript(stdout_path: &Path) -> Option<PathBuf> {
    let file_name = stdout_path.file_name()?.to_str()?;
    let preserved = stdout_path.with_file_name(format!("{file_name}.failed"));
    fs::rename(stdout_path, &preserved).ok()?;
    Some(preserved)
}

/// Copy PTY output into the transcript until the child is confirmed gone.
///
/// A zero-byte read is not trusted as EOF: on Linux the master reads EIO —
/// which the vendored PTY surfaces as `Ok(0)` — whenever no process
/// momentarily holds the slave side, and the agent spawns and reaps its own
/// subprocesses. Treating that transient state as EOF once froze the activity
/// clock and let the idle watchdog kill a live agent, so zero-byte reads are
/// retried with capped backoff until the poll loop confirms via `child_exited`
/// that the child was actually reaped.
pub(super) fn pump_pty_output(
    reader: &mut dyn Read,
    transcript: &mut fs::File,
    transcript_path: &Path,
    activity: &Mutex<Instant>,
    child_exited: &AtomicBool,
) -> RefineResult<()> {
    let mut buffer = [0_u8; 4096];
    let mut eof_retry = PTY_EOF_RETRY_INITIAL;
    let mut drain_deadline = None;
    loop {
        if child_exited.load(Ordering::SeqCst) {
            let deadline =
                drain_deadline.get_or_insert_with(|| Instant::now() + Duration::from_millis(200));
            if Instant::now() >= *deadline {
                return Err(RefineError::Io(
                    "Goal Agent transcript capture incomplete: output continued beyond final drain"
                        .into(),
                ));
            }
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                if child_exited.load(Ordering::SeqCst) {
                    return Ok(());
                }
                thread::sleep(eof_retry);
                eof_retry = (eof_retry * 2).min(PTY_EOF_RETRY_MAX);
            }
            Ok(count) => {
                eof_retry = PTY_EOF_RETRY_INITIAL;
                *activity.lock().expect("Goal Agent activity clock poisoned") = Instant::now();
                transcript.write_all(&buffer[..count]).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to append Goal Agent transcript {}: {error}",
                        transcript_path.display()
                    ))
                })?;
                transcript.flush().map_err(|error| {
                    RefineError::Io(format!(
                        "failed to flush Goal Agent transcript {}: {error}",
                        transcript_path.display()
                    ))
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child_exited.load(Ordering::SeqCst) {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "Goal Agent output stream failed: {error}"
                )));
            }
        }
    }
}

/// The distinct failure for a reader thread that died while the child was
/// still running. The activity clock is frozen from that moment, so letting
/// the idle watchdog speak would blame a live agent for the harness's own
/// capture fault.
pub(super) fn transcript_capture_failure(
    join_result: std::thread::Result<RefineResult<()>>,
) -> RefineError {
    let cause = match join_result {
        Ok(Err(error)) => error.to_string(),
        Ok(Ok(())) => "transcript reader stopped without an error".to_string(),
        Err(_) => "transcript reader panicked".to_string(),
    };
    RefineError::Io(format!(
        "Goal Agent transcript capture failed while the agent was still running: {cause}"
    ))
}

/// Poll a dedicated duplicate: a quiet slave or detached descendant cannot
/// hold the transcript thread inside a blocking read indefinitely.
#[cfg(unix)]
pub(super) fn interruptible_reader(
    master: &dyn portable_pty::MasterPty,
) -> std::io::Result<Box<dyn Read + Send>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    struct Reader(fs::File);
    impl Read for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let mut fd = libc::pollfd {
                fd: self.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut fd, 1, 20) };
            if ready < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if ready == 0 {
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            match self.0.read(bytes) {
                Err(e) if e.raw_os_error() == Some(libc::EIO) => Ok(0),
                result => result,
            }
        }
    }
    let fd = master
        .as_raw_fd()
        .ok_or_else(|| std::io::Error::other("PTY has no pollable descriptor"))?;
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(Box::new(Reader(unsafe {
        fs::File::from_raw_fd(duplicate)
    })))
}

#[cfg(not(unix))]
pub(super) fn interruptible_reader(
    _: &dyn portable_pty::MasterPty,
) -> std::io::Result<Box<dyn Read + Send>> {
    Err(std::io::Error::other(
        "interruptible PTY capture is unavailable on this platform",
    ))
}

pub(super) fn finish_capture(
    reader: &mut Option<thread::JoinHandle<RefineResult<()>>>,
) -> RefineResult<()> {
    let Some(handle) = reader.take() else {
        return Ok(());
    };
    let deadline = Instant::now() + Duration::from_millis(700);
    while !handle.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    if !handle.is_finished() {
        return Err(RefineError::Io("Goal Agent transcript capture incomplete: final drain exceeded its bound; evidence retained".into()));
    }
    handle
        .join()
        .map_err(|_| RefineError::Io("Goal Agent output reader panicked".into()))?
}

pub(super) fn append_settlement_faults(
    error: RefineError,
    stop: Option<RefineError>,
    capture: Option<RefineError>,
) -> RefineError {
    if stop.is_none() && capture.is_none() {
        return error;
    }
    let mut message = error.to_string();
    if let Some(stop) = stop {
        message.push_str(&format!("; scope settlement: {stop}"));
    }
    if let Some(capture) = capture {
        message.push_str(&format!("; transcript capture: {capture}"));
    }
    RefineError::Degraded(message)
}

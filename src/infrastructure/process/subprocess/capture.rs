//! Standard pipe capture. No reader threads or EOF-dependent joins: each poll
//! performs bounded nonblocking work, leaving deadlines in the execution owner.
use super::*;
use std::os::fd::AsRawFd;

pub(super) const FINAL_DRAIN: Duration = Duration::from_millis(200);
const MEMORY_LIMIT: usize = 16 * 1024 * 1024;

pub(super) struct Capture<R> {
    reader: R,
    file: fs::File,
    bytes: Vec<u8>,
    pub eof: bool,
    failure: Option<String>,
    truncated: bool,
}

impl<R: Read + AsRawFd> Capture<R> {
    pub fn new(reader: R, path: &Path) -> RefineResult<Self> {
        let fd = reader.as_raw_fd();
        // These are exclusively owned read ends; the writer's flags are separate.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(RefineError::Io(format!(
                "failed to make capture interruptible: {}",
                std::io::Error::last_os_error()
            )));
        }
        let file = fs::File::create(path).map_err(|e| {
            RefineError::Io(format!("failed to create capture {}: {e}", path.display()))
        })?;
        Ok(Self {
            reader,
            file,
            bytes: Vec::new(),
            eof: false,
            failure: None,
            truncated: false,
        })
    }

    pub fn poll(&mut self, mut on_chunk: impl FnMut(&[u8])) -> RefineResult<bool> {
        if self.eof || self.failure.is_some() {
            return Ok(false);
        }
        let mut progress = false;
        let mut buffer = [0; 8192];
        // Even a continuously ready writer yields to deadline and receipt checks.
        for _ in 0..8 {
            let result = match self.reader.read(&mut buffer) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => {
                    progress = true;
                    let bytes = &buffer[..n];
                    let keep = n.min(MEMORY_LIMIT.saturating_sub(self.bytes.len()));
                    self.bytes.extend_from_slice(&bytes[..keep]);
                    self.truncated |= keep < n;
                    let written = self.file.write_all(bytes);
                    if written.is_ok() {
                        on_chunk(bytes);
                    }
                    written
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                self.failure = Some(e.to_string());
                return Err(RefineError::Io(format!("output capture failed: {e}")));
            }
        }
        Ok(progress)
    }

    pub fn evidence(&self, reason: &str) -> Value {
        json!({"complete": self.eof && self.failure.is_none() && !self.truncated,
            "eof": self.eof, "failure": self.failure, "buffer_truncated": self.truncated,
            "reason": if self.failure.is_some() { "capture failure" } else if self.truncated { "memory capture limit" } else if self.eof { "eof" } else { reason }})
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

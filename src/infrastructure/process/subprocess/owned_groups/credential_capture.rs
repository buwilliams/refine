//! Detached credential capture lives with the guardian, so daemon exit cannot
//! close the workload's pipes or leave raw output in a file.
use super::launch_scope::io_error;
use super::*;
use crate::infrastructure::process::redaction::Redactor;
use std::os::fd::FromRawFd;

pub(super) struct CredentialCapture {
    stdout: fs::File,
    stderr: fs::File,
    redactor: Redactor,
}
type Pump = std::thread::JoinHandle<RefineResult<()>>;
impl CredentialCapture {
    pub(super) fn prepare(
        spec: &ManagedProcessSpec,
        command: &mut Command,
    ) -> RefineResult<Option<Self>> {
        let Some(names) = spec
            .metadata
            .get("provider_credential_env")
            .and_then(Value::as_array)
        else {
            return Ok(None);
        };
        let secrets = names
            .iter()
            .map(|name| {
                let name = name.as_str().ok_or_else(|| {
                    RefineError::InvalidInput("invalid credential capture reference".into())
                })?;
                std::env::var(name).map(String::into_bytes).map_err(|_| {
                    RefineError::InvalidInput(format!(
                        "credential capture reference {name} is missing"
                    ))
                })
            })
            .collect::<RefineResult<Vec<_>>>()?;
        let duplicate = |fd| {
            let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 4) };
            if copy < 0 {
                Err(io_error(std::io::Error::last_os_error()))
            } else {
                Ok(unsafe { fs::File::from_raw_fd(copy) })
            }
        };
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        Ok(Some(Self {
            stdout: duplicate(1)?,
            stderr: duplicate(2)?,
            redactor: Redactor::new(secrets),
        }))
    }
    pub(super) fn start(self, child: &mut std::process::Child) -> Vec<Pump> {
        vec![
            pump(
                child.stdout.take().expect("piped stdout"),
                self.stdout,
                self.redactor.clone(),
            ),
            pump(
                child.stderr.take().expect("piped stderr"),
                self.stderr,
                self.redactor,
            ),
        ]
    }
}
fn pump(
    mut reader: impl Read + Send + 'static,
    mut output: fs::File,
    mut redactor: Redactor,
) -> Pump {
    std::thread::spawn(move || {
        let mut bytes = [0; 8192];
        loop {
            let count = match reader.read(&mut bytes) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                result => result.map_err(io_error)?,
            };
            output
                .write_all(&redactor.push(&bytes[..count], count == 0))
                .map_err(io_error)?;
            if count == 0 {
                return Ok(());
            }
        }
    })
}
pub(super) fn finish(pumps: Vec<Pump>) -> RefineResult<()> {
    let deadline = Instant::now() + Duration::from_secs(1);
    for pump in pumps {
        while !pump.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !pump.is_finished() {
            return Err(RefineError::Degraded(
                "credential capture did not drain after scope exit; evidence retained".into(),
            ));
        }
        pump.join().map_err(|_| {
            RefineError::Degraded("credential capture failed; evidence retained".into())
        })??;
    }
    Ok(())
}

use super::*;

pub fn managed_pid_is_alive(pid: u32) -> RefineResult<bool> {
    pid_alive(pid)
}

pub(super) enum ProcessOutputEvent {
    Chunk {
        stream: ManagedProcessOutputStream,
        bytes: Vec<u8>,
    },
    Done,
    Error(RefineError),
}

pub(super) fn spawn_output_reader<R>(
    mut reader: R,
    stream: ManagedProcessOutputStream,
    tx: mpsc::Sender<ProcessOutputEvent>,
) -> std::thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = tx.send(ProcessOutputEvent::Done);
                    return;
                }
                Ok(read) => {
                    let _ = tx.send(ProcessOutputEvent::Chunk {
                        stream,
                        bytes: buffer[..read].to_vec(),
                    });
                }
                Err(error) => {
                    let _ = tx.send(ProcessOutputEvent::Error(RefineError::Io(format!(
                        "failed to read managed process {stream:?}: {error}"
                    ))));
                    return;
                }
            }
        }
    })
}

impl FileProcessSupervisor {
    pub(super) fn recover_running_process(
        &self,
        process: &mut ManagedProcess,
    ) -> RefineResult<bool> {
        match process.pid {
            Some(pid) if pid_alive(pid)? => {}
            Some(_) => {
                process.state = "exited".to_string();
                process.details = Some(append_detail(
                    process.details.take(),
                    "process was not alive during recovery",
                ));
                self.write_process(process)?;
                self.archive_terminal_process(process)?;
                return Ok(false);
            }
            None => {
                process.state = "interrupted".to_string();
                process.details = Some(append_detail(
                    process.details.take(),
                    "running process had no pid during recovery",
                ));
                self.write_process(process)?;
                self.archive_terminal_process(process)?;
                return Ok(false);
            }
        }
        Ok(true)
    }
}

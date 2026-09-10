use super::*;

pub fn managed_pid_is_alive(pid: u32) -> RefineResult<bool> {
    pid_alive(pid)
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

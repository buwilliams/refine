//! Keep ancestry inspectable until the owned tree can no longer fork during termination.
use super::*;

pub(super) struct QuiescedGroup {
    pub(super) group: OwnedGroup,
    suspended: BTreeMap<u32, String>,
}
impl Drop for QuiescedGroup {
    fn drop(&mut self) {
        // A failed inspection or bounded stop must not leave newly suspended survivors frozen.
        // Processes already stopped before this operation retain that state.
        for (pid, token) in &self.suspended {
            if os_process_identity(*pid).ok().flatten().as_ref() == Some(token) {
                unsafe {
                    libc::kill(*pid as i32, libc::SIGCONT);
                }
            }
        }
    }
}

impl FileProcessSupervisor {
    pub(super) fn quiesce_owned_group(
        &self,
        group: &OwnedGroup,
        deadline: Instant,
    ) -> RefineResult<QuiescedGroup> {
        let mut quiesced = QuiescedGroup {
            group: group.clone(),
            suspended: BTreeMap::new(),
        };
        loop {
            if Instant::now() >= deadline {
                return Err(RefineError::Degraded(
                    "owned process tree could not be quiesced within the stop budget; ownership evidence retained".into(),
                ));
            }
            for (pid, token) in &quiesced.group.witnesses {
                if os_process_identity(*pid)?.as_ref() != Some(token) || process_stopped(*pid)? {
                    continue;
                }
                if unsafe { libc::kill(*pid as i32, libc::SIGSTOP) } != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() == Some(libc::ESRCH) {
                        continue;
                    }
                    return Err(RefineError::Degraded(format!(
                        "could not quiesce owned process {pid}: {error}; replacement refused"
                    )));
                }
                quiesced.suspended.insert(*pid, token.clone());
            }
            #[cfg(test)]
            tests::after_suspend(&quiesced.group.process.id);
            // Parents remain alive, so a child that forked after the prior scan still has
            // discoverable ancestry even if it created a separate process group/session.
            quiesced.group = self.observe_owned_group(&quiesced.group)?;
            if quiesced.group.confirmed_exit {
                return Ok(quiesced);
            }
            let mut all_stopped = true;
            for (pid, token) in &quiesced.group.witnesses {
                if os_process_identity(*pid)?.as_ref() == Some(token) {
                    all_stopped &= process_stopped(*pid)?;
                }
            }
            if all_stopped && Instant::now() < deadline {
                let verified = self.observe_owned_group(&quiesced.group)?;
                if verified.witnesses == quiesced.group.witnesses {
                    quiesced.group = verified;
                    return Ok(quiesced);
                }
                quiesced.group = verified;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn process_stopped(pid: u32) -> RefineResult<bool> {
    let tasks = match fs::read_dir(format!("/proc/{pid}/task")) {
        Ok(tasks) => tasks,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(RefineError::Io(error.to_string())),
    };
    let mut observed = false;
    for task in tasks {
        let path = task
            .map_err(|e| RefineError::Io(e.to_string()))?
            .path()
            .join("stat");
        let stat = match fs::read_to_string(path) {
            Ok(stat) => stat,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(RefineError::Io(error.to_string())),
        };
        let state = stat
            .rsplit_once(')')
            .and_then(|(_, fields)| fields.split_whitespace().next());
        if !matches!(state, Some("T" | "t" | "Z" | "X")) {
            return Ok(false);
        }
        observed = true;
    }
    Ok(observed)
}

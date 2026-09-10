//! Bounded, coordinated termination of owned work; only lifetime proof permits release.
use super::*;
impl FileProcessSupervisor {
    pub fn stop_owned_group(
        &self,
        expected: &OwnedGroup,
        timeout: Duration,
    ) -> RefineResult<OwnedGroup> {
        let deadline = Instant::now() + timeout;
        crate::infrastructure::process::supervisor::coordination::with_lock_deadline(
            deadline,
            || self.stop_owned_group_before(expected, deadline),
        )
    }
    fn stop_owned_group_before(
        &self,
        expected: &OwnedGroup,
        deadline: Instant,
    ) -> RefineResult<OwnedGroup> {
        let timeout = deadline.saturating_duration_since(Instant::now());
        // Worker and Agent groups can overlap through ancestry. Serialize tree stops across
        // both registries so one failed stop cannot resume another stop's frozen witnesses.
        let runtime = if self.runtime_root.file_name().and_then(|s| s.to_str()) == Some("agents") {
            self.runtime_root.parent().unwrap_or(&self.runtime_root)
        } else {
            &self.runtime_root
        };
        let _stop_fence =
            crate::infrastructure::process::supervisor::coordination::with_lock_timeout(
                timeout.min(Duration::from_millis(200)),
                || {
                    crate::infrastructure::process::supervisor::coordination::acquire_record_lock(
                        runtime,
                        "owned-tree-stop",
                    )
                },
            )?;
        #[cfg(test)]
        if expected
            .process
            .details
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .is_some_and(|v| v["test_termination_failure"] == true)
        {
            return Err(RefineError::Degraded(
                "injected group termination failure".into(),
            ));
        }
        let mut group = self.observe_owned_group(expected)?;
        if group.confirmed_exit {
            return Ok(group);
        }
        #[cfg(all(test, target_os = "linux"))]
        tests::before_signal(&group.process.id);
        // Stop forks before killing their parents, otherwise a newly isolated child can
        // lose its ancestry between the ownership scan and termination.
        #[cfg(target_os = "linux")]
        let quiesced = self.quiesce_owned_group(&group, deadline)?;
        #[cfg(target_os = "linux")]
        {
            group = quiesced.group.clone();
        }
        // An escaped descendant can outlive the original group. Its identity does not
        // authorize signalling a now-reused group number: require a witness in that group.
        #[cfg(unix)]
        if let Some(pgid) = group.pgid {
            let mut witnessed_group = false;
            for (pid, token) in &group.witnesses {
                if os_process_identity(*pid)?.as_ref() == Some(token)
                    && unsafe { libc::getpgid(*pid as i32) } == pgid as i32
                {
                    witnessed_group = true;
                    break;
                }
            }
            if witnessed_group && let Some(error) = signal_os_process(pgid, "kill", true)? {
                return Err(RefineError::Degraded(error));
            }
        }
        // A child may have created a new session. Witnessed descendants are still owned;
        // recheck each OS identity before signalling outside the original group.
        for (child, token) in &group.witnesses {
            if os_process_identity(*child)?.as_ref() == Some(token)
                && let Some(error) = signal_os_process(*child, "kill", false)?
            {
                return Err(RefineError::Degraded(error));
            }
        }
        loop {
            group = self.observe_owned_group(&group)?;
            if group.confirmed_exit {
                return Ok(group);
            }
            if let Some(reason) = &group.ownership_gap {
                return Err(RefineError::Degraded(format!(
                    "{}: {reason}; exit unverified; retain capacity and inspect process diagnostics",
                    group.process.id
                )));
            }
            if Instant::now() >= deadline {
                return Err(RefineError::Degraded(
                    "owned process group did not confirm exit; evidence retained".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

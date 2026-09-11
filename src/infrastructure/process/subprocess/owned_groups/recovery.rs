//! Recover missing group records only from their original complete launch-scope proof.
use super::*;

impl FileProcessSupervisor {
    pub(super) fn restore_exited_owned_group(
        &self,
        expected: &ManagedProcess,
    ) -> RefineResult<Option<OwnedGroup>> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = expected;
            Ok(None)
        }
        #[cfg(target_os = "linux")]
        {
            use crate::infrastructure::process::supervisor::coordination::{
                acquire_record_lock, with_lock_timeout,
            };
            fn scope(process: &ManagedProcess) -> RefineResult<Option<launch_scope::LaunchScope>> {
                let details: Value = process
                    .details
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()
                    .map_err(|e| RefineError::Serialization(e.to_string()))?
                    .unwrap_or(Value::Null);
                details
                    .get("launch_scope")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|e| RefineError::Serialization(e.to_string()))
            }
            let Some(proof) = scope(expected)? else {
                return Ok(None);
            };
            let _registration = self.artifact_registration_fence(&expected.id)?;
            let _group = with_lock_timeout(Duration::from_millis(200), || {
                acquire_record_lock(&self.runtime_root, &format!("owned-group:{}", expected.id))
            })?;
            // Another observer may have restored it. Never replace existing or corrupt evidence.
            match fs::read(self.group_path(&expected.id)) {
                Ok(bytes) => {
                    let group: OwnedGroup = serde_json::from_slice(&bytes)
                        .map_err(|e| RefineError::Serialization(e.to_string()))?;
                    Self::ensure_same_registration(expected, &group.process)?;
                    return Ok(Some(group));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(RefineError::Io(e.to_string())),
            }
            let current = match fs::read(self.processes_dir().join(format!("{}.json", expected.id)))
            {
                Ok(bytes) => serde_json::from_slice::<ManagedProcess>(&bytes)
                    .map_err(|e| RefineError::Serialization(e.to_string()))?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    match self.inspect_terminal(&expected.id) {
                        Ok(process) => process,
                        Err(RefineError::NotFound(_)) => return Ok(None),
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(RefineError::Io(e.to_string())),
            };
            Self::ensure_same_registration(expected, &current)?;
            if scope(&current)?.as_ref() != Some(&proof) {
                return Err(RefineError::Conflict(
                    "launch ownership scope changed during recovery".into(),
                ));
            }
            let mut group = OwnedGroup {
                runtime_root: self
                    .runtime_root
                    .canonicalize()
                    .map_err(|e| RefineError::Io(e.to_string()))?,
                process: current,
                pgid: None,
                witnesses: BTreeMap::new(),
                confirmed_exit: false,
                ownership_gap: None,
                launch_scope: Some(proof.clone()),
            };
            // This checks canonical runtime, registration ID, workload PID, device,
            // inode and complete guardian receipt. An exited leader or empty scan
            // cannot substitute for that receipt, and no live PID is signalled.
            if !proof.proof(&group)? {
                return Ok(None);
            }
            group.confirmed_exit = true;
            self.write_owned_group(&group)?;
            Ok(Some(group))
        }
    }
}

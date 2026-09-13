//! Recover ownership only from original, identity-bound kernel lifetime evidence.
use super::*;

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct EnclosingScopeExit {
    pub process_id: String,
    pub launch_scope: launch_scope::LaunchScope,
    pub previous_ownership_gap: Option<String>,
}

#[cfg(target_os = "linux")]
fn linux_identity(identity: &str) -> Option<(uuid::Uuid, u64)> {
    let mut parts = identity.split(':');
    if parts.next()? != "linux" {
        return None;
    }
    let boot = uuid::Uuid::parse_str(parts.next()?).ok()?;
    let start = parts.next()?.parse().ok()?;
    (parts.next().is_none() && !boot.is_nil()).then_some((boot, start))
}

#[cfg(target_os = "linux")]
impl FileProcessSupervisor {
    /// A lost nested guardian can leave a partial receipt even though its enclosing
    /// workflow guardian subsequently reaped the entire tree. Accept only that
    /// direct launch relationship; a shared incarnation or witness alone is not ancestry.
    pub(super) fn enclosing_scope_exit_evidence(
        &self,
        child: &OwnedGroup,
    ) -> RefineResult<Option<EnclosingScopeExit>> {
        let Some(child_scope) = child.launch_scope.as_ref() else {
            return Ok(None);
        };
        let Some(child_pid) = child.process.pid else {
            return Ok(None);
        };
        let Some(launcher_pid) = registered_launcher_pid(&child.process.id) else {
            return Ok(None);
        };
        let Some(child_identity) = child
            .witnesses
            .get(&child_pid)
            .and_then(|s| linux_identity(s))
        else {
            return Ok(None);
        };
        let Some(child_guardian) = child_scope
            .guardian_identity
            .as_deref()
            .and_then(linux_identity)
        else {
            return Ok(None);
        };
        let current = os_process_identity(std::process::id())?;
        if current.as_deref().and_then(linux_identity).map(|id| id.0) != Some(child_identity.0)
            || child_guardian.0 != child_identity.0
            || child_guardian.1 > child_identity.1
            || child_pid == launcher_pid
            || child_pid == child_scope.guardian_pid
            || child_scope.alive()?
            || !self.original_isolated_scope(child)?
        {
            return Ok(None);
        }
        // Authenticate even the incomplete child's original inode and workload PID.
        // Empty or malformed files cannot establish that the launch handshake completed.
        if !matches!(child_scope.read_proof(child)?.len(), 4 | 8) {
            return Ok(None);
        }
        for (pid, token) in &child.witnesses {
            if linux_identity(token)
                .is_none_or(|id| id.0 != child_identity.0 || id.1 < child_identity.1)
                || os_process_identity(*pid)?.as_ref() == Some(token)
            {
                return Ok(None);
            }
        }
        let parent_root = if child
            .runtime_root
            .file_name()
            .is_some_and(|name| name == "agents")
        {
            child.runtime_root.parent().unwrap_or(&child.runtime_root)
        } else {
            &child.runtime_root
        };
        let parent_owner = FileProcessSupervisor::new(parent_root);
        for parent in parent_owner
            .owned_group_observations()?
            .into_iter()
            .flatten()
        {
            if parent.process.pid != Some(launcher_pid)
                || parent.process.id == child.process.id
                || parent.runtime_root != parent_root
                || parent.process.owner != ProcessOwner::Runner
                || ownership_incarnation(&parent.process) != ownership_incarnation(&child.process)
            {
                continue;
            }
            let metadata: Value =
                serde_json::from_str(parent.process.details.as_deref().unwrap_or("{}"))
                    .unwrap_or(Value::Null);
            if metadata["worker_kind"] != "workflow"
                || !parent_owner
                    .original_isolated_scope(&parent)
                    .unwrap_or(false)
            {
                continue;
            }
            let Some(scope) = parent.launch_scope.as_ref() else {
                continue;
            };
            let Some(parent_identity) = parent
                .witnesses
                .get(&launcher_pid)
                .and_then(|s| linux_identity(s))
            else {
                continue;
            };
            let Some(guardian_identity) =
                scope.guardian_identity.as_deref().and_then(linux_identity)
            else {
                continue;
            };
            if parent_identity.0 != child_identity.0
                || guardian_identity.0 != child_identity.0
                || guardian_identity.1 > parent_identity.1
                || parent_identity.1 > child_guardian.1
                || scope.guardian_pid == child_scope.guardian_pid
                || scope.proof_path == child_scope.proof_path
                || parent.witnesses.values().any(|token| {
                    linux_identity(token)
                        .is_none_or(|id| id.0 != child_identity.0 || id.1 < parent_identity.1)
                })
                || child
                    .witnesses
                    .iter()
                    .any(|(pid, token)| parent.witnesses.get(pid) != Some(token))
            {
                continue;
            }
            // Read the enclosing guardian's own ECHILD receipt directly. Never
            // recurse through inferred exit or accept its cached confirmed_exit flag.
            let Ok(bytes) = scope.read_proof(&parent) else {
                continue;
            };
            if bytes.len() != 15 || &bytes[8..] != b"exited\n" {
                continue;
            }
            // Non-redacted, non-PTY isolated launches execute setsid before the
            // workload runs. Combined with the original creator PID, exact boot/start
            // witnesses establish a nested fork tree, not incidental PG membership.
            return Ok(Some(EnclosingScopeExit {
                process_id: parent.process.id,
                launch_scope: scope.clone(),
                previous_ownership_gap: child
                    .enclosing_scope_exit
                    .as_ref()
                    .and_then(|evidence| evidence.previous_ownership_gap.clone())
                    .or_else(|| child.ownership_gap.clone()),
            }));
        }
        Ok(None)
    }

    fn original_isolated_scope(&self, group: &OwnedGroup) -> RefineResult<bool> {
        let Some(scope) = group.launch_scope.as_ref() else {
            return Ok(false);
        };
        if self.runtime_root.canonicalize().ok().as_ref() != Some(&group.runtime_root)
            || group.process.pid.is_none_or(|pid| pid <= 1)
            // Registration precedes release of the workload's setsid handshake;
            // a missing early PGID is expected, a conflicting observed PGID is not.
            || group.pgid.is_some_and(|pgid| Some(pgid) != group.process.pid)
            || ownership_incarnation(&group.process).is_none_or(|id| id.is_empty())
        {
            return Ok(false);
        }
        let path = self
            .processes_dir()
            .join(format!("{}.json", group.process.id));
        let current = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice::<ManagedProcess>(&bytes)
                .map_err(|e| RefineError::Serialization(e.to_string()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match self.inspect_terminal(&group.process.id) {
                    Ok(process) => process,
                    Err(RefineError::NotFound(_)) => return Ok(false),
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(RefineError::Io(e.to_string())),
        };
        Self::ensure_same_registration(&group.process, &current)?;
        for process in [&group.process, &current] {
            let metadata: Value = serde_json::from_str(process.details.as_deref().unwrap_or("{}"))
                .map_err(|e| RefineError::Serialization(e.to_string()))?;
            let recorded = metadata
                .get("launch_scope")
                .cloned()
                .map(serde_json::from_value::<launch_scope::LaunchScope>)
                .transpose()
                .map_err(|e| RefineError::Serialization(e.to_string()))?;
            if recorded.as_ref() != Some(scope)
                || metadata["isolated_process_group"] != true
                || metadata["pty_owned_scope"] == true
                || metadata["command"]
                    .as_str()
                    .is_none_or(|command| command == "redacted" || command.is_empty())
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

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
                enclosing_scope_exit: None,
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

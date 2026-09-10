//! Registration-time group evidence shared by deadline maintenance and worker replacement.
//! Selectively adapted from DR74050DA12C5442DF797B8AB6's retained group termination work.
use super::*;
use std::collections::BTreeMap;

#[cfg(target_os = "linux")]
pub(crate) mod launch_scope;
#[cfg(target_os = "linux")]
mod quiescence;
#[cfg(target_os = "linux")]
mod scope_guardian;
#[cfg(target_os = "linux")]
pub use scope_guardian::run_if_requested as run_scope_guardian_if_requested;
mod assessment;
mod stop;
pub use assessment::OwnershipAssessment;
#[cfg(all(test, target_os = "linux"))]
mod identity_tests;
#[cfg(all(test, target_os = "linux"))]
mod launch_tests;
#[cfg(all(test, target_os = "linux"))]
pub(crate) mod test_fixture;
#[cfg(all(test, target_os = "linux"))]
mod tests;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OwnedGroup {
    pub runtime_root: PathBuf,
    pub process: ManagedProcess,
    pub pgid: Option<u32>,
    pub witnesses: BTreeMap<u32, String>,
    pub confirmed_exit: bool,
    #[serde(default)]
    pub ownership_gap: Option<String>,
    #[cfg(target_os = "linux")]
    #[serde(default)]
    pub launch_scope: Option<launch_scope::LaunchScope>,
}

impl FileProcessSupervisor {
    pub fn requires_group_ownership(process: &ManagedProcess) -> bool {
        process.details.as_deref().is_some_and(requires_ownership)
    }
    pub(super) fn group_path(&self, id: &str) -> PathBuf {
        self.runtime_root
            .join("owned-groups")
            .join(format!("{id}.json"))
    }
    pub(super) fn register_owned_group(&self, process: &ManagedProcess) -> RefineResult<()> {
        if !process.details.as_deref().is_some_and(requires_ownership) {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        let metadata: Value = process
            .details
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Null);
        let pid = process
            .pid
            .ok_or_else(|| RefineError::Degraded("owned process has no PID".into()))?;
        // Preserve even a launch that ended before registration could observe its identity.
        let (token, ownership_gap) = match os_process_identity(pid) {
            Ok(token) => (token, None),
            Err(error) => (
                None,
                Some(format!("registration identity unavailable: {error}")),
            ),
        };
        #[cfg(unix)]
        let pgid = (unsafe { libc::getpgid(pid as i32) } == pid as i32 && pid > 1).then_some(pid);
        #[cfg(not(unix))]
        let pgid = None;
        let group = OwnedGroup {
            runtime_root: self
                .runtime_root
                .canonicalize()
                .map_err(|e| RefineError::Io(e.to_string()))?,
            process: process.clone(),
            pgid,
            witnesses: token
                .map(|token| BTreeMap::from([(pid, token)]))
                .unwrap_or_default(),
            confirmed_exit: false,
            ownership_gap,
            #[cfg(target_os = "linux")]
            launch_scope: metadata
                .get("launch_scope")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| RefineError::Serialization(e.to_string()))?,
        };
        self.write_owned_group(&group)
    }
    fn write_owned_group(&self, group: &OwnedGroup) -> RefineResult<()> {
        let path = self.group_path(&group.process.id);
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| RefineError::Io(e.to_string()))?;
        write_json_atomically(
            &path,
            &serde_json::to_vec(group).map_err(|e| RefineError::Serialization(e.to_string()))?,
            "owned process group",
        )
    }
    pub fn owned_groups(&self) -> RefineResult<Vec<OwnedGroup>> {
        self.owned_group_observations()?.into_iter().collect()
    }
    /// A damaged registration does not suppress observations of other groups. Recovery callers
    /// collect the results fail-closed; maintenance callers report each failed inspection.
    pub fn owned_group_observations(&self) -> RefineResult<Vec<RefineResult<OwnedGroup>>> {
        let entries = match fs::read_dir(self.runtime_root.join("owned-groups")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(RefineError::Io(e.to_string())),
        };
        let mut groups = Vec::new();
        for entry in entries {
            let path = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            groups.push(
                fs::read(&path)
                    .map_err(|e| RefineError::Io(e.to_string()))
                    .and_then(|bytes| {
                        serde_json::from_slice(&bytes).map_err(|e| {
                            RefineError::Serialization(format!("{}: {e}", path.display()))
                        })
                    }),
            );
        }
        Ok(groups)
    }
    /// Retained ownership outlives a leader's primary registry entry. Admission must include
    /// these groups until exit is proved; damaged evidence keeps admission fail-closed.
    pub fn capacity_processes(&self) -> RefineResult<Vec<ManagedProcess>> {
        let mut processes = self
            .list()?
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect::<BTreeMap<_, _>>();
        // Workload-terminal registrations can still own an uncertain scope,
        // including when its group record was lost. Active list() omits them.
        if let Ok(entries) = fs::read_dir(self.processes_dir()) {
            for entry in entries {
                let path = entry.map_err(|e| RefineError::Io(e.to_string()))?.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                let Ok(process) = serde_json::from_slice::<ManagedProcess>(&bytes) else {
                    continue;
                };
                if Self::requires_group_ownership(&process)
                    && self.group_pending(&process).unwrap_or(true)
                {
                    processes.entry(process.id.clone()).or_insert(process);
                }
            }
        }
        for group in self.owned_groups()? {
            if self.runtime_root.canonicalize().ok().as_ref() != Some(&group.runtime_root) {
                return Err(RefineError::Conflict("owned group runtime changed".into()));
            }
            if self.assess_owned_group(&group)?.pending() {
                processes
                    .entry(group.process.id.clone())
                    .or_insert(group.process);
            } else {
                processes.remove(&group.process.id);
            }
        }
        Ok(processes.into_values().collect())
    }
    pub(super) fn owned_group_for_process(
        &self,
        process: &ManagedProcess,
    ) -> RefineResult<OwnedGroup> {
        let bytes = fs::read(self.group_path(&process.id)).map_err(|e| {
            RefineError::Io(format!(
                "owned execution evidence unavailable for {}: {e}; exit unverified",
                process.id
            ))
        })?;
        let group: OwnedGroup = serde_json::from_slice(&bytes)
            .map_err(|e| RefineError::Serialization(e.to_string()))?;
        Self::ensure_same_registration(process, &group.process)?;
        Ok(group)
    }
    pub fn group_pending(&self, process: &ManagedProcess) -> RefineResult<bool> {
        match fs::read(self.group_path(&process.id)) {
            Ok(bytes) => {
                let group: OwnedGroup = serde_json::from_slice(&bytes)
                    .map_err(|e| RefineError::Serialization(e.to_string()))?;
                Self::ensure_same_registration(process, &group.process)?;
                Ok(self.assess_owned_group(&group)?.pending())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if Self::requires_group_ownership(process) {
                    Ok(true)
                } else {
                    Self::process_is_alive(process)
                }
            }
            Err(e) => Err(RefineError::Io(e.to_string())),
        }
    }
    pub fn observe_owned_group(&self, expected: &OwnedGroup) -> RefineResult<OwnedGroup> {
        use crate::infrastructure::process::supervisor::coordination::{
            acquire_record_lock, with_lock_timeout,
        };
        let _lock = with_lock_timeout(Duration::from_millis(200), || {
            acquire_record_lock(
                &self.runtime_root,
                &format!("owned-group:{}", expected.process.id),
            )
        })?;
        // Independent maintenance and replacement can arrive with different snapshots. Merge
        // their witnessed identities under one lock so a stale reader cannot erase an escaped
        // descendant observed by the other lane after the original parent exits.
        let bytes = fs::read(self.group_path(&expected.process.id))
            .map_err(|e| RefineError::Io(e.to_string()))?;
        let latest: OwnedGroup = serde_json::from_slice(&bytes)
            .map_err(|e| RefineError::Serialization(e.to_string()))?;
        if latest.process.id != expected.process.id
            || latest.process.pid != expected.process.pid
            || latest.process.started_at != expected.process.started_at
            || latest.process.owner != expected.process.owner
            || latest.pgid != expected.pgid
            || latest.runtime_root != expected.runtime_root
            || ownership_incarnation(&latest.process) != ownership_incarnation(&expected.process)
        {
            return Err(RefineError::Conflict(
                "owned group snapshot identity changed".into(),
            ));
        }
        if expected.witnesses.iter().any(|(pid, token)| {
            latest
                .witnesses
                .get(pid)
                .is_some_and(|current| current != token)
        }) {
            return Err(RefineError::Conflict(
                "owned group witness identity changed".into(),
            ));
        }
        let mut expected = expected.clone();
        expected.witnesses.extend(latest.witnesses.clone());
        expected.ownership_gap = latest.ownership_gap.clone().or(expected.ownership_gap);
        #[cfg(target_os = "linux")]
        if expected.launch_scope != latest.launch_scope {
            return Err(RefineError::Conflict(
                "owned group launch scope changed".into(),
            ));
        }
        if self.runtime_root.canonicalize().ok().as_ref() != Some(&expected.runtime_root) {
            return Err(RefineError::Conflict("owned group runtime changed".into()));
        }
        // Ownership inspection must not invoke registry retirement while the reaper holds
        // its cleanup lock. Read the exact registration without archive side effects.
        let current = match fs::read(
            self.processes_dir()
                .join(format!("{}.json", expected.process.id)),
        ) {
            Ok(bytes) => Some(
                serde_json::from_slice::<ManagedProcess>(&bytes)
                    .map_err(|e| RefineError::Serialization(e.to_string()))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(RefineError::Io(e.to_string())),
        };
        if let Some(current) = current
            && (current.pid != expected.process.pid
                || current.started_at != expected.process.started_at
                || current.owner != expected.process.owner
                || ownership_incarnation(&current) != ownership_incarnation(&expected.process))
        {
            return Err(RefineError::Conflict(
                "owned group registration was replaced".into(),
            ));
        }
        let (assessment, members) = self.assess_group_members(&expected)?;
        let mut group = expected.clone();
        group.confirmed_exit = matches!(assessment, OwnershipAssessment::Exited);
        if let OwnershipAssessment::Unverified { reason } = assessment {
            group.ownership_gap = Some(reason);
        }
        // Retain every identity witness: disappearance cannot erase an earlier coverage gap.
        group.witnesses.extend(members);
        if group.witnesses != latest.witnesses
            || group.ownership_gap != latest.ownership_gap
            || group.confirmed_exit != latest.confirmed_exit
        {
            self.write_owned_group(&group)?;
        }
        Ok(group)
    }
}

#[cfg(target_os = "linux")]
fn group_members(group: &OwnedGroup) -> RefineResult<BTreeMap<u32, String>> {
    let pgid = group
        .pgid
        .or_else(|| group.launch_scope.as_ref().and(group.process.pid))
        .ok_or_else(|| {
            RefineError::Degraded(
                "isolated process group evidence unavailable; exit cannot be proved".into(),
            )
        })?;
    let mut tree = BTreeMap::new();
    for entry in fs::read_dir("/proc").map_err(|e| RefineError::Io(e.to_string()))? {
        let entry = entry.map_err(|e| RefineError::Io(e.to_string()))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let stat = match fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => stat,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(RefineError::Io(e.to_string())),
        };
        let fields = stat
            .rsplit_once(')')
            .ok_or_else(|| RefineError::Serialization("invalid process stat".into()))?
            .1
            .split_whitespace()
            .collect::<Vec<_>>();
        if fields.first().is_some_and(|s| matches!(*s, "Z" | "X")) {
            continue;
        }
        let parent = fields
            .get(1)
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| RefineError::Serialization("missing process parent".into()))?;
        let process_group = fields
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or_else(|| RefineError::Serialization("missing process group".into()))?;
        tree.insert(pid, (parent, process_group));
    }
    let mut selected = BTreeSet::new();
    if let Some(scope) = &group.launch_scope
        && scope.alive()?
    {
        selected.insert(scope.guardian_pid);
    }
    let mut group_present = false;
    let mut group_witnessed = false;
    for (pid, (_, process_group)) in &tree {
        let witnessed = match group.witnesses.get(pid) {
            Some(token) => os_process_identity(*pid)?.as_ref() == Some(token),
            None => false,
        };
        if *process_group == pgid {
            group_present = true;
            group_witnessed |= witnessed;
        }
        if *process_group == pgid || witnessed {
            selected.insert(*pid);
        }
    }
    if group_present
        && !group_witnessed
        && let Some(scope) = &group.launch_scope
    {
        if scope.alive()? {
            group_witnessed = tree
                .iter()
                .filter(|(_, (_, pg))| *pg == pgid)
                .all(|(pid, _)| {
                    let mut cursor = *pid;
                    let mut seen = BTreeSet::new();
                    while seen.insert(cursor) {
                        if cursor == scope.guardian_pid {
                            return true;
                        }
                        let Some((parent, _)) = tree.get(&cursor) else {
                            return false;
                        };
                        cursor = *parent;
                    }
                    false
                });
        }
    }
    if group_present && !group_witnessed {
        return Err(RefineError::Conflict(
            "owned process group identity is unverified; an escaped witness cannot authorize a reused group ID".into(),
        ));
    }
    loop {
        let children = tree
            .iter()
            .filter(|(pid, (parent, _))| !selected.contains(pid) && selected.contains(parent))
            .map(|(pid, _)| *pid)
            .collect::<Vec<_>>();
        if children.is_empty() {
            break;
        }
        selected.extend(children);
    }
    let mut members = BTreeMap::new();
    for pid in selected {
        if group
            .launch_scope
            .as_ref()
            .is_some_and(|s| s.guardian_pid == pid)
            || is_scope_guardian(group, pid)
        {
            continue;
        }
        if let Some(token) = os_process_identity(pid)? {
            members.insert(pid, token);
        }
    }
    Ok(members)
}
#[cfg(not(target_os = "linux"))]
fn group_members(_group: &OwnedGroup) -> RefineResult<BTreeMap<u32, String>> {
    Err(RefineError::Degraded(
        "group exit inspection is unavailable on this platform; retain ownership evidence".into(),
    ))
}

fn requires_ownership(details: &str) -> bool {
    serde_json::from_str::<Value>(details)
        .ok()
        .is_some_and(|value| {
            !value["workflow_incarnation"].is_null() || !value["agent_hard_cap_millis"].is_null()
        })
}
#[cfg(target_os = "linux")]
fn is_scope_guardian(group: &OwnedGroup, pid: u32) -> bool {
    let root = if group
        .runtime_root
        .file_name()
        .is_some_and(|n| n == "agents")
    {
        group.runtime_root.parent().unwrap_or(&group.runtime_root)
    } else {
        &group.runtime_root
    };
    [root.to_path_buf(), root.join("agents")]
        .into_iter()
        .any(|root| {
            FileProcessSupervisor::new(root)
                .owned_groups()
                .is_ok_and(|groups| {
                    groups.into_iter().any(|g| {
                        g.launch_scope
                            .is_some_and(|s| s.guardian_pid == pid && s.alive().unwrap_or(false))
                    })
                })
        })
}

fn ownership_incarnation(process: &ManagedProcess) -> Option<String> {
    process
        .details
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v["workflow_incarnation"].as_str().map(str::to_string))
}

#[cfg(all(test, not(target_os = "linux")))]
mod unsupported_tests {
    use super::*;
    #[test]
    fn unsupported_scope_proof_retains_capacity_and_reports_uncertainty() {
        let root =
            std::env::temp_dir().join(format!("refine-unavailable-scope-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let supervisor = FileProcessSupervisor::new(&root);
        let group = OwnedGroup {
            runtime_root: root.canonicalize().unwrap(),
            process: ManagedProcess {
                id: "unverified".into(),
                owner: ProcessOwner::Agent,
                pid: Some(999999),
                state: "running".into(),
                label: None,
                details: Some(json!({"workflow_incarnation":"unsupported"}).to_string()),
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: "0".into(),
                exit_code: None,
            },
            pgid: None,
            witnesses: BTreeMap::new(),
            confirmed_exit: true,
            ownership_gap: None,
        };
        supervisor.write_owned_group(&group).unwrap();
        assert!(
            matches!(supervisor.assess_owned_group(&group).unwrap(),OwnershipAssessment::Unverified{reason} if reason.contains("unavailable on this platform"))
        );
        assert!(supervisor.group_pending(&group.process).unwrap());
        assert_eq!(supervisor.capacity_processes().unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}

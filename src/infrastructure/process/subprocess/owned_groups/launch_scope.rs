//! Parent side of the bounded launch handshake. No workload runs before registration.
use super::scope_guardian::{Request, helper_command, receive, send};
use super::*;
use std::os::unix::{fs::MetadataExt, net::UnixStream, process::ExitStatusExt};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LaunchScope {
    pub guardian_pid: u32,
    pub guardian_identity: Option<String>,
    pub proof_path: PathBuf,
    pub proof_device: u64,
    pub proof_inode: u64,
}

pub(crate) struct ScopeLaunch {
    connection: UnixStream,
    listener: Option<std::os::unix::net::UnixListener>,
    inherited: Option<UnixStream>,
    request: Request,
    scope: LaunchScope,
}
impl ScopeLaunch {
    /// Called before stdio is configured. The helper inherits the exact prepared environment.
    pub(crate) fn prepare(
        supervisor: &FileProcessSupervisor,
        id: &str,
        spec: &ManagedProcessSpec,
        command: &mut Command,
    ) -> RefineResult<Option<Self>> {
        if !requires_ownership(&process_details(spec)) {
            return Ok(None);
        }
        let dir = supervisor.runtime_root.join("owned-scopes");
        fs::create_dir_all(&dir).map_err(io_error)?;
        let path = dir
            .canonicalize()
            .map_err(io_error)?
            .join(format!("{id}-{}.exit", uuid::Uuid::new_v4()));
        let proof = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&path)
            .map_err(io_error)?;
        let metadata = proof.metadata().map_err(io_error)?;
        let (connection, inherited) = UnixStream::pair().map_err(io_error)?;
        connection
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(io_error)?;
        connection
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(io_error)?;
        let mut helper = helper_command("guardian", &inherited)?;
        helper.env_clear();
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                helper.env(key, value);
            }
        }
        #[cfg(test)]
        helper.env("REFINE_SCOPE_BOOTSTRAP", "guardian");
        if let Some(cwd) = command.get_current_dir() {
            helper.current_dir(cwd);
        }
        *command = helper;
        let mut workload = spec.clone();
        // Input and environment stay in their existing streams, never in the proof file.
        workload.stdin = None;
        workload.env.clear();
        Ok(Some(Self {
            connection,
            listener: None,
            inherited: Some(inherited),
            request: Request {
                workload,
                parent_pid: std::process::id(),
                proof_path: path.clone(),
            },
            scope: LaunchScope {
                guardian_pid: 0,
                guardian_identity: None,
                proof_path: path,
                proof_device: metadata.dev(),
                proof_inode: metadata.ino(),
            },
        }))
    }
    /// PTYs cannot inherit arbitrary control descriptors. Use a kernel-local socket
    /// and verify its peer PID before the same registration handshake.
    pub(crate) fn prepare_pty(
        supervisor: &FileProcessSupervisor,
        id: &str,
        spec: &ManagedProcessSpec,
        command: &mut portable_pty::CommandBuilder,
    ) -> RefineResult<Option<Self>> {
        use std::os::linux::net::SocketAddrExt;
        let mut prepared = Command::new(&spec.command);
        let Some(mut scope) = Self::prepare(supervisor, id, spec, &mut prepared)? else {
            return Ok(None);
        };
        let token = format!("refine-scope-{}", uuid::Uuid::new_v4());
        let address = std::os::unix::net::SocketAddr::from_abstract_name(token.as_bytes())
            .map_err(io_error)?;
        let listener = std::os::unix::net::UnixListener::bind_addr(&address).map_err(io_error)?;
        listener.set_nonblocking(true).map_err(io_error)?;
        scope.listener = Some(listener);
        scope
            .request
            .workload
            .metadata
            .insert("pty_owned_scope".into(), json!(true));
        *command.get_argv_mut() = vec![
            PathBuf::from("/proc/self/exe").into_os_string(),
            "--refine-owned-scope".into(),
            "guardian-socket".into(),
            token.clone().into(),
        ];
        #[cfg(test)]
        {
            command.env("REFINE_SCOPE_BOOTSTRAP", "guardian-socket");
            command.env("REFINE_SCOPE_SOCKET", &token);
        }
        Ok(Some(scope))
    }
    pub(crate) fn attach(
        &mut self,
        child: &std::process::Child,
        details: &mut String,
    ) -> RefineResult<u32> {
        self.attach_pid(child.id(), details)
    }
    pub(crate) fn attach_pid(
        &mut self,
        helper_pid: u32,
        details: &mut String,
    ) -> RefineResult<u32> {
        if let Some(listener) = self.listener.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match listener.accept() {
                    Ok((connection, _)) => {
                        use std::os::fd::AsRawFd;
                        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
                        let mut size = std::mem::size_of_val(&credentials) as libc::socklen_t;
                        if unsafe {
                            libc::getsockopt(
                                connection.as_raw_fd(),
                                libc::SOL_SOCKET,
                                libc::SO_PEERCRED,
                                (&mut credentials as *mut libc::ucred).cast(),
                                &mut size,
                            )
                        } != 0
                            || credentials.pid != helper_pid as i32
                        {
                            return Err(RefineError::Conflict(
                                "ownership helper connection identity changed".into(),
                            ));
                        }
                        connection
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .map_err(io_error)?;
                        connection
                            .set_write_timeout(Some(Duration::from_secs(5)))
                            .map_err(io_error)?;
                        self.connection = connection;
                        break;
                    }
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => return Err(io_error(e)),
                }
            }
        }
        self.inherited.take();
        send(&mut self.connection, &self.request)?;
        let pid: u32 = receive(&mut self.connection)?;
        self.scope.guardian_pid = helper_pid;
        self.scope.guardian_identity = os_process_identity(helper_pid)?;
        if self.scope.guardian_identity.is_none() || os_process_identity(pid)?.is_none() {
            return Err(RefineError::Degraded(
                "launch handshake lost process identity".into(),
            ));
        }
        let mut value: Value =
            serde_json::from_str(details).map_err(|e| RefineError::Serialization(e.to_string()))?;
        value["launch_scope"] = serde_json::to_value(&self.scope)
            .map_err(|e| RefineError::Serialization(e.to_string()))?;
        *details = value.to_string();
        Ok(pid)
    }
    pub(crate) fn release(&mut self) -> RefineResult<()> {
        self.connection.write_all(b"S").map_err(io_error)
    }
}
pub(super) fn io_error(error: std::io::Error) -> RefineError {
    RefineError::Io(format!("ownership launch handshake: {error}"))
}
impl LaunchScope {
    fn read_proof(&self, group: &OwnedGroup) -> RefineResult<Vec<u8>> {
        if self.guardian_identity.is_none()
            || self.proof_path.parent() != Some(group.runtime_root.join("owned-scopes").as_path())
            || !self
                .proof_path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with(&format!("{}-", group.process.id)))
        {
            return Err(RefineError::Conflict(
                "ownership scope runtime or registration changed".into(),
            ));
        }
        let file = fs::File::open(&self.proof_path).map_err(io_error)?;
        let metadata = file.metadata().map_err(io_error)?;
        if metadata.dev() != self.proof_device || metadata.ino() != self.proof_inode {
            return Err(RefineError::Conflict(
                "ownership proof file identity changed".into(),
            ));
        }
        let mut bytes = Vec::new();
        file.take(32).read_to_end(&mut bytes).map_err(io_error)?;
        if bytes.len() >= 4
            && Some(u32::from_ne_bytes(bytes[..4].try_into().unwrap())) != group.process.pid
        {
            return Err(RefineError::Conflict(
                "ownership workload identity changed".into(),
            ));
        }
        Ok(bytes)
    }
    pub(super) fn proof(&self, group: &OwnedGroup) -> RefineResult<bool> {
        let bytes = self.read_proof(group)?;
        Ok(bytes.len() == 15 && &bytes[8..] == b"exited\n")
    }
    pub(super) fn alive(&self) -> RefineResult<bool> {
        Ok(self.guardian_identity.is_some()
            && os_process_identity(self.guardian_pid)? == self.guardian_identity)
    }
    pub(crate) fn workload_status(
        &self,
        process: &ManagedProcess,
        runtime: &Path,
    ) -> RefineResult<Option<std::process::ExitStatus>> {
        let group = OwnedGroup {
            runtime_root: runtime.canonicalize().map_err(io_error)?,
            process: process.clone(),
            pgid: process.pid,
            witnesses: BTreeMap::new(),
            confirmed_exit: false,
            ownership_gap: None,
            launch_scope: Some(self.clone()),
        };
        let bytes = self.read_proof(&group)?;
        Ok((bytes.len() >= 8).then(|| {
            std::process::ExitStatus::from_raw(i32::from_ne_bytes(bytes[4..8].try_into().unwrap()))
        }))
    }
}

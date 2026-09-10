//! Deterministic parent-exit fixture shared by the real ownership consumers.
use super::*;
use std::os::unix::process::CommandExt;

pub(crate) struct UnobservedChild {
    pub group: OwnedGroup,
    pub child_pid: u32,
    child_identity: String,
}
impl UnobservedChild {
    pub fn launch(supervisor: &FileProcessSupervisor, tracked: bool, mut metadata: Value) -> Self {
        fs::create_dir_all(&supervisor.runtime_root).unwrap();
        let gate = supervisor
            .runtime_root
            .join(format!("gate-{}", uuid::Uuid::new_v4()));
        let pid_path = gate.with_extension("child");
        let script = "while [ ! -e \"$1\" ]; do :; done; setsid sh -c 'echo $$ > \"$1\"; exec sleep 60' sh \"$2\" & while [ ! -s \"$2\" ]; do :; done; exit 0";
        metadata["isolated_process_group"] = json!(true);
        let mut raw = None;
        let process = if tracked {
            supervisor
                .launch(ManagedProcessSpec {
                    owner: ProcessOwner::Agent,
                    command: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        script.into(),
                        "sh".into(),
                        gate.display().to_string(),
                        pid_path.display().to_string(),
                    ],
                    cwd: None,
                    env: Vec::new(),
                    stdin: None,
                    limits: None,
                    authorization_command: None,
                    sensitive: false,
                    metadata: serde_json::from_value(metadata).unwrap(),
                })
                .unwrap()
        } else {
            let child = Command::new("/bin/sh")
                .args(["-c", script, "sh"])
                .arg(&gate)
                .arg(&pid_path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap();
            let process = supervisor
                .register(ManagedProcess {
                    id: uuid::Uuid::new_v4().to_string(),
                    owner: ProcessOwner::Agent,
                    pid: Some(child.id()),
                    state: "running".into(),
                    label: Some("unobserved leader".into()),
                    details: Some(metadata.to_string()),
                    stdout_path: None,
                    stderr_path: None,
                    stdin_path: None,
                    limits: None,
                    started_at: chrono::Utc::now().timestamp_millis().to_string(),
                    exit_code: None,
                })
                .unwrap();
            raw = Some(child);
            process
        };
        // Register before opening the gate. No descendant observation or witness seeding.
        let group = supervisor
            .owned_groups()
            .unwrap()
            .into_iter()
            .find(|g| g.process.id == process.id)
            .unwrap();
        assert_eq!(group.witnesses.len(), 1);
        assert!(group.witnesses.contains_key(&process.pid.unwrap()));
        fs::write(&gate, "launch child").unwrap();
        if let Some(child) = raw.as_mut() {
            assert!(child.wait().unwrap().success());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Path::new(&format!("/proc/{}", process.pid.unwrap())).exists() {
            assert!(
                Instant::now() < deadline,
                "leader was not reaped before observation"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let child_pid = fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let child_identity = os_process_identity(child_pid)
            .unwrap()
            .expect("escaped child must still be alive");
        Self {
            group,
            child_pid,
            child_identity,
        }
    }
    pub fn child_alive(&self) -> bool {
        os_process_identity(self.child_pid).unwrap().as_ref() == Some(&self.child_identity)
    }
    pub fn kill_child(&self) {
        if self.child_alive() {
            unsafe {
                libc::kill(self.child_pid as i32, libc::SIGKILL);
            }
        }
    }
}
impl Drop for UnobservedChild {
    fn drop(&mut self) {
        self.kill_child();
    }
}

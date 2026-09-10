use super::*;
use std::os::unix::process::CommandExt;

type SignalHook = Box<dyn FnOnce() + Send>;
static SIGNAL_HOOKS: std::sync::OnceLock<Mutex<BTreeMap<String, SignalHook>>> =
    std::sync::OnceLock::new();
pub(super) fn before_signal(id: &str) {
    let hook = SIGNAL_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .remove(id);
    if let Some(hook) = hook {
        hook();
    }
}
pub(super) fn after_suspend(id: &str) {
    before_signal(&format!("suspend:{id}"));
}

struct IsolatedChild(std::process::Child);
impl IsolatedChild {
    fn new() -> Self {
        Self(
            Command::new("/bin/sleep")
                .arg("60")
                .process_group(0)
                .spawn()
                .unwrap(),
        )
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
}
impl Drop for IsolatedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn register(supervisor: &FileProcessSupervisor, pid: u32) -> OwnedGroup {
    let process = ManagedProcess {
        id: uuid::Uuid::new_v4().to_string(),
        owner: ProcessOwner::Agent,
        pid: Some(pid),
        state: "running".into(),
        label: None,
        details: Some(json!({"workflow_incarnation":"test-reuse"}).to_string()),
        stdout_path: None,
        stderr_path: None,
        stdin_path: None,
        limits: None,
        started_at: chrono::Utc::now().timestamp_millis().to_string(),
        exit_code: None,
    };
    supervisor.register(process).unwrap();
    supervisor.owned_groups().unwrap().remove(0)
}

#[test]
fn an_isolated_child_launched_during_stop_must_not_escape_exit_confirmation() {
    let root =
        std::env::temp_dir().join(format!("refine-group-fork-race-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let gate = root.join("fork");
    let child_path = root.join("child-pid");
    let supervisor = FileProcessSupervisor::new(&root);
    let group = launch_group(
        &supervisor,
        "/bin/sh",
        &[
            "-c",
            "while [ ! -e \"$1\" ]; do :; done; setsid sh -c 'echo $$ > \"$1\"; exec sleep 60' sh \"$2\" & wait",
            "sh",
            gate.to_str().unwrap(),
            child_path.to_str().unwrap(),
        ],
    );
    let child_ready = child_path.clone();
    SIGNAL_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(
            group.process.id.clone(),
            Box::new(move || {
                fs::write(gate, "fork now").unwrap();
                let deadline = Instant::now() + Duration::from_secs(2);
                while fs::read_to_string(&child_ready)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .is_none()
                {
                    assert!(Instant::now() < deadline, "isolated child did not launch");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }),
        );
    let result = supervisor.stop_owned_group(&group, Duration::from_secs(2));
    let child_pid = fs::read_to_string(child_path)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let child_alive = os_process_identity(child_pid).unwrap().is_some();
    if child_alive {
        unsafe {
            libc::kill(child_pid as i32, libc::SIGKILL);
        }
    }
    fs::remove_dir_all(root).unwrap();
    assert!(
        result.as_ref().is_ok_and(|g| g.confirmed_exit) && !child_alive,
        "exit was confirmed while an isolated child launched after the scan survived: {result:?}"
    );
}

#[test]
fn a_quiescence_inspection_failure_resumes_survivors_and_retains_evidence() {
    let root = std::env::temp_dir().join(format!(
        "refine-group-quiescence-fault-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let supervisor = FileProcessSupervisor::new(&root);
    let group = launch_group(&supervisor, "/bin/sleep", &["60"]);
    let path = supervisor.group_path(&group.process.id);
    let damaged_path = path.clone();
    let pid = group.process.pid.unwrap();
    let stopped = move || {
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .unwrap()
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .next()
            == Some("T")
    };
    SIGNAL_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(
            format!("suspend:{}", group.process.id),
            Box::new(move || {
                let deadline = Instant::now() + Duration::from_secs(1);
                while !stopped() {
                    assert!(Instant::now() < deadline, "fixture was not suspended");
                    std::thread::sleep(Duration::from_millis(5));
                }
                fs::write(damaged_path, "{damaged").unwrap();
            }),
        );
    let result = supervisor.stop_owned_group(&group, Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(1);
    while stopped() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let resumed = !stopped();
    let retained = fs::read_to_string(path).unwrap();
    supervisor
        .request_termination(&group.process.id, "kill")
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        result.is_err() && resumed,
        "failed stop left a suspended survivor"
    );
    assert_eq!(retained, "{damaged");
}

#[test]
fn concurrent_stops_defer_while_an_owned_tree_is_being_quiesced() {
    let root =
        std::env::temp_dir().join(format!("refine-group-stop-fence-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let supervisor = FileProcessSupervisor::new(&root);
    let group = launch_group(&supervisor, "/bin/sleep", &["60"]);
    // Worker recovery and deadline maintenance can reach an overlapping tree through
    // separate port and Agent registries; their stop fence must still be shared.
    let agent_supervisor = FileProcessSupervisor::new(root.join("agents"));
    let agent_group = register(&agent_supervisor, group.process.pid.unwrap());
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    SIGNAL_HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(
            group.process.id.clone(),
            Box::new(move || {
                ready_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }),
        );
    let first_supervisor = supervisor.clone();
    let first_group = group.clone();
    let first = std::thread::spawn(move || {
        first_supervisor.stop_owned_group(&first_group, Duration::from_secs(2))
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let second = agent_supervisor.stop_owned_group(&agent_group, Duration::from_millis(50));
    let still_alive = os_process_identity(group.process.pid.unwrap())
        .unwrap()
        .is_some();
    release_tx.send(()).unwrap();
    let first_result = first.join().unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        second.is_err() && still_alive,
        "a competing stop crossed the tree suspension boundary"
    );
    assert!(first_result.unwrap().confirmed_exit);
}

#[test]
fn reused_group_id_cannot_borrow_identity_from_an_escaped_descendant() {
    let root = std::env::temp_dir().join(format!("refine-group-reuse-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let supervisor = FileProcessSupervisor::new(&root);
    let mut leader = IsolatedChild::new();
    let mut escaped = IsolatedChild::new();
    let mut foreign = IsolatedChild::new();
    let process = ManagedProcess {
        id: uuid::Uuid::new_v4().to_string(),
        owner: ProcessOwner::Agent,
        pid: Some(leader.pid()),
        state: "running".into(),
        label: None,
        details: Some(json!({"workflow_incarnation":"test-reuse"}).to_string()),
        stdout_path: None,
        stderr_path: None,
        stdin_path: None,
        limits: None,
        started_at: chrono::Utc::now().timestamp_millis().to_string(),
        exit_code: None,
    };
    supervisor.register(process).unwrap();
    let mut group = supervisor.owned_groups().unwrap().remove(0);
    group.witnesses.insert(
        escaped.pid(),
        os_process_identity(escaped.pid()).unwrap().unwrap(),
    );
    leader.0.kill().unwrap();
    leader.0.wait().unwrap();
    // Simulate reuse of the departed group's numeric ID without waiting for the host's PID
    // allocator. The escaped child's original identity is still valid outside that group.
    group.pgid = Some(foreign.pid());
    supervisor.write_owned_group(&group).unwrap();
    let result = supervisor.observe_owned_group(&group);
    let stopped = supervisor.stop_owned_group(&group, Duration::from_millis(100));
    let escaped_alive = escaped.0.try_wait().unwrap().is_none();
    let foreign_alive = foreign.0.try_wait().unwrap().is_none();
    fs::remove_dir_all(root).unwrap();
    assert!(
        result.is_err(),
        "unrelated group accepted using an escaped witness: {result:?}"
    );
    assert!(
        stopped.is_err() && escaped_alive && foreign_alive,
        "ambiguous ownership must preserve both groups without signalling"
    );
}

fn launch_group(supervisor: &FileProcessSupervisor, command: &str, args: &[&str]) -> OwnedGroup {
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(
                json!({"workflow_incarnation":"test-owned-scope", "isolated_process_group":true}),
            )
            .unwrap(),
        })
        .unwrap();
    supervisor
        .owned_groups()
        .unwrap()
        .into_iter()
        .find(|g| g.process.id == process.id)
        .unwrap()
}

#[test]
fn an_unobserved_reaped_leader_requires_lifetime_proof_even_after_later_empty_scans() {
    use super::test_fixture::UnobservedChild;
    for tracked in [false, true] {
        for remove_primary in [false, true] {
            let root =
                std::env::temp_dir().join(format!("refine-unobserved-{}", uuid::Uuid::new_v4()));
            let supervisor = FileProcessSupervisor::new(&root);
            let fixture = UnobservedChild::launch(
                &supervisor,
                tracked,
                json!({"workflow_incarnation":"unobserved", "goal_id":"GOAL1"}),
            );
            let process = &fixture.group.process;
            assert!(fixture.child_alive());
            assert!(!fixture.group.witnesses.contains_key(&fixture.child_pid));
            if remove_primary {
                fs::remove_file(
                    supervisor
                        .processes_dir()
                        .join(format!("{}.json", process.id)),
                )
                .unwrap();
            }
            assert!(supervisor.group_pending(process).unwrap());
            assert_eq!(
                supervisor
                    .capacity_processes()
                    .unwrap()
                    .iter()
                    .filter(|p| p.id == process.id)
                    .count(),
                1
            );
            let observed = supervisor.observe_owned_group(&fixture.group).unwrap();
            assert!(!observed.confirmed_exit);
            if tracked {
                assert!(observed.witnesses.contains_key(&fixture.child_pid));
                assert_eq!(
                    supervisor.assess_owned_group(&observed).unwrap(),
                    OwnershipAssessment::Live
                );
                assert!(
                    supervisor
                        .stop_owned_group(&observed, Duration::from_secs(2))
                        .unwrap()
                        .confirmed_exit
                );
                assert!(!fixture.child_alive());
                assert!(!supervisor.group_pending(process).unwrap());
            } else {
                assert!(matches!(
                    supervisor.assess_owned_group(&observed).unwrap(),
                    OwnershipAssessment::Unverified { .. }
                ));
                assert!(
                    supervisor
                        .stop_owned_group(&observed, Duration::from_millis(100))
                        .is_err()
                );
                assert!(
                    fixture.child_alive(),
                    "unwitnessed child must remain independently alive"
                );
                fixture.kill_child();
                assert!(
                    supervisor.group_pending(process).unwrap(),
                    "an empty scan cannot repair missing lifetime coverage"
                );
                assert!(
                    !supervisor
                        .observe_owned_group(&fixture.group)
                        .unwrap()
                        .confirmed_exit
                );
            }
            drop(fixture);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn a_lost_guardian_or_legacy_exit_boolean_cannot_authorize_exit() {
    use super::test_fixture::UnobservedChild;
    let root = std::env::temp_dir().join(format!("refine-lost-guardian-{}", uuid::Uuid::new_v4()));
    let supervisor = FileProcessSupervisor::new(&root);
    let fixture = UnobservedChild::launch(
        &supervisor,
        true,
        json!({"workflow_incarnation":"lost-guardian"}),
    );
    let mut group = fixture.group.clone();
    let guard = group.launch_scope.as_ref().unwrap();
    unsafe {
        libc::kill(guard.guardian_pid as i32, libc::SIGKILL);
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while guard.alive().unwrap() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(fixture.child_alive());
    let observed = supervisor.observe_owned_group(&group).unwrap();
    assert!(
        observed
            .ownership_gap
            .as_deref()
            .unwrap()
            .contains("guardian disappeared")
    );
    fixture.kill_child();
    assert!(supervisor.group_pending(&group.process).unwrap());
    group.launch_scope = None;
    group.confirmed_exit = true;
    supervisor.write_owned_group(&group).unwrap();
    assert!(
        supervisor.group_pending(&group.process).unwrap(),
        "legacy empty-scan boolean is not exit proof"
    );
    assert!(
        !supervisor
            .observe_owned_group(&group)
            .unwrap()
            .confirmed_exit
    );
    drop(fixture);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn registration_keeps_evidence_when_the_leader_already_exited() {
    let root =
        std::env::temp_dir().join(format!("refine-registration-exit-{}", uuid::Uuid::new_v4()));
    let supervisor = FileProcessSupervisor::new(&root);
    let mut child = IsolatedChild::new();
    let pid = child.pid();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let group = register(&supervisor, pid);
    assert!(group.witnesses.is_empty());
    assert!(!group.confirmed_exit);
    assert!(supervisor.group_pending(&group.process).unwrap());
    assert!(supervisor.group_path(&group.process.id).exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn completion_preserves_workload_output_status_and_fast_exit_lifetime_proof() {
    let root =
        std::env::temp_dir().join(format!("refine-scope-completion-{}", uuid::Uuid::new_v4()));
    let supervisor = FileProcessSupervisor::new(&root);
    for status in [0, 7] {
        let output = supervisor
            .run_to_completion(ManagedProcessSpec {
                owner: ProcessOwner::Agent,
                command: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    format!("printf stdout; printf stderr >&2; exit {status}"),
                ],
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: None,
                authorization_command: None,
                sensitive: false,
                metadata: serde_json::from_value(json!({"workflow_incarnation":"fast-exit"}))
                    .unwrap(),
            })
            .unwrap();
        assert_eq!(output.stdout, "stdout");
        assert_eq!(output.stderr, "stderr");
        assert_eq!(output.process.exit_code, Some(status));
        let group = supervisor
            .owned_groups()
            .unwrap()
            .into_iter()
            .find(|g| g.process.id == output.process.id)
            .unwrap();
        assert_eq!(
            supervisor.assess_owned_group(&group).unwrap(),
            OwnershipAssessment::Exited
        );
        assert!(!supervisor.group_pending(&output.process).unwrap());
    }
    fs::remove_dir_all(root).unwrap();
}

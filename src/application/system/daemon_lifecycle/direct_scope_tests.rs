use super::*;
use crate::application::system::daemon_lifecycle::managed::{
    run_service_managed_daemon_with, stop_service_managed_daemon_with,
};
use crate::application::system::installation::InstalledServiceAction;
use crate::infrastructure::process::subprocess::owned_groups::OwnedGroup;
use crate::infrastructure::process::subprocess::scheduler_observation::current_os_identity;
use crate::infrastructure::process::subprocess::{
    ManagedProcess, ManagedProcessSpec, ProcessResourceLimits,
};
use crate::infrastructure::process::supervisor::runtime::RuntimeRoot;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

const FIXTURE_ROOT: &str = "REFINE_TEST_NESTED_SHUTDOWN_ROOT";
const FIXTURE_TEST: &str =
    "application::system::daemon_lifecycle::direct::scope_tests::nested_scope_launcher_fixture";
const PORT: u16 = 4557;

fn spec(owner: ProcessOwner, command: &str, args: Vec<String>) -> ManagedProcessSpec {
    ManagedProcessSpec {
        owner,
        command: command.into(),
        args,
        cwd: None,
        env: Vec::new(),
        stdin: None,
        limits: Some(ProcessResourceLimits {
            kill_on_parent_exit: true,
            ..Default::default()
        }),
        authorization_command: None,
        sensitive: false,
        metadata: serde_json::from_value(json!({
            "workflow_incarnation": "nested-shutdown",
            "isolated_process_group": true
        }))
        .unwrap(),
    }
}

#[test]
fn nested_scope_launcher_fixture() {
    let Ok(root) = std::env::var(FIXTURE_ROOT) else {
        return;
    };
    let root = PathBuf::from(root);
    let supervisor = FileProcessSupervisor::new(root.join("agents"));
    let mut agent = spec(ProcessOwner::Agent, "/bin/sleep", vec!["60".into()]);
    agent.metadata.insert(
        "test_scope_exit_gate".into(),
        json!(root.join("allow-agent-proof")),
    );
    let agent = supervisor.launch(agent).unwrap();
    fs::write(root.join("agent.json"), serde_json::to_vec(&agent).unwrap()).unwrap();
    loop {
        if !FileProcessSupervisor::process_is_alive(&agent).unwrap() {
            // Models result settlement by a worker left running after its child
            // was killed. Restart must freeze this writer with the child workload.
            fs::write(root.join("parent-observed-agent-exit"), "settled failure").unwrap();
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

struct Fixture {
    root: PathBuf,
    lifecycle: FileDaemonLifecycleService,
    supervisor: FileProcessSupervisor,
    worker: ManagedProcess,
    daemon: Child,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("refine-nested-stop-{}", uuid::Uuid::new_v4()));
        let lifecycle = FileDaemonLifecycleService::new(RuntimeRoot { root: root.clone() });
        let runtime = lifecycle.runtime_root.port_root(PORT);
        fs::create_dir_all(&runtime).unwrap();
        let supervisor = FileProcessSupervisor::new(&runtime);
        let mut worker = spec(
            ProcessOwner::Runner,
            std::env::current_exe().unwrap().to_str().unwrap(),
            vec!["--exact".into(), FIXTURE_TEST.into(), "--nocapture".into()],
        );
        worker
            .env
            .push((FIXTURE_ROOT.into(), runtime.display().to_string()));
        let worker = supervisor.launch(worker).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !runtime.join("agent.json").exists() {
            assert!(Instant::now() < deadline, "nested Agent did not register");
            thread::sleep(Duration::from_millis(5));
        }
        // This real daemon stand-in is registered without a background reaper;
        // the fixture retains its Child handle to join teardown deterministically.
        let daemon = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        supervisor
            .register(ManagedProcess {
                id: "nested-daemon".into(),
                owner: ProcessOwner::Daemon,
                pid: Some(daemon.id()),
                state: "running".into(),
                label: None,
                details: None,
                stdout_path: None,
                stderr_path: None,
                stdin_path: None,
                limits: None,
                started_at: chrono::Utc::now().to_rfc3339(),
                exit_code: None,
            })
            .unwrap();
        Self {
            root,
            lifecycle,
            supervisor,
            worker,
            daemon,
        }
    }

    fn runtime(&self) -> PathBuf {
        self.lifecycle.runtime_root.port_root(PORT)
    }

    fn groups(&self) -> (OwnedGroup, OwnedGroup) {
        let worker = self
            .supervisor
            .owned_groups()
            .unwrap()
            .into_iter()
            .find(|group| group.process.id == self.worker.id)
            .unwrap();
        let agent = FileProcessSupervisor::new(self.runtime().join("agents"))
            .owned_groups()
            .unwrap()
            .remove(0);
        (worker, agent)
    }

    fn assert_complete(&self, worker: &OwnedGroup, agent: &OwnedGroup) {
        for group in [worker, agent] {
            let supervisor = FileProcessSupervisor::new(&group.runtime_root);
            assert!(
                supervisor
                    .observe_owned_group(group)
                    .unwrap()
                    .confirmed_exit
            );
            assert!(!supervisor.group_pending(&group.process).unwrap());
            let proof = fs::read(&group.launch_scope.as_ref().unwrap().proof_path).unwrap();
            assert_eq!(&proof[8..], b"exited\n", "missing exact scope receipt");
            assert!(!FileProcessSupervisor::process_is_alive(&group.process).unwrap());
        }
        assert!(!self.runtime().join("parent-observed-agent-exit").exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::write(self.runtime().join("allow-agent-proof"), "release fixture");
        let _ = self.lifecycle.stop_runtime_processes(PORT);
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
        // The worker was launched by this test's reaper. Its active registration
        // disappears only after that writer finishes creating archive files.
        let registration = self
            .supervisor
            .processes_dir()
            .join(format!("{}.json", self.worker.id));
        let deadline = Instant::now() + Duration::from_secs(2);
        while registration.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        if !registration.exists() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn ancestor_stop_preserves_nested_guardians_until_both_scope_receipts_complete() {
    let fixture = Fixture::new();
    let (worker, agent) = fixture.groups();
    let guardian = agent.launch_scope.as_ref().unwrap();
    // Registration precedes the gated workload's setsid; its cached pgid can
    // still be absent. Agent registration proves the worker has now executed.
    let worker_pid = worker.process.pid.unwrap() as i32;
    let worker_pgid = unsafe { libc::getpgid(worker_pid) };
    assert_eq!(worker_pgid, worker_pid);
    assert_eq!(
        unsafe { libc::getpgid(guardian.guardian_pid as i32) },
        worker_pgid
    );
    let error = fixture
        .supervisor
        .stop_owned_group(&worker, Duration::from_secs(2))
        .unwrap_err();
    assert!(
        error.to_string().contains("did not confirm exit"),
        "{error}"
    );
    assert_eq!(
        current_os_identity(guardian.guardian_pid).unwrap(),
        guardian.guardian_identity
    );
    assert!(
        !fixture
            .runtime()
            .join("parent-observed-agent-exit")
            .exists()
    );
    fs::write(fixture.runtime().join("allow-agent-proof"), "complete").unwrap();
    fixture
        .supervisor
        .stop_owned_group(&worker, Duration::from_secs(2))
        .unwrap();
    fixture.assert_complete(&worker, &agent);
}

#[test]
fn direct_stop_waits_for_retained_nested_scope_before_daemon_and_replacement() {
    let mut fixture = Fixture::new();
    let (worker, agent) = fixture.groups();
    // The retained scope remains discoverable even after the primary record is lost.
    fs::remove_file(
        FileProcessSupervisor::new(&agent.runtime_root)
            .processes_dir()
            .join(format!("{}.json", agent.process.id)),
    )
    .unwrap();
    let error = stop_direct_runtime(&fixture.lifecycle, PORT).unwrap_err();
    assert!(
        error.to_string().contains("did not confirm exit"),
        "{error}"
    );
    assert!(
        fixture.daemon.try_wait().unwrap().is_none(),
        "daemon stopped before scope proof"
    );
    assert!(
        !fixture
            .runtime()
            .join("parent-observed-agent-exit")
            .exists()
    );
    fs::write(fixture.runtime().join("allow-agent-proof"), "complete").unwrap();
    stop_direct_runtime(&fixture.lifecycle, PORT).unwrap();
    fixture.daemon.wait().unwrap();
    fixture.assert_complete(&worker, &agent);
    let replacement = fixture
        .supervisor
        .run_to_completion(spec(
            ProcessOwner::Runner,
            "/bin/sleep",
            vec!["0.01".into()],
        ))
        .unwrap();
    assert_eq!(replacement.process.exit_code, Some(0));
    fixture.assert_complete(&worker, &agent);
    fixture.lifecycle.stop_runtime(PORT).unwrap();
}

fn managed_control(
    lifecycle: &FileDaemonLifecycleService,
    action: InstalledServiceAction,
    control: impl FnMut() -> RefineResult<()>,
    probe: impl FnMut(u16) -> DaemonReachability,
) -> RefineResult<DaemonStatus> {
    match action {
        InstalledServiceAction::Stop => stop_service_managed_daemon_with(
            lifecycle,
            PORT,
            "systemd_user",
            Duration::from_secs(1),
            Duration::from_millis(5),
            control,
            probe,
        ),
        InstalledServiceAction::Restart => run_service_managed_daemon_with(
            lifecycle,
            PORT,
            "systemd_user",
            action,
            Duration::from_secs(1),
            Duration::from_millis(5),
            control,
            probe,
        ),
        _ => unreachable!(),
    }
}

#[test]
fn managed_stop_and_restart_drain_nested_scopes_under_the_supervision_lease() {
    use fs2::FileExt;
    use std::cell::Cell;

    for action in [
        InstalledServiceAction::Stop,
        InstalledServiceAction::Restart,
    ] {
        let mut fixture = Fixture::new();
        let lifecycle = fixture.lifecycle.clone();
        let (worker, agent) = fixture.groups();
        let controlled = Cell::new(false);
        let error = managed_control(
            &lifecycle,
            action,
            || {
                controlled.set(true);
                Ok(())
            },
            |_| DaemonReachability::Reachable,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("did not confirm exit"),
            "{error}"
        );
        assert!(
            !controlled.get(),
            "manager control ran before nested scope exit"
        );
        assert!(fixture.daemon.try_wait().unwrap().is_none());
        assert!(
            !fixture
                .runtime()
                .join("parent-observed-agent-exit")
                .exists()
        );

        fs::write(fixture.runtime().join("allow-agent-proof"), "complete").unwrap();
        let status = managed_control(
            &lifecycle,
            action,
            || {
                fixture.assert_complete(&worker, &agent);
                assert!(
                    fixture.daemon.try_wait().unwrap().is_none(),
                    "drain killed the daemon"
                );
                let competing = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(fixture.runtime().join("workflow-supervision.lock"))
                    .unwrap();
                assert_eq!(
                    competing.try_lock_exclusive().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock,
                    "workflow replacement was allowed during manager control"
                );
                controlled.set(true);
                fixture.daemon.kill().unwrap();
                fixture.daemon.wait().unwrap();
                Ok(())
            },
            |_| {
                if action == InstalledServiceAction::Stop && controlled.get() {
                    DaemonReachability::Unreachable("manager stopped daemon".into())
                } else {
                    DaemonReachability::Reachable
                }
            },
        )
        .unwrap();
        assert!(controlled.get());
        fixture.assert_complete(&worker, &agent);
        assert_eq!(
            status.daemon_healthy,
            action == InstalledServiceAction::Restart
        );
        // Managed Stop's ordinary post-control cleanup acquires this same lease;
        // successful completion also proves the wrapper did not reenter it.
        let available = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.runtime().join("workflow-supervision.lock"))
            .unwrap();
        available.try_lock_exclusive().unwrap();
        FileExt::unlock(&available).unwrap();
    }
}

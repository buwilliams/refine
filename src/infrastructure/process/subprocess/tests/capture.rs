use super::*;
use std::thread;

struct Fixture(PathBuf, FileProcessSupervisor);
impl Fixture {
    fn new() -> Self {
        let root = unique_temp_dir("standard-capture");
        fs::create_dir_all(&root).unwrap();
        Self(root.clone(), FileProcessSupervisor::new(root))
    }
    fn spec(&self, script: String) -> ManagedProcessSpec {
        ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: "python3".into(),
            args: vec!["-c".into(), script],
            cwd: Some(self.0.display().to_string()),
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(json!({
                "workflow_incarnation":"standard-capture", "isolated_process_group":true,
                "completion_timeout_seconds":5
            }))
            .unwrap(),
        }
    }
    fn run(&self, spec: ManagedProcessSpec) -> RefineResult<ManagedProcessOutput> {
        let supervisor = self.1.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(supervisor.run_to_completion(spec));
        });
        rx.recv_timeout(Duration::from_secs(4))
            .expect("capture did not return within bound")
    }
    fn released(&self, process: &ManagedProcess) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.1.group_pending(process).unwrap() {
            assert!(Instant::now() < deadline, "scope exit was not proved");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(self.1.capacity_processes().unwrap().is_empty());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for group in self.1.owned_groups().unwrap_or_default() {
            let _ = self.1.stop_owned_group(&group, Duration::from_secs(2));
        }
        // Keep failing test evidence; only process cleanup is unconditional.
        if !thread::panicking() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn script(streams: &[i32], writing: bool, exit: &str, delay: f32) -> String {
    format!(
        r#"
import os, time, signal
from pathlib import Path
if os.fork() == 0:
    os.setsid()
    for fd in (1,2):
        if fd not in {streams:?}:
            null = os.open('/dev/null', os.O_WRONLY)
            os.dup2(null, fd)
            os.close(null)
    Path('child.pid').write_text(str(os.getpid()))
    Path('ready').touch()
    while not Path('release').exists():
        if {writing}:
            for fd in {streams:?}:
                try: os.write(fd, b'descendant\n' * 32)
                except BrokenPipeError: Path('broken').touch()
        time.sleep(0.001)
    os._exit(0)
while not Path('ready').exists(): time.sleep(0.001)
os.write(1,b'leader-out\n')
os.write(2,b'leader-err\n')
time.sleep({delay})
{exit}
"#,
        writing = if writing { "True" } else { "False" }
    )
}

#[test]
fn inherited_pipes_return_bounded_results_and_retain_owned_capacity_and_artifacts() {
    for streams in [vec![1], vec![2], vec![1, 2]] {
        for writing in [false, true] {
            let f = Fixture::new();
            let exit = match streams.len() + usize::from(writing) {
                1 => "os._exit(0)",
                2 => "os._exit(9)",
                _ => "os.kill(os.getpid(), signal.SIGTERM)",
            };
            let started = Instant::now();
            let output = f.run(f.spec(script(&streams, writing, exit, 0.0))).unwrap();
            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(
                output.process.exit_code,
                if exit.contains("SIGTERM") {
                    None
                } else if exit.contains("9") {
                    Some(9)
                } else {
                    Some(0)
                }
            );
            assert!(output.stdout.contains("leader-out"));
            assert!(output.stderr.contains("leader-err"));
            assert!(!output.capture_complete());
            assert!(!output.success());
            assert!(output.require_complete_capture().is_err());
            let details: Value =
                serde_json::from_str(output.process.details.as_ref().unwrap()).unwrap();
            for (fd, name) in [(1, "stdout"), (2, "stderr")] {
                assert_eq!(
                    details["output_capture"][name]["eof"],
                    !streams.contains(&fd)
                );
            }
            let child: u32 = fs::read_to_string(f.0.join("child.pid"))
                .unwrap()
                .parse()
                .unwrap();
            let identity = os_process_identity(child).unwrap().unwrap();
            let group = f.1.owned_groups().unwrap().remove(0);
            let scope = group.launch_scope.as_ref().unwrap();
            assert_eq!(
                os_process_identity(scope.guardian_pid).unwrap(),
                scope.guardian_identity
            );
            assert!(
                os_process_identity(output.process.pid.unwrap())
                    .unwrap()
                    .is_none()
            );
            assert!(f.1.group_pending(&output.process).unwrap());
            assert_eq!(f.1.capacity_processes().unwrap().len(), 1);
            let receipt: ManagedProcess = serde_json::from_slice(
                &fs::read(
                    f.0.join("output-captures")
                        .join(format!("{}.json", output.process.id)),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(receipt, output.process);
            // Actual retirement must remain conservative without the primary registration.
            fs::remove_file(
                f.1.processes_dir()
                    .join(format!("{}.json", output.process.id)),
            )
            .unwrap();
            f.1.retire_aged_process_logs(Duration::ZERO);
            assert_eq!(f.1.capacity_processes().unwrap().len(), 1);
            for path in [&output.process.stdout_path, &output.process.stderr_path] {
                assert!(Path::new(path.as_ref().unwrap()).exists());
            }
            if writing {
                let deadline = Instant::now() + Duration::from_secs(1);
                while !f.0.join("broken").exists() {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
            }
            assert_eq!(os_process_identity(child).unwrap(), Some(identity));
            fs::write(f.0.join("release"), "").unwrap();
            f.released(&output.process);
            assert!(os_process_identity(child).unwrap().is_none());
            f.1.retire_aged_process_logs(Duration::ZERO);
            assert!(!Path::new(output.process.stdout_path.as_ref().unwrap()).exists());
            assert!(!Path::new(output.process.stderr_path.as_ref().unwrap()).exists());
        }
    }
}

#[test]
fn ordinary_eof_and_finite_trailing_output_preserve_complete_callback_order() {
    for script in [
        "import os; os.write(1,b'first'); os.write(2,b'error')",
        "import os,time\nif os.fork()==0:\n time.sleep(0.05); os.write(1,b'last'); os._exit(0)\nos.write(1,b'first')\nos._exit(0)",
    ] {
        let f = Fixture::new();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let output =
            f.1.run_to_completion_with_output(f.spec(script.into()), |stream, bytes| {
                let (observed, suffix) = match stream {
                    ManagedProcessOutputStream::Stdout => (&mut out, ".stdout.log"),
                    ManagedProcessOutputStream::Stderr => (&mut err, ".stderr.log"),
                };
                observed.extend_from_slice(bytes);
                // Artifact bytes must be visible before notifying the consumer.
                let path = fs::read_dir(f.1.processes_dir())
                    .unwrap()
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| p.to_string_lossy().ends_with(suffix))
                    .unwrap();
                assert_eq!(fs::read(path).unwrap(), *observed);
            })
            .unwrap();
        assert!(output.success());
        assert!(output.capture_complete());
        assert_eq!(output.stdout.as_bytes(), out);
        assert_eq!(output.stderr.as_bytes(), err);
        assert_eq!(
            output.stdout,
            if script.contains("fork") {
                "firstlast"
            } else {
                "first"
            }
        );
        f.released(&output.process);
    }
}

#[test]
fn completion_and_stall_deadlines_remain_effective_during_final_drain() {
    for hard_cap in [false, true] {
        let f = Fixture::new();
        let mut spec = f.spec(script(&[1, 2], false, "os._exit(9)", 0.9));
        if hard_cap {
            spec.metadata
                .insert("completion_timeout_seconds".into(), json!(1));
        } else {
            spec.limits = Some(ProcessResourceLimits {
                stall_timeout_seconds: Some(1),
                ..Default::default()
            });
        }
        let start = Instant::now();
        let error = f.run(spec).unwrap_err().to_string();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(
            error.contains(if hard_cap {
                "completion timeout"
            } else {
                "produced no output"
            }),
            "{error}"
        );
        let group = f.1.owned_groups().unwrap().remove(0);
        f.released(&group.process);
        let receipt: ManagedProcess = serde_json::from_slice(
            &fs::read(
                f.0.join("output-captures")
                    .join(format!("{}.json", group.process.id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(receipt.exit_code, Some(9));
        assert!(
            receipt.details.unwrap().contains("incomplete") || error.contains("\"complete\":false")
        );
    }
}

#[test]
fn capture_write_failure_uses_bounded_scope_termination_and_retains_original_error() {
    let f = Fixture::new();
    install_capture_hook(&f.0, "before_capture", |process| {
        std::os::unix::fs::symlink("/dev/full", process.stdout_path.as_ref().unwrap()).unwrap();
    });
    let error = f
        .run(f.spec(script(&[1, 2], false, "time.sleep(30)", 0.0)))
        .unwrap_err()
        .to_string();
    assert!(error.contains("output capture failed"), "{error}");
    assert!(error.contains("No space left"), "{error}");
    let group = f.1.owned_groups().unwrap().remove(0);
    f.released(&group.process);
    let evidence =
        f.0.join("output-captures")
            .join(format!("{}.json", group.process.id));
    let value: Value = serde_json::from_slice(&fs::read(evidence).unwrap()).unwrap();
    assert!(value["details"].as_str().unwrap().contains("No space left"));
    assert!(
        value["captured_stdout"]
            .as_str()
            .unwrap()
            .contains("leader-out")
    );
}

#[test]
fn missing_and_mismatched_scope_proof_cannot_release_incomplete_capture() {
    for missing in [false, true] {
        let f = Fixture::new();
        let root = f.0.clone();
        install_capture_hook(&f.0, "workload_exit", move |process| {
            let details: Value = serde_json::from_str(process.details.as_ref().unwrap()).unwrap();
            let path = PathBuf::from(details["launch_scope"]["proof_path"].as_str().unwrap());
            fs::rename(&path, root.join("retained-proof")).unwrap();
            if !missing {
                fs::write(&path, fs::read(root.join("retained-proof")).unwrap()).unwrap();
            }
        });
        let output = f
            .run(f.spec(script(&[1, 2], false, "os._exit(9)", 0.0)))
            .unwrap();
        assert_eq!(output.process.exit_code, Some(9));
        assert!(!output.capture_complete());
        let group = f.1.owned_groups().unwrap().remove(0);
        assert!(matches!(
            f.1.assess_owned_group(&group).unwrap(),
            owned_groups::OwnershipAssessment::Unverified { .. }
        ));
        assert_eq!(f.1.capacity_processes().unwrap().len(), 1);
        f.1.retire_aged_process_logs(Duration::ZERO);
        assert!(Path::new(output.process.stdout_path.as_ref().unwrap()).exists());
        fs::write(f.0.join("release"), "").unwrap();
        // Restore the original inode, not a fabricated scan-based exit claim.
        fs::rename(
            f.0.join("retained-proof"),
            &group.launch_scope.as_ref().unwrap().proof_path,
        )
        .unwrap();
        f.released(&output.process);
    }
}

#[test]
fn capture_memory_is_bounded_and_truncation_is_explicit_while_artifact_is_complete() {
    let f = Fixture::new();
    let path = f.0.join("bounded.log");
    let mut capture =
        super::super::capture::Capture::new(fs::File::open("/dev/zero").unwrap(), &path).unwrap();
    let mut observed = 0;
    for _ in 0..257 {
        assert!(capture.poll(|b| observed += b.len()).unwrap());
    }
    assert_eq!(capture.text().len(), 16 * 1024 * 1024);
    assert!(observed > capture.text().len());
    assert_eq!(fs::metadata(&path).unwrap().len(), observed as u64);
    let evidence = capture.evidence("drain expired");
    assert_eq!(evidence["buffer_truncated"], true);
    assert_eq!(evidence["complete"], false);
}

#[test]
fn direct_capture_deadline_preserves_replacement_registration_and_both_processes() {
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    type Original = Arc<Mutex<Option<(u32, String)>>>;
    struct OriginalGuard(Original);
    impl Drop for OriginalGuard {
        fn drop(&mut self) {
            if let Some((pid, identity)) = self.0.lock().unwrap().as_ref() {
                if os_process_identity(*pid).ok().flatten().as_ref() == Some(identity) {
                    let _ = signal_os_process(*pid, "kill", false);
                }
            }
        }
    }
    let f = Fixture::new();
    let replacement = ChildGuard(Command::new("sleep").arg("30").spawn().unwrap());
    let replacement_pid = replacement.0.id();
    let original = OriginalGuard(Arc::new(Mutex::new(None)));
    let observed = original.0.clone();
    let root = f.0.clone();
    install_capture_hook(&f.0, "before_capture", move |process| {
        let details: Value = serde_json::from_str(process.details.as_ref().unwrap()).unwrap();
        assert!(details["launch_scope"].is_null());
        let pid = process.pid.unwrap();
        *observed.lock().unwrap() = Some((pid, os_process_identity(pid).unwrap().unwrap()));
        let mut current = process.clone();
        current.pid = Some(replacement_pid);
        current.started_at = "replacement".into();
        fs::write(
            root.join("processes").join(format!("{}.json", process.id)),
            serde_json::to_vec(&current).unwrap(),
        )
        .unwrap();
    });
    let mut spec =
        f.spec("import os,time; os.write(1,b'captured-before-timeout'); time.sleep(30)".into());
    spec.metadata.remove("workflow_incarnation");
    spec.metadata
        .insert("completion_timeout_seconds".into(), json!(1));
    let error = f.run(spec).unwrap_err().to_string();
    assert!(error.contains("completion timeout"), "{error}");
    assert!(error.contains("registry identity changed"), "{error}");
    let pid = original.0.lock().unwrap().as_ref().unwrap().0;
    assert!(os_process_identity(pid).unwrap().is_some());
    assert!(os_process_identity(replacement_pid).unwrap().is_some());
    let current = f.1.list().unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].pid, Some(replacement_pid));
    assert_eq!(
        fs::read_to_string(current[0].stdout_path.as_ref().unwrap()).unwrap(),
        "captured-before-timeout"
    );
}

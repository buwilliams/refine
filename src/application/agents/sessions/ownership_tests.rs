//! Actual PTY consumer regressions with unobserved, reparented descendants.
use super::*;
use crate::infrastructure::process::subprocess::owned_groups::OwnershipAssessment;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

type Hook = Arc<dyn Fn(&str, &ManagedProcess) -> RefineResult<()> + Send + Sync>;
static HOOKS: OnceLock<Mutex<BTreeMap<PathBuf, Hook>>> = OnceLock::new();
pub(crate) fn install(root: &Path, hook: Hook) {
    HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(root.into(), hook);
}
pub(crate) fn hook(root: &Path, stage: &str, process: &ManagedProcess) -> RefineResult<()> {
    let hook = HOOKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .get(root)
        .cloned();
    if let Some(hook) = hook {
        hook(stage, process)?;
    }
    Ok(())
}
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready() {
        assert!(Instant::now() < deadline, "fixture gate timed out");
        thread::sleep(Duration::from_millis(5));
    }
}
struct Env(Option<std::ffi::OsString>);
impl Drop for Env {
    fn drop(&mut self) {
        unsafe {
            match &self.0 {
                Some(v) => std::env::set_var("REFINE_SMOKE_AI_PATH", v),
                None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
            }
        }
    }
}
fn fixture(mode: &str, extra: Value) -> (PathBuf, GoalAgentLaunch, Env) {
    let root = std::env::temp_dir().join(format!("refine-pty-{mode}-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let provider = root.join("provider");
    let ending = match mode {
        "completion" => {
            "printf '%s' '{\"state\":\"completed\",\"message\":\"done\",\"guidance_applied\":[0],\"planning_result\":{\"summary\":\"plan\",\"checklist\":[]}}' > \"$REFINE_AGENT_SIGNAL_PATH\"; exit 0"
        }
        "nonzero" => "exit 7",
        "natural" => "exit 0",
        _ => "sleep 60",
    };
    fs::write(&provider, format!("#!/bin/sh\nsetsid sh -c 'echo $$ > child.pid; exec sleep 60' &\nwhile [ ! -s child.pid ]; do sleep 0.005; done\necho transcript-evidence\n{ending}\n")).unwrap();
    fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
    let env = Env(std::env::var_os("REFINE_SMOKE_AI_PATH"));
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }
    let mut metadata = serde_json::from_value::<Map<String, Value>>(extra).unwrap();
    metadata.insert("goal_id".into(), json!("PTY-OWNERSHIP"));
    if mode == "completion" {
        metadata.insert("implementation_phase".into(), json!("plan"));
    }
    let launch = GoalAgentLaunch {
        runtime_root: root.join("runtime"),
        cwd: root.clone(),
        provider: "smoke-ai".into(),
        prompt: "test".into(),
        metadata,
        completion_timeout: Some(Duration::from_millis(350)),
        idle_timeout: (mode == "idle").then_some(Duration::from_millis(100)),
        provider_session: None,
    };
    (root, launch, env)
}

#[test]
fn pty_results_and_deadlines_reap_unobserved_setsid_descendants() {
    let _lock = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for mode in ["completion", "natural", "nonzero", "hard-cap", "idle"] {
        let (root, launch, _env) = fixture(mode, json!({}));
        let supervisor = FileProcessSupervisor::new(&launch.runtime_root);
        let observed = Arc::new(Mutex::new(None));
        let saved = observed.clone();
        let owner = supervisor.clone();
        let path = root.clone();
        install(
            &launch.runtime_root,
            Arc::new(move |stage, process| {
                if stage != "poll" || saved.lock().unwrap().is_some() {
                    return Ok(());
                }
                wait(|| path.join("child.pid").exists());
                if ["completion", "natural", "nonzero"].contains(&mode) {
                    wait(|| !Path::new(&format!("/proc/{}", process.pid.unwrap())).exists());
                }
                let child: u32 = fs::read_to_string(path.join("child.pid"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                assert!(os_process_identity(child)?.is_some());
                let group = owner.owned_groups()?.remove(0);
                assert_eq!(
                    group.witnesses.len(),
                    1,
                    "no descendant observation before parent exit"
                );
                assert!(
                    os_process_identity(group.launch_scope.as_ref().unwrap().guardian_pid)?
                        .is_some()
                );
                // Missing primary registration must still use retained ownership.
                if mode == "natural" {
                    fs::remove_file(owner.processes_dir().join(format!("{}.json", process.id)))
                        .unwrap();
                }
                assert!(owner.group_pending(process)?);
                assert_eq!(owner.capacity_processes()?.len(), 1);
                *saved.lock().unwrap() = Some((process.clone(), child));
                Ok(())
            }),
        );
        let start = Instant::now();
        let result = run_goal_agent(launch, |_| {});
        assert!(start.elapsed() < Duration::from_secs(4), "{mode}");
        if mode == "completion" {
            let result = result.unwrap();
            assert_eq!(result.output, "done");
            assert_eq!(result.guidance_applied, Some(vec![0]));
            assert_eq!(result.planning_result.unwrap()["summary"], "plan");
        } else if mode == "natural" {
            assert!(result.unwrap().output.contains("transcript-evidence"));
        } else {
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains(if mode == "nonzero" {
                    "unsuccessfully: 7"
                } else if mode == "idle" {
                    "no output"
                } else {
                    "valid completion signal"
                }),
                "{mode}: {error}"
            );
        }
        let (process, child) = observed.lock().unwrap().clone().unwrap();
        assert!(
            os_process_identity(child).unwrap().is_none(),
            "real descendant remains"
        );
        assert!(!supervisor.group_pending(&process).unwrap());
        assert!(supervisor.capacity_processes().unwrap().is_empty());
        let group = supervisor.owned_groups().unwrap().remove(0);
        assert_eq!(
            supervisor.assess_owned_group(&group).unwrap(),
            OwnershipAssessment::Exited
        );
        HOOKS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .remove(&supervisor.runtime_root);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn pty_uncertainty_returns_with_capacity_and_artifacts_until_proof_is_available() {
    let _lock = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for fault in ["termination", "missing", "mismatch", "delayed-proof"] {
        let (root, mut launch, _env) = fixture(
            if fault == "termination" {
                "nonzero"
            } else {
                "natural"
            },
            json!({"test_termination_failure":fault == "termination"}),
        );
        let gate = root.join("receipt-gate");
        if fault == "delayed-proof" {
            launch
                .metadata
                .insert("test_scope_exit_gate".into(), json!(gate));
        }
        let owner = FileProcessSupervisor::new(&launch.runtime_root);
        let supervisor = owner.clone();
        let path = root.clone();
        let evidence = Arc::new(Mutex::new(None));
        let saved = evidence.clone();
        install(
            &launch.runtime_root,
            Arc::new(move |stage, process| {
                if stage != "poll" || saved.lock().unwrap().is_some() {
                    return Ok(());
                }
                wait(|| !Path::new(&format!("/proc/{}", process.pid.unwrap())).exists());
                let group = supervisor.owned_groups()?.remove(0);
                let child: u32 = fs::read_to_string(path.join("child.pid"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                assert!(os_process_identity(child)?.is_some());
                let record = supervisor
                    .runtime_root
                    .join("owned-groups")
                    .join(format!("{}.json", process.id));
                if fault == "missing" {
                    fs::remove_file(&record).unwrap();
                }
                if fault == "mismatch" {
                    let proof = &group.launch_scope.as_ref().unwrap().proof_path;
                    fs::rename(proof, proof.with_extension("saved")).unwrap();
                    fs::write(proof, b"wrong identity").unwrap();
                }
                *saved.lock().unwrap() = Some((group, record, child));
                Ok(())
            }),
        );
        let start = Instant::now();
        let error = run_goal_agent(launch, |_| {}).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(4), "{fault}: {error}");
        let (mut group, record, child) = evidence.lock().unwrap().clone().unwrap();
        assert!(owner.group_pending(&group.process).unwrap_or(true));
        let health =
            crate::application::workflow::health::assess_workflow_health(&owner.runtime_root);
        assert!(!health.healthy);
        assert!(
            health.reason.contains(&group.process.id),
            "{}",
            health.reason
        );
        assert!(health.remedy.contains("refine system doctor"));
        assert_eq!(owner.capacity_processes().unwrap().len(), 1);
        assert!(Path::new(group.process.stdout_path.as_ref().unwrap()).is_file());
        assert!(Path::new(group.process.stdin_path.as_ref().unwrap()).is_file());
        if fault == "mismatch" {
            let proof = &group.launch_scope.as_ref().unwrap().proof_path;
            fs::rename(proof.with_extension("saved"), proof).unwrap();
        }
        if fault == "termination" {
            let mut details: Value =
                serde_json::from_str(group.process.details.as_ref().unwrap()).unwrap();
            details["test_termination_failure"] = json!(false);
            group.process.details = Some(details.to_string());
        }
        if fault == "missing" || fault == "termination" {
            fs::write(record, serde_json::to_vec(&group).unwrap()).unwrap();
        }
        fs::write(gate, "release receipt").unwrap();
        assert!(
            owner
                .stop_owned_group(&group, Duration::from_secs(2))
                .unwrap()
                .confirmed_exit
        );
        assert!(os_process_identity(child).unwrap().is_none());
        assert!(!owner.group_pending(&group.process).unwrap());
        assert!(owner.capacity_processes().unwrap().is_empty());
        HOOKS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .remove(&owner.runtime_root);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn pty_setup_transport_and_capture_faults_preserve_original_failure() {
    let _lock = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    for fault in [
        "register",
        "release",
        "released",
        "capture",
        "transport",
        "settlement",
    ] {
        let (root, launch, _env) = fixture(
            if fault == "settlement" {
                "natural"
            } else {
                "hard-cap"
            },
            json!({}),
        );
        let owner = FileProcessSupervisor::new(&launch.runtime_root);
        let path = root.clone();
        let first = Arc::new(Mutex::new(true));
        let once = first.clone();
        install(
            &launch.runtime_root,
            Arc::new(move |stage, process| {
                if stage == fault && fault != "capture" {
                    return Err(RefineError::Io(format!("injected {fault}")));
                }
                if stage == "capture" && fault == "capture" {
                    let output = process.stdout_path.as_ref().unwrap();
                    fs::remove_file(output).unwrap();
                    std::os::unix::fs::symlink("/dev/full", output).unwrap();
                }
                if stage == "poll" && *once.lock().unwrap() {
                    *once.lock().unwrap() = false;
                    wait(|| path.join("child.pid").exists());
                    if fault == "transport" {
                        fs::write(process.stdin_path.as_ref().unwrap(), "malformed command\n")
                            .unwrap();
                    }
                }
                Ok(())
            }),
        );
        let result = run_goal_agent_with_settlement(
            launch,
            |_| {},
            |_| Err(RefineError::Io("injected settlement".into())),
        );
        assert!(result.is_err());
        if ["register", "release", "capture"].contains(&fault) {
            assert!(result.unwrap_err().to_string().contains(fault));
        }
        // Gated failures never run the workload; released failures prove scope exit.
        if ["register", "release"].contains(&fault) {
            assert!(!root.join("child.pid").exists());
        } else {
            if let Ok(pid) = fs::read_to_string(root.join("child.pid")) {
                let pid: u32 = pid.trim().parse().unwrap();
                assert!(os_process_identity(pid).unwrap().is_none());
            }
            assert!(owner.capacity_processes().unwrap().is_empty());
        }
        HOOKS
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .remove(&owner.runtime_root);
        fs::remove_dir_all(root).unwrap();
    }
}

fn os_process_identity(pid: u32) -> RefineResult<Option<String>> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            let fields: Vec<_> = stat
                .rsplit_once(')')
                .unwrap()
                .1
                .split_whitespace()
                .collect();
            Ok((fields[0] != "Z" && fields[0] != "X").then(|| fields[19].to_string()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(RefineError::Io(e.to_string())),
    }
}

#[test]
fn completed_pty_keeps_structured_result_when_scope_termination_is_unavailable() {
    let _lock = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (root, launch, _env) = fixture("completion", json!({"test_termination_failure":true}));
    let owner = FileProcessSupervisor::new(&launch.runtime_root);
    let error = run_goal_agent(launch, |_| {}).unwrap_err();
    assert!(error.to_string().contains("termination failure"));
    let mut group = owner.owned_groups().unwrap().remove(0);
    let process: ManagedProcess = serde_json::from_slice(
        &fs::read(
            owner
                .processes_dir()
                .join(format!("{}.json", group.process.id)),
        )
        .unwrap(),
    )
    .unwrap();
    let details: Value = serde_json::from_str(process.details.as_ref().unwrap()).unwrap();
    assert_eq!(details["workload_result"]["output"], "done");
    assert_eq!(
        details["workload_result"]["planning_result"]["summary"],
        "plan"
    );
    assert_eq!(details["workload_result"]["guidance_applied"], json!([0]));
    assert!(owner.group_pending(&process).unwrap());
    let mut origin: Value = serde_json::from_str(group.process.details.as_ref().unwrap()).unwrap();
    origin["test_termination_failure"] = json!(false);
    group.process.details = Some(origin.to_string());
    fs::write(
        owner
            .runtime_root
            .join("owned-groups")
            .join(format!("{}.json", process.id)),
        serde_json::to_vec(&group).unwrap(),
    )
    .unwrap();
    owner
        .stop_owned_group(&group, Duration::from_secs(2))
        .unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_pty_settlement_preserves_replacement_registration_and_descendants() {
    let _lock = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (root, launch, _env) = fixture("natural", json!({}));
    let owner = FileProcessSupervisor::new(&launch.runtime_root);
    let saved = Arc::new(Mutex::new(None));
    let evidence = saved.clone();
    let supervisor = owner.clone();
    install(
        &launch.runtime_root,
        Arc::new(move |stage, process| {
            if stage != "poll" || evidence.lock().unwrap().is_some() {
                return Ok(());
            }
            wait(|| !Path::new(&format!("/proc/{}", process.pid.unwrap())).exists());
            let mut replacement = supervisor.owned_groups()?.remove(0);
            replacement.process.started_at = "replacement registration".into();
            fs::write(
                supervisor
                    .runtime_root
                    .join("owned-groups")
                    .join(format!("{}.json", process.id)),
                serde_json::to_vec(&replacement).unwrap(),
            )
            .unwrap();
            let record = supervisor
                .processes_dir()
                .join(format!("{}.json", process.id));
            let bytes = serde_json::to_vec(&replacement.process).unwrap();
            fs::write(&record, &bytes).unwrap();
            *evidence.lock().unwrap() = Some((replacement, record, bytes));
            Ok(())
        }),
    );
    let error = run_goal_agent(launch, |_| {}).unwrap_err();
    assert!(
        error.to_string().contains("registration identity changed"),
        "{error}"
    );
    let (replacement, record, bytes) = saved.lock().unwrap().take().unwrap();
    assert_eq!(
        fs::read(record).unwrap(),
        bytes,
        "stale error cleanup overwrote replacement"
    );
    let child: u32 = fs::read_to_string(root.join("child.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(os_process_identity(child).unwrap().is_some());
    assert_eq!(owner.capacity_processes().unwrap().len(), 1);
    assert!(Path::new(replacement.process.stdout_path.as_ref().unwrap()).is_file());
    owner
        .stop_owned_group(&replacement, Duration::from_secs(2))
        .unwrap();
    assert!(os_process_identity(child).unwrap().is_none());
    assert!(owner.capacity_processes().unwrap().is_empty());
    HOOKS
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .remove(&owner.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

use super::*;

fn worker_record(state: &str, worker_kind: &str) -> ManagedProcess {
    ManagedProcess {
        id: format!("process-{state}-{worker_kind}"),
        owner: ProcessOwner::Runner,
        pid: Some(4242),
        state: state.to_string(),
        label: Some("workflow runner".to_string()),
        details: Some(json!({"worker_kind": worker_kind}).to_string()),
        stdout_path: None,
        stderr_path: None,
        stdin_path: None,
        limits: None,
        started_at: String::new(),
        exit_code: None,
    }
}

#[test]
fn settled_worker_records_are_not_adopted_as_a_running_workflow_runner() {
    assert!(adoptable_worker(
        &worker_record("running", WORKFLOW_RUNNER),
        WORKFLOW_RUNNER
    ));
    // Each of these means the worker is gone. Adopting one leaves nothing
    // ticking the workflow while supervision believes a runner exists.
    for state in ["exited", "failed", "stopped", "interrupted"] {
        assert!(
            !adoptable_worker(&worker_record(state, WORKFLOW_RUNNER), WORKFLOW_RUNNER),
            "a {state} record must not be adopted as a live workflow runner"
        );
    }
    assert!(!adoptable_worker(
        &worker_record("running", GIT_SYNC_RUNNER),
        WORKFLOW_RUNNER
    ));
    let mut foreign_owner = worker_record("running", WORKFLOW_RUNNER);
    foreign_owner.owner = ProcessOwner::Agent;
    assert!(!adoptable_worker(&foreign_owner, WORKFLOW_RUNNER));
}

#[test]
fn an_unremovable_settled_record_cannot_stop_supervision_from_seeing_the_registry() {
    // A settled record whose artifacts refuse to be removed used to fail the
    // entire process listing. Every consumer reads through that listing, so
    // one leftover silently stopped supervision from noticing a dead workflow
    // runner — and nothing relaunched it short of a daemon restart.
    let runtime_root =
        std::env::temp_dir().join(format!("refine-stuck-record-{}", uuid::Uuid::new_v4()));
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    std::fs::create_dir_all(supervisor.processes_dir()).unwrap();

    let mut settled = worker_record("exited", WORKFLOW_RUNNER);
    // A directory cannot be removed as a file, so cleanup of this record
    // fails the way an undeletable leftover does in the field.
    let undeletable = runtime_root.join("undeletable-stdout");
    std::fs::create_dir_all(&undeletable).unwrap();
    settled.stdout_path = Some(undeletable.display().to_string());
    std::fs::write(
        supervisor
            .processes_dir()
            .join(format!("{}.json", settled.id)),
        serde_json::to_vec_pretty(&settled).unwrap(),
    )
    .unwrap();

    let live = worker_record("running", GIT_SYNC_RUNNER);
    std::fs::write(
        supervisor.processes_dir().join(format!("{}.json", live.id)),
        serde_json::to_vec_pretty(&live).unwrap(),
    )
    .unwrap();

    let listed = supervisor
        .list()
        .expect("a settled record that will not clean up must not fail the listing");

    assert!(
        listed.iter().any(|process| process.id == live.id),
        "the running record must still be visible"
    );
    assert!(
        !listed.iter().any(|process| process.id == settled.id),
        "the settled record must not be reported as running"
    );
    std::fs::remove_dir_all(&runtime_root).unwrap();
}

#[test]
fn retired_supervisor_state_is_purged_before_workflow_evaluation() {
    let target_root = std::env::temp_dir().join(format!(
        "refine-retired-supervisor-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&target_root).unwrap();
    let initialized = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&target_root)
        .status()
        .unwrap();
    assert!(initialized.success());
    let refine_dir = prepare_refine_dir(&target_root).unwrap();
    std::fs::create_dir_all(&refine_dir).unwrap();
    let runtime_root = target_root.join("run/8082");
    for name in ["supervisor-agent.json", "supervisor-agent.lock"] {
        std::fs::write(refine_dir.join(name), "{}\n").unwrap();
    }

    retire_legacy_supervisor(&runtime_root, &target_root).unwrap();

    assert!(!refine_dir.join("supervisor-agent.json").exists());
    assert!(!refine_dir.join("supervisor-agent.lock").exists());
    std::fs::remove_dir_all(target_root).unwrap();
}

#[test]
fn runner_specs_create_real_runner_processes() {
    let spec = project_sync_worker_spec(
        Path::new("/opt/refine"),
        Path::new("/tmp/run/8082"),
        Path::new("/tmp/app"),
        "OP1",
    );
    assert_eq!(spec.owner, ProcessOwner::Runner);
    assert_eq!(spec.metadata["kind"], "runner");
    assert_eq!(spec.metadata["worker_kind"], PROJECT_SYNC_RUNNER);
    assert!(spec.args.iter().any(|arg| arg == "--operation-id"));
    assert_eq!(
        spec.limits
            .as_ref()
            .map(|limits| limits.kill_on_parent_exit),
        Some(true)
    );

    let jira_spec =
        jira_export_worker_spec(Path::new("/opt/refine"), Path::new("/tmp/run/8082"), "OP2");
    assert_eq!(jira_spec.owner, ProcessOwner::Runner);
    assert_eq!(jira_spec.metadata["worker_kind"], JIRA_EXPORT_RUNNER);
    assert_eq!(jira_spec.metadata["operation_id"], "OP2");
    assert!(!jira_spec.args.iter().any(|arg| arg == "--target-root"));
    assert_eq!(
        jira_spec
            .limits
            .as_ref()
            .map(|limits| limits.kill_on_parent_exit),
        Some(false)
    );
}

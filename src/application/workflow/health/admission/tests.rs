use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::infrastructure::storage::automation::AutomationStore;
fn fixture() -> (PathBuf, PathBuf, FileWorkItemService) {
    let root =
        std::env::temp_dir().join(format!("refine-admission-health-{}", uuid::Uuid::new_v4()));
    let target = root.join("target");
    std::fs::create_dir_all(&target).unwrap();
    let items = FileWorkItemService::new(target.join(".refine"));
    items
        .create_goal_summary("Waiting Goal", Some("GOAL1"))
        .unwrap();
    items
        .append_goal_round_summary("GOAL1", "Reporter", "Request")
        .unwrap();
    items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    (root, target, items)
}
#[test]
fn continuous_eligibility_requires_fresh_observation_and_resets_for_pause_and_missing_skills() {
    let (root, target, items) = fixture();
    let now = chrono::Utc::now().timestamp_millis();
    let mut snapshot = sample_admission(&root, &target, None, now - 31_000).unwrap();
    assert_eq!(snapshot.waiting_count(now - 31_000), 0);
    for i in 1..=31 {
        snapshot =
            sample_admission(&root, &target, Some(&snapshot), now - 31_000 + i * 1000).unwrap();
    }
    assert_eq!(snapshot.waiting_count(now), 1);
    let registry =
        crate::application::projects::registry::FileProjectRegistryService::new(&root, None);
    let mut apps = registry.load().unwrap();
    apps.active_app = Some(target.display().to_string());
    registry.save(&apps).unwrap();
    std::fs::write(
        root.join("workflow-admission.json"),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    assert!(read_admission(&root, Some(&target), now).is_some());
    assert!(read_admission(&root, Some(&target), now + 3000).is_none());
    assert!(read_admission(&root, Some(&root), now).is_none());
    let next = crate::application::guidance::FileNextActionsService::with_runtime_root(
        &items.refine_dir,
        &root,
    )
    .next_response()
    .unwrap();
    assert!(
        next["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["id"] != "all-quiet")
    );
    assert!(
        next["suggestions"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("1 continuously eligible")
    );
    FileProcessSupervisor::new(&root)
        .set_workflow_paused(true)
        .unwrap();
    snapshot = sample_admission(&root, &target, Some(&snapshot), now + 1000).unwrap();
    assert_eq!(snapshot.cause, "paused");
    assert!(snapshot.eligible_since_ms.is_empty());
    FileProcessSupervisor::new(&root)
        .set_workflow_paused(false)
        .unwrap();
    let store = AutomationStore::new(&items.refine_dir);
    let config = store.load().unwrap();
    store
        .update(config.revision, |config| {
            config
                .events
                .get_mut("workflow.plan.enter")
                .unwrap()
                .scope
                .node_id = Some("another-node".into());
            Ok(())
        })
        .unwrap();
    let snapshot = sample_admission(&root, &target, Some(&snapshot), now + 2000).unwrap();
    assert!(
        snapshot
            .cause
            .contains("missing enabled blocking plan Skill")
    );
    assert!(snapshot.eligible_since_ms.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn feature_order_and_gaps_do_not_look_like_stalled_admission() {
    let (root, target, items) = fixture();
    items
        .create_feature_summary("Ordered", Some("FEAT1"), None, None, None)
        .unwrap();
    items.assign_goal_to_feature("FEAT1", "GOAL1").unwrap();
    items.order_goal_in_feature("FEAT1", "GOAL1").unwrap();
    items.create_goal_summary("Blocked", Some("GOAL2")).unwrap();
    items
        .append_goal_round_summary("GOAL2", "Reporter", "Request")
        .unwrap();
    items
        .transition_goal_status("GOAL2", GoalStatus::Todo)
        .unwrap();
    items.assign_goal_to_feature("FEAT1", "GOAL2").unwrap();
    items.order_goal_in_feature("FEAT1", "GOAL2").unwrap();
    items
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let snapshot = sample_admission(&root, &target, None, now).unwrap();
    assert!(snapshot.eligible_since_ms.contains_key("GOAL1"));
    assert!(!snapshot.eligible_since_ms.contains_key("GOAL2"));
    assert_eq!(
        snapshot.blocked_goals["GOAL2"],
        "Feature ordering blocks this Goal"
    );
    items.cancel_goal_summary("GOAL1").unwrap();
    let first = sample_admission(&root, &target, None, now).unwrap();
    assert!(first.eligible_since_ms.contains_key("GOAL2"));
    let after_gap = sample_admission(&root, &target, Some(&first), now + 31_000).unwrap();
    assert_eq!(after_gap.waiting_count(now + 31_000), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn retained_failure_claim_and_legacy_fence_cannot_veto_new_round_in_health_or_admission() {
    let (root, target, items) = fixture();
    let engine = WorkflowEngine::with_target_root(&root, &target);
    let (round, revision, prompt) = items.authored_goal_commitment("GOAL1").unwrap();
    let authority = items
        .claim_workflow_attempt("GOAL1", GoalStatus::Todo, round, revision, &prompt)
        .unwrap();
    items
        .settle_workflow_attempt_failure(
            "GOAL1",
            authority,
            "implementation",
            "Retained failure",
            "2026-09-11T00:00:00Z",
        )
        .unwrap();
    let failed_round = items.show_goal_detail("GOAL1").unwrap()["rounds"][0].clone();
    use sha2::{Digest, Sha256};
    let key = format!("{}:GOAL1", target.display());
    let directory = root.join("workflow-failure-fences");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{:x}.json", Sha256::digest(key.as_bytes())));
    let legacy = serde_json::json!({"round_idx":0,"workflow_revision":revision});
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    items
        .append_goal_round_summary("GOAL1", "Reporter", "Recover using the retained work")
        .unwrap();
    for restart in [false, true] {
        let replacement = WorkflowEngine::with_target_root(&root, &target);
        if restart {
            assert_eq!(replacement.recover_interrupted_goals("restart").unwrap(), 0);
        }
        let snapshot =
            sample_admission(&root, &target, None, chrono::Utc::now().timestamp_millis()).unwrap();
        assert!(snapshot.eligible_since_ms.contains_key("GOAL1"));
        assert!(snapshot.blocked_goals.is_empty());
        assert_eq!(
            replacement.launchable_goals(&BTreeSet::new()).unwrap(),
            vec!["GOAL1"]
        );
    }
    assert_eq!(
        items.show_goal_detail("GOAL1").unwrap()["rounds"][0],
        failed_round
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
        legacy
    );
    assert!(
        engine
            .unresolved_workflow_outcome("GOAL1", &items.show_goal_detail("GOAL1").unwrap())
            .unwrap()
            .is_none()
    );
    // A damaged historical marker is diagnostic evidence, not authority to stop
    // the newly authorized Round or prevent health from describing it.
    let damaged = directory.join(format!("{:x}.json", Sha256::digest(key.as_bytes())));
    std::fs::write(&damaged, b"{damaged historical marker").unwrap();
    let replacement = WorkflowEngine::with_target_root(&root, &target);
    assert_eq!(replacement.recover_interrupted_goals("restart").unwrap(), 0);
    assert_eq!(
        replacement.launchable_goals(&BTreeSet::new()).unwrap(),
        vec!["GOAL1"]
    );
    assert!(
        sample_admission(&root, &target, None, chrono::Utc::now().timestamp_millis())
            .unwrap()
            .eligible_since_ms
            .contains_key("GOAL1")
    );
    assert_eq!(
        std::fs::read(&damaged).unwrap(),
        b"{damaged historical marker"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn health_and_admission_expose_the_same_unpersisted_outcome_until_superseded() {
    let (root, target, items) = fixture();
    let engine = WorkflowEngine::with_target_root(&root, &target);
    let goal = items.show_goal_detail("GOAL1").unwrap();
    let directory = root.join("workflow-failures");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("unpersisted.json");
    let evidence = serde_json::json!({"goal_id":"GOAL1", "target_root":target, "round_idx":0,
        "generation":goal["event_generation"], "original_error":"Provider completion failed",
        "settlement":{"unpersisted_evidence":"Goal filesystem unavailable"}});
    std::fs::write(&path, serde_json::to_vec(&evidence).unwrap()).unwrap();
    let snapshot =
        sample_admission(&root, &target, None, chrono::Utc::now().timestamp_millis()).unwrap();
    assert!(snapshot.eligible_since_ms.is_empty());
    assert_eq!(
        snapshot.blocked_goals["GOAL1"],
        engine
            .unresolved_workflow_outcome("GOAL1", &goal)
            .unwrap()
            .unwrap()
    );
    assert!(
        engine
            .launchable_goals(&BTreeSet::new())
            .unwrap()
            .is_empty()
    );
    items.cancel_goal_summary("GOAL1").unwrap();
    items.undo_goal_summary("GOAL1").unwrap();
    let snapshot =
        sample_admission(&root, &target, None, chrono::Utc::now().timestamp_millis()).unwrap();
    assert!(snapshot.eligible_since_ms.contains_key("GOAL1"));
    assert_eq!(
        engine.launchable_goals(&BTreeSet::new()).unwrap(),
        vec!["GOAL1"]
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),
        evidence
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn restoring_a_legacy_marker_after_same_round_retry_cannot_resurrect_its_restriction() {
    let (root, target, items) = fixture();
    let engine = WorkflowEngine::with_target_root(&root, &target);
    let (round, revision, prompt) = items.authored_goal_commitment("GOAL1").unwrap();
    items
        .claim_workflow_attempt("GOAL1", GoalStatus::Todo, round, revision, &prompt)
        .unwrap();
    use sha2::{Digest, Sha256};
    let path = root.join("workflow-failure-fences").join(format!(
        "{:x}.json",
        Sha256::digest(format!("{}:GOAL1", target.display()).as_bytes())
    ));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes =
        serde_json::to_vec(&serde_json::json!({"round_idx":round,"workflow_revision":revision}))
            .unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let before = items.show_goal_detail("GOAL1").unwrap();
    assert!(
        engine
            .unresolved_workflow_outcome("GOAL1", &before)
            .unwrap()
            .is_some()
    );
    items
        .control_workflow(
            "GOAL1",
            &crate::application::work_items::WorkflowControl {
                to: GoalStatus::Todo,
                reason: "Explicit retry".into(),
                context: String::new(),
                expected_revision: before["workflow_revision"].as_u64().unwrap(),
                request_id: "retry-once".into(),
                actor: "Operator".into(),
                force: false,
                invocation_id: None,
            },
        )
        .unwrap();
    // Simulate copying an old runtime backup after the new workflow decision.
    std::fs::write(&path, &bytes).unwrap();
    let replacement = WorkflowEngine::with_target_root(&root, &target);
    assert_eq!(
        replacement
            .recover_interrupted_goals("restart after restore")
            .unwrap(),
        0
    );
    assert!(
        replacement
            .unresolved_workflow_outcome("GOAL1", &items.show_goal_detail("GOAL1").unwrap())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        replacement.launchable_goals(&BTreeSet::new()).unwrap(),
        vec!["GOAL1"]
    );
    assert_eq!(
        items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_attempt_authority"],
        before["rounds"][0]["workflow_attempt_authority"]
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn late_legacy_failure_cannot_veto_a_superseding_node_decision_after_restart() {
    let (root, target, items) = fixture();
    let (round, revision, prompt) = items.authored_goal_commitment("GOAL1").unwrap();
    items
        .claim_workflow_attempt("GOAL1", GoalStatus::Todo, round, revision, &prompt)
        .unwrap();
    let directory = root.join("workflow-failures");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("legacy-late.json");
    let bytes = serde_json::to_vec(&serde_json::json!({
        "goal_id":"GOAL1", "round_idx":round, "workflow_revision":revision,
        "failure_at":"2099-01-01T00:00:00Z", "original_error":"Late legacy worker failure",
        "settlement":{"unpersisted_evidence":"Goal write failed"}
    }))
    .unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let engine = WorkflowEngine::with_target_root(&root, &target);
    assert!(
        engine
            .launchable_goals(&BTreeSet::new())
            .unwrap()
            .is_empty()
    );
    crate::application::fleet::nodes::FileNodeRegistryService::new(&items.refine_dir)
        .create("worker")
        .unwrap();
    items.transfer_goal_to_node("worker", "GOAL1").unwrap();
    FileWorkItemService::for_node(&items.refine_dir, "worker")
        .transfer_goal_to_node("default", "GOAL1")
        .unwrap();
    // Restoring or receiving legacy evidence after the decision cannot make its
    // late timestamp outrank the workflow's recorded revision and occurrence.
    std::fs::write(&path, &bytes).unwrap();
    let replacement = WorkflowEngine::with_target_root(&root, &target);
    assert_eq!(replacement.recover_interrupted_goals("restart").unwrap(), 0);
    assert_eq!(
        replacement.launchable_goals(&BTreeSet::new()).unwrap(),
        vec!["GOAL1"]
    );
    assert!(
        sample_admission(&root, &target, None, chrono::Utc::now().timestamp_millis())
            .unwrap()
            .eligible_since_ms
            .contains_key("GOAL1")
    );
    assert_eq!(std::fs::read(path).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn exited_worker_snapshot_cannot_keep_ghost_capacity_after_replacement() {
    use crate::infrastructure::process::subprocess::{
        ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
    };
    let (root, target, _) = fixture();
    let supervisor = FileProcessSupervisor::new(&root);
    let token = uuid::Uuid::new_v4().to_string();
    let worker = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Runner,
            command: "/bin/sleep".into(),
            args: vec!["60".into()],
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(serde_json::json!({"worker_kind":"workflow",
            "workflow_incarnation":token, "isolated_process_group":true}))
            .unwrap(),
        })
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    SchedulerObservation {
        runtime_root: root.canonicalize().unwrap(),
        process_id: worker.id.clone(),
        pid: worker.pid.unwrap(),
        os_identity: current_os_identity(worker.pid.unwrap()).unwrap().unwrap(),
        incarnation: token,
        target_root: Some(target.clone()),
        node_id: Some("default".into()),
        sequence: 1,
        tick_ms: now,
        tick_monotonic_ms: None,
        completed_cycle_monotonic_ms: None,
        completed_cycle_ms: Some(now),
        active_attempts: BTreeSet::from(["GOAL1".into()]),
        failure: None,
        retry_delays: Default::default(),
    }
    .write(&root)
    .unwrap();
    assert!(
        sample_admission(&root, &target, None, now)
            .unwrap()
            .active_work
    );
    let group = supervisor.owned_groups().unwrap().remove(0);
    supervisor
        .stop_owned_group(&group, Duration::from_secs(2))
        .unwrap();
    let after_exit = sample_admission(&root, &target, None, now + 1000).unwrap();
    assert!(!after_exit.active_work);
    assert!(after_exit.eligible_since_ms.contains_key("GOAL1"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn admission_uses_the_pinned_occurrence_requirement_after_configuration_changes() {
    let (root, target, items) = fixture();
    let events = crate::application::events::FileEventService::new(&items.refine_dir);
    let detail = items.show_goal_detail("GOAL1").unwrap();
    let generation = detail["event_generation"].as_u64().unwrap_or(0);
    let source = "workflow.plan.enter";
    let pinned = events
        .gate_configuration("GOAL1", 0, "default", source, || Ok(()))
        .unwrap();
    assert!(
        crate::application::events::gate_configuration::has_blocking_workflow_skill(
            &pinned, "default", source
        )
    );
    let current = events.config().unwrap();
    AutomationStore::new(&items.refine_dir)
        .update(current.revision, |config| {
            for event in config
                .events
                .values_mut()
                .filter(|e| e.source.as_deref() == Some(source))
            {
                event.bindings.clear();
            }
            Ok(())
        })
        .unwrap();
    let observed =
        sample_admission(&root, &target, None, chrono::Utc::now().timestamp_millis()).unwrap();
    assert!(observed.eligible_since_ms.contains_key("GOAL1"));
    assert_eq!(
        items.show_goal_detail("GOAL1").unwrap()["event_generation"]
            .as_u64()
            .unwrap_or(0),
        generation
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn every_executable_step_is_monitored_for_missing_execution_ownership() {
    for status in [
        GoalStatus::Todo,
        GoalStatus::Plan,
        GoalStatus::Implement,
        GoalStatus::Quality,
        GoalStatus::Governance,
    ] {
        let (root, target, items) = fixture();
        for step in [
            GoalStatus::Plan,
            GoalStatus::Implement,
            GoalStatus::Quality,
            GoalStatus::Governance,
        ] {
            if items.show_goal_detail("GOAL1").unwrap()["status"] == status.as_str() {
                break;
            }
            items.advance_automated_goal_status("GOAL1", step).unwrap();
        }
        let now = chrono::Utc::now().timestamp_millis();
        let first = sample_admission(&root, &target, None, now).unwrap();
        assert!(
            first.eligible_since_ms.contains_key("GOAL1"),
            "{status:?}: {first:?}"
        );
        let mut observed = first;
        for tick in 1..=31 {
            observed =
                sample_admission(&root, &target, Some(&observed), now + tick * 1000).unwrap();
        }
        assert_eq!(observed.waiting_count(now + 31_000), 1, "{status:?}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn supervisor_reconciles_unowned_work_even_when_scheduler_ticks_are_fresh() {
    use crate::infrastructure::process::subprocess::{
        ManagedProcessSpec, ProcessOwner, ProcessSupervisor,
    };
    let (root, target, _) = fixture();
    let supervisor = FileProcessSupervisor::new(&root);
    let token = uuid::Uuid::new_v4().to_string();
    let worker = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Runner,
            command: "/bin/sleep".into(),
            args: vec!["60".into()],
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(serde_json::json!({"worker_kind":"workflow",
            "workflow_incarnation":token, "isolated_process_group":true}))
            .unwrap(),
        })
        .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    SchedulerObservation {
        runtime_root: root.canonicalize().unwrap(),
        process_id: worker.id.clone(),
        pid: worker.pid.unwrap(),
        os_identity: current_os_identity(worker.pid.unwrap()).unwrap().unwrap(),
        incarnation: token,
        target_root: Some(target.clone()),
        node_id: Some("default".into()),
        sequence: 1,
        tick_ms: now,
        tick_monotonic_ms: None,
        completed_cycle_monotonic_ms: None,
        completed_cycle_ms: Some(now),
        active_attempts: BTreeSet::new(),
        failure: None,
        retry_delays: Default::default(),
    }
    .write(&root)
    .unwrap();

    let registry =
        crate::application::projects::registry::FileProjectRegistryService::new(&root, None);
    let mut apps = registry.load().unwrap();
    apps.active_app = Some(target.display().to_string());
    registry.save(&apps).unwrap();
    let mut admission = sample_admission(&root, &target, None, now).unwrap();
    admission
        .eligible_since_ms
        .insert("GOAL1".into(), now - 31_000);
    std::fs::write(
        root.join("workflow-admission.json"),
        serde_json::to_vec(&admission).unwrap(),
    )
    .unwrap();
    let health = super::super::assess_worker(&root, &worker, Some(&target));
    assert_eq!(health.state, "admission_stalled", "{health:?}");
    // A stale observation from an older scheduler must not condemn its replacement.
    admission.scheduler_incarnation = Some("previous-incarnation".into());
    std::fs::write(
        root.join("workflow-admission.json"),
        serde_json::to_vec(&admission).unwrap(),
    )
    .unwrap();
    assert!(super::super::assess_worker(&root, &worker, Some(&target)).healthy);
    admission.scheduler_incarnation = workflow_incarnation(&worker);
    std::fs::write(
        root.join("workflow-admission.json"),
        serde_json::to_vec(&admission).unwrap(),
    )
    .unwrap();
    let service = crate::application::workers::FileRunnerWorkerService::new(&root);
    assert!(service.ensure_background_worker("workflow").is_err());
    let recovery: Value =
        serde_json::from_slice(&std::fs::read(root.join("workflow-recovery.json")).unwrap())
            .unwrap();
    assert_eq!(recovery["worker"]["id"], worker.id);
    assert_eq!(recovery["stopped"], true);
    assert!(!FileProcessSupervisor::process_is_alive(&worker).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

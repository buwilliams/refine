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
    assert!(snapshot.eligible_since_ms.is_empty());
    assert_eq!(snapshot.cause, "scope or ordering blocks");
    items.cancel_goal_summary("GOAL1").unwrap();
    let first = sample_admission(&root, &target, None, now).unwrap();
    assert!(first.eligible_since_ms.contains_key("GOAL2"));
    let after_gap = sample_admission(&root, &target, Some(&first), now + 31_000).unwrap();
    assert_eq!(after_gap.waiting_count(now + 31_000), 0);
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

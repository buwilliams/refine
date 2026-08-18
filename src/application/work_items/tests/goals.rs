use super::*;

#[test]
fn a_nested_projection_cache_still_resolves_the_real_active_node() {
    let temp_root = unique_temp_dir("work-item-nested-cache-active-node");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8082");
    let nodes = crate::application::fleet::nodes::FileNodeRegistryService::with_active_root(
        &refine_dir,
        &runtime_root,
    );
    nodes.create("bo2lnxnevo03-buddy").unwrap();
    nodes.activate("bo2lnxnevo03-buddy").unwrap();

    // A goal owned by that Node, as automation would have created it.
    let owned = FileWorkItemService::for_node(&refine_dir, "bo2lnxnevo03-buddy")
        .create_goal_summary("Owned by the active node", Some("GOAL1"))
        .unwrap();
    assert_eq!(owned.goal.node_id.as_deref(), Some("bo2lnxnevo03-buddy"));

    // A nested cache directory two levels below the runtime root.
    let nested_cache = runtime_root.join("cache/projections").join("worker123");
    let service =
        FileWorkItemService::with_projection_cache(&refine_dir, &runtime_root, &nested_cache);

    // Ownership must resolve against runtime_root/active-node.json, not against a
    // file that would have to sit next to the nested cache directory.
    assert!(!nested_cache.join("active-node.json").exists());
    service
        .transition_goal_status(&owned.goal.id, GoalStatus::Todo)
        .unwrap();
    assert_eq!(
        service
            .show_goal_summary(&owned.goal.id)
            .unwrap()
            .goal
            .status,
        GoalStatus::Todo
    );

    fs::remove_dir_all(temp_root).unwrap_or(());
}

#[test]
fn file_work_item_service_creates_and_lists_goal_json() {
    let temp_root = unique_temp_dir("work-item-create");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);

    let goal = service
        .create_goal_summary("Created from Rust", Some("GOAL1"))
        .unwrap();
    assert_eq!(goal.goal.id, "GOAL1");
    assert_eq!(goal.goal.status, GoalStatus::Backlog);
    assert!(refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert_eq!(service.list_goal_summaries().unwrap().len(), 1);
    assert_eq!(
        service.show_goal_summary("GOAL1").unwrap().goal.name,
        "Created from Rust"
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn retrying_a_failed_goal_clears_the_recorded_failure_reason() {
    let temp_root = unique_temp_dir("work-item-retry-clears-failure");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Stale candidate", Some("GOAL1"))
        .unwrap();
    service
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    service
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Plan)
        .unwrap();
    service
        .advance_automated_goal_status("GOAL1", GoalStatus::Failed)
        .unwrap();
    service
        .update_latest_goal_round_evaluation_summary(
            "GOAL1",
            &json!({
                "failure_category": "merge",
                "failure_message": "Candidate abc123 is stale: recorded base def456 is not its ancestor",
                "failure_at": "2026-07-25T12:02:30Z"
            }),
        )
        .unwrap();

    let failed = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(failed["rounds"][0]["failure_category"], "merge");
    assert!(
        failed["rounds"][0]["failure_message"]
            .as_str()
            .unwrap_or_default()
            .contains("is stale")
    );

    // A retry reuses this same round, so the reason it carries is now spent.
    // Leaving it would show a live failure on work that has moved on.
    service.retry_goal_governance_summary("GOAL1").unwrap();

    let retried = service.show_goal_detail("GOAL1").unwrap();
    assert_eq!(retried["status"], "governance");
    assert_eq!(retried["rounds"][0]["failure_category"], "");
    assert_eq!(retried["rounds"][0]["failure_message"], "");
    assert_eq!(retried["rounds"][0]["failure_at"], "");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_work_item_service_uses_active_node_and_rejects_foreign_mutations() {
    let temp_root = unique_temp_dir("work-item-node-ownership");
    let refine_dir = temp_root.join(".refine");
    let nodes = crate::application::fleet::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("remote-node").unwrap();
    nodes.activate("remote-node").unwrap();

    let service = FileWorkItemService::new(&refine_dir);
    let local_goal = service
        .create_goal_summary("Remote-owned", Some("GOAL1"))
        .unwrap();
    assert_eq!(local_goal.goal.node_id.as_deref(), Some("remote-node"));
    let local_feature = service
        .create_feature_summary("Remote feature", Some("FEA1"), None, None, None)
        .unwrap();
    assert_eq!(
        local_feature.feature.node_id.as_deref(),
        Some("remote-node")
    );

    nodes.activate("default").unwrap();
    let err = service
        .update_goal_metadata_summary("GOAL1", Some("Blocked"), None, None, None)
        .unwrap_err();
    assert_eq!(err.category(), crate::error::ErrorCategory::Conflict);
    let err = service
        .update_feature_metadata_summary("FEA1", Some("Blocked"), None, None, None)
        .unwrap_err();
    assert_eq!(err.category(), crate::error::ErrorCategory::Conflict);

    service
        .bulk_transfer_goals_to_node(
            "default",
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string()]),
                ..Default::default()
            },
        )
        .unwrap();
    let updated = service
        .update_goal_metadata_summary("GOAL1", Some("Default-owned"), None, None, None)
        .unwrap();
    assert_eq!(updated.goal.name, "Default-owned");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_spreads_eligible_goals_evenly_across_nodes() {
    let temp_root = unique_temp_dir("distribute-spread");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let nodes = crate::application::fleet::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("node-a").unwrap();
    nodes.create("node-b").unwrap();
    for index in 1..=6 {
        service
            .create_goal_summary(&format!("Goal {index}"), Some(&format!("GOAL{index}")))
            .unwrap();
    }

    let targets = vec![
        "default".to_string(),
        "node-a".to_string(),
        "node-b".to_string(),
    ];
    let result = service
        .distribute_goals_across_nodes(&targets, false, false)
        .unwrap();

    assert_eq!(result.strategy, "spread");
    assert_eq!(result.eligible, 6);
    let mut counts = std::collections::BTreeMap::new();
    for goal in service.list_goal_summaries().unwrap() {
        let owner = goal.goal.node_id.unwrap_or_else(|| "default".to_string());
        *counts.entry(owner).or_insert(0usize) += 1;
    }
    assert_eq!(counts.get("default"), Some(&2));
    assert_eq!(counts.get("node-a"), Some(&2));
    assert_eq!(counts.get("node-b"), Some(&2));
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn distribute_converge_moves_only_reviewable_goals_to_review_node() {
    let temp_root = unique_temp_dir("distribute-converge");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let nodes = crate::application::fleet::nodes::FileNodeRegistryService::new(&refine_dir);
    nodes.create("worker").unwrap();
    service
        .create_goal_summary("Reviewable", Some("GOAL1"))
        .unwrap();
    service
        .create_goal_summary("Still backlog", Some("GOAL2"))
        .unwrap();
    service.transfer_goal_to_node("worker", "GOAL1").unwrap();
    service.transfer_goal_to_node("worker", "GOAL2").unwrap();
    // Review is a workflow-owned state; write it directly for the fixture.
    let goal_path = refine_dir.join("goals/GO/AL1/goal.json");
    let updated = fs::read_to_string(&goal_path)
        .unwrap()
        .replace("\"backlog\"", "\"review\"");
    fs::write(&goal_path, updated).unwrap();

    let targets = vec!["default".to_string()];
    let result = service
        .distribute_goals_across_nodes(&targets, true, false)
        .unwrap();

    assert_eq!(result.strategy, "converge");
    assert_eq!(result.moved, 1);
    assert_eq!(result.moves[0].goal_id, "GOAL1");
    assert_eq!(result.moves[0].to_node_id, "default");
    let backlog_goal = service.show_goal_summary("GOAL2").unwrap();
    assert_eq!(backlog_goal.goal.node_id.as_deref(), Some("worker"));
    fs::remove_dir_all(temp_root).unwrap();
}

// Every Goal mutation used to take one lock covering the whole target
// application, so two unrelated Goals could not be written at the same time
// however much capacity the host had — a fleet running several agents
// serialized all of their bookkeeping against each other. Locking is per record
// now, and this is decisive: with a Goal's lock held, mutating other Goals still
// proceeds, where under the old lock none of them could.
#[test]
fn holding_one_goal_does_not_block_mutating_others() {
    use crate::infrastructure::process::supervisor::coordination::acquire_record_lock;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let temp_root = unique_temp_dir("goal-lock-independence");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Held", Some("GOALHELD"))
        .unwrap();
    let others = (0..8)
        .map(|index| format!("GOALFREE{index:02}"))
        .collect::<Vec<_>>();
    for id in &others {
        service.create_goal_summary("Free", Some(id)).unwrap();
    }

    // The lock is re-entrant per thread, so it has to be held from this one
    // while another thread does the mutating.
    let held = acquire_record_lock(&refine_dir, "GOALHELD").unwrap();

    let (tx, rx) = mpsc::channel();
    let worker_refine_dir = refine_dir.clone();
    let worker_others = others.clone();
    let worker = std::thread::spawn(move || {
        let service = FileWorkItemService::new(&worker_refine_dir);
        for id in worker_others {
            let started = Instant::now();
            let outcome =
                service.update_goal_metadata_summary(&id, Some("Renamed"), None, None, None);
            let _ = tx.send((outcome.is_ok(), started.elapsed()));
        }
    });

    // Comfortably shorter than the lock acquisition deadline, so a Goal that
    // was actually blocked cannot be counted as free.
    let budget = Duration::from_secs(5);
    let mut proceeded = 0;
    for _ in 0..others.len() {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok((true, elapsed)) if elapsed < budget => proceeded += 1,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    drop(held);
    worker.join().unwrap();

    // Records are striped rather than given a lock each, so one of these may
    // share a stripe with the held Goal. Under a single global lock none would
    // have proceeded at all.
    assert!(
        proceeded >= others.len() - 1,
        "only {proceeded} of {} unrelated Goals proceeded while one Goal was held",
        others.len()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

// Narrowing the lock must not weaken what it protected. Writers of the same
// Goal still serialize, so a read-modify-write cannot lose an update to a
// concurrent one.
#[test]
fn concurrent_writers_of_one_goal_do_not_lose_updates() {
    let temp_root = unique_temp_dir("goal-lock-same-record");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_goal_summary("Contended", Some("GOALSAME"))
        .unwrap();

    const WRITERS: usize = 6;
    const NOTES_EACH: usize = 4;
    let workers = (0..WRITERS)
        .map(|writer| {
            let refine_dir = refine_dir.clone();
            std::thread::spawn(move || {
                let service = FileWorkItemService::new(&refine_dir);
                for note in 0..NOTES_EACH {
                    service
                        .add_goal_note_summary(
                            "GOALSAME",
                            "tester",
                            &format!("writer {writer} note {note}"),
                        )
                        .unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }

    // Every append survives: none was overwritten by a writer that had read the
    // Goal before it landed.
    let detail = service.show_goal_detail("GOALSAME").unwrap();
    let notes = detail
        .get("notes")
        .and_then(|notes| notes.as_array())
        .expect("notes array");
    assert_eq!(notes.len(), WRITERS * NOTES_EACH);
    for writer in 0..WRITERS {
        for note in 0..NOTES_EACH {
            let needle = format!("writer {writer} note {note}");
            assert!(
                notes.iter().any(|entry| {
                    entry.get("body").and_then(|body| body.as_str()) == Some(needle.as_str())
                }),
                "lost update: {needle}"
            );
        }
    }

    fs::remove_dir_all(temp_root).unwrap();
}

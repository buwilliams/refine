use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::infrastructure::storage::automation::{read_json, write_json};
use crate::model::workflow::{GoalStatus, WorkflowControl};

fn assign(work: &FileWorkItemService, id: &str, to: GoalStatus) {
    let goal = work.show_goal_detail(id).unwrap();
    work.control_workflow(
        id,
        &WorkflowControl {
            to,
            reason: "Recovery fixture decision".into(),
            context: String::new(),
            expected_revision: goal["workflow_revision"].as_u64().unwrap_or(0),
            request_id: uuid::Uuid::new_v4().to_string(),
            force: true,
            actor: "Operator".into(),
            invocation_id: None,
        },
    )
    .unwrap();
}

#[test]
fn deleted_terminal_dispatch_queue_is_rebuilt_without_replaying_completed_hooks() {
    let fixture = Fixture::new();
    fixture.repository();
    let service = fixture.service();
    let _smoke = super::super::test_support::SmokeSkill::install(&service, &fixture.0);
    add_gate(&service, "workflow.cancelled.enter", false);
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Retain delivery intent", Some("RECOVERHOOK"))
        .unwrap();
    work.append_goal_round_summary("RECOVERHOOK", "Reporter", "Do work")
        .unwrap();
    assign(&work, "RECOVERHOOK", GoalStatus::Cancelled);
    let goal = work.show_goal_detail("RECOVERHOOK").unwrap();
    let pending = goal["pending_event_dispatches"].as_object().unwrap();
    assert_eq!(pending.len(), 1);
    let (dispatch_key, payload) = pending.iter().next().unwrap();
    assert_eq!(
        payload["occurrence"],
        *goal["workflow_events"].as_array().unwrap().last().unwrap()
    );
    let pinned_prompt = payload["config"]["skills"]["gate"]["prompt"].clone();
    let queue = service.refine_dir.join("automation/occurrences/default");
    std::fs::remove_dir_all(&queue).unwrap();
    let mut config = (*service.config().unwrap()).clone();
    config.skills.get_mut("gate").unwrap().prompt = "Changed after the decision".into();
    crate::infrastructure::storage::automation::AutomationStore::new(&service.refine_dir)
        .update(config.revision, |stored| {
            *stored = config;
            Ok(())
        })
        .unwrap();

    let restarted = fixture.service();
    restarted.dispatch_goal_events(&fixture.0).unwrap();
    let restored: serde_json::Value =
        read_json(&queue.join(format!("{dispatch_key}.json"))).unwrap();
    assert_eq!(
        restored["config"]["skills"]["gate"]["prompt"],
        pinned_prompt
    );
    let runs = restarted.goal_invocations("RECOVERHOOK", 0, 100).unwrap();
    let id = runs["items"][0]["id"].as_str().unwrap();
    let invocation = restarted.invocation(id).unwrap();
    assert_eq!(
        invocation.bindings[0].skill.prompt,
        pinned_prompt.as_str().unwrap()
    );
    let completed = restarted
        .execute(id, || restarted.validate_manual_authority(&invocation))
        .unwrap();
    assert_eq!(completed.state, InvocationState::Succeeded, "{completed:?}");
    restarted.dispatch_goal_events(&fixture.0).unwrap();
    restarted.dispatch_goal_events(&fixture.0).unwrap();
    let settled = work.show_goal_detail("RECOVERHOOK").unwrap();
    assert!(
        settled["pending_event_dispatches"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert_eq!(settled["status"], "cancelled");
    // A stale delivery marker cannot recreate execution even if its retained
    // invocation artifact is subsequently removed during history cleanup.
    std::fs::remove_file(restarted.invocation_path(id).unwrap()).unwrap();
    write_json(&queue.join(format!("{dispatch_key}.json")), payload).unwrap();
    restarted.dispatch_goal_events(&fixture.0).unwrap();
    assert!(!restarted.invocation_path(id).unwrap().exists());
    assert!(!queue.join(format!("{dispatch_key}.json")).exists());
    assert_eq!(work.show_goal_detail("RECOVERHOOK").unwrap(), settled);
}

#[test]
fn lost_transition_queue_and_corrupt_sibling_do_not_block_valid_goal() {
    let fixture = Fixture::new();
    let service = fixture.service();
    add_gate(&service, "workflow.backlog.exit", true);
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Recover pending transition", Some("RECOVEREDGE"))
        .unwrap();
    work.append_goal_round_summary("RECOVEREDGE", "Reporter", "Do work")
        .unwrap();
    assert!(
        work.transition_goal_status("RECOVEREDGE", GoalStatus::Todo)
            .is_err()
    );
    let pending = work.show_goal_detail("RECOVEREDGE").unwrap()["pending_event_transition"].clone();
    let queue = service.refine_dir.join("automation/transitions/default");
    std::fs::remove_dir_all(&queue).unwrap();
    std::fs::create_dir_all(&queue).unwrap();
    std::fs::write(queue.join("000-corrupt.json"), "{not-json").unwrap();
    let restarted = fixture.service();
    restarted.dispatch_goal_events(&fixture.0).unwrap();
    let goal = work.show_goal_detail("RECOVEREDGE").unwrap();
    assert_eq!(goal["status"], "failed");
    assert_eq!(goal["event_transition_history"][0]["id"], pending["id"]);
    assert!(
        service
            .refine_dir
            .join("automation/occurrence-errors/000-corrupt.json")
            .exists()
    );
    assert_eq!(
        goal["event_transition_history"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn delayed_occurrence_on_existing_workspace_cannot_follow_new_decision() {
    let fixture = Fixture::new();
    let service = fixture.service();
    let _ = service.config().unwrap();
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Late occurrence", Some("OLDHOOK"))
        .unwrap();
    work.append_goal_round_summary("OLDHOOK", "Reporter", "Do work")
        .unwrap();
    let goal = work.show_goal_detail("OLDHOOK").unwrap();
    let mut context = service
        .manual_context(&fixture.0, &json!({"goal_id":"OLDHOOK"}))
        .unwrap();
    context.data["occurrence"] = goal["workflow_events"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let invocation = EventInvocation {
        id: "oldhook".into(),
        event: custom_event(),
        config_revision: 1,
        context,
        bindings: vec![],
        state: InvocationState::Pending,
        results: BTreeMap::new(),
        attempts: vec![],
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
        error: None,
        action_applied: false,
    };
    service.save_invocation(&invocation).unwrap();
    service.validate_manual_authority(&invocation).unwrap();
    assign(&work, "OLDHOOK", GoalStatus::Todo);
    let error = service.validate_manual_authority(&invocation).unwrap_err();
    assert!(error.to_string().contains("superseded"), "{error}");
}

#[test]
fn deleted_error_queue_and_corrupt_sibling_preserve_pending_outcome_delivery() {
    let fixture = Fixture::new();
    let service = fixture.service();
    add_gate(&service, "workflow.backlog.exit", true);
    let mut config = (*service.config().unwrap()).clone();
    let mut error_skill = config.skills["gate"].clone();
    error_skill.id = "error-gate".into();
    config.skills.insert(error_skill.id.clone(), error_skill);
    let mut error_binding = config.events["workflow.backlog.exit"].bindings[0].clone();
    error_binding.id = "error-gate".into();
    error_binding.skill_id = "error-gate".into();
    config
        .events
        .get_mut("workflow.backlog.error")
        .unwrap()
        .bindings
        .push(error_binding);
    crate::infrastructure::storage::automation::AutomationStore::new(&service.refine_dir)
        .update(config.revision, |stored| {
            *stored = config;
            Ok(())
        })
        .unwrap();
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Recover error delivery", Some("RECOVERERROR"))
        .unwrap();
    work.append_goal_round_summary("RECOVERERROR", "Reporter", "Do work")
        .unwrap();
    assert!(
        work.transition_goal_status("RECOVERERROR", GoalStatus::Todo)
            .is_err()
    );
    service.dispatch_goal_events(&fixture.0).unwrap();
    let goal = work.show_goal_detail("RECOVERERROR").unwrap();
    assert_eq!(goal["pending_workflow_outcome"]["state"], "pending");
    let outcome_id = goal["pending_workflow_outcome"]["id"].clone();
    let queue = service.refine_dir.join("automation/outcomes/default");
    std::fs::remove_dir_all(&queue).unwrap();
    std::fs::create_dir_all(&queue).unwrap();
    std::fs::write(queue.join("000-corrupt.json"), "{not-json").unwrap();
    fixture.service().dispatch_outcomes(&fixture.0).unwrap();
    let settled = work.show_goal_detail("RECOVERERROR").unwrap();
    assert_eq!(settled["status"], "failed");
    assert_eq!(settled["pending_workflow_outcome"]["id"], outcome_id);
    assert_eq!(settled["pending_workflow_outcome"]["state"], "failed");
}

#[test]
fn malformed_delivery_batch_cannot_starve_goal_behind_it() {
    let fixture = Fixture::new();
    let service = fixture.service();
    add_gate(&service, "workflow.backlog.exit", true);
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Do not starve behind corruption", Some("FAIRDELIVERY"))
        .unwrap();
    work.append_goal_round_summary("FAIRDELIVERY", "Reporter", "Do work")
        .unwrap();
    assert!(
        work.transition_goal_status("FAIRDELIVERY", GoalStatus::Todo)
            .is_err()
    );
    let queue = service.refine_dir.join("automation/transitions/default");
    for index in 0..128 {
        std::fs::write(
            queue.join(format!("000-corrupt-{index:03}.json")),
            "{not-json",
        )
        .unwrap();
    }
    service.dispatch_goal_events(&fixture.0).unwrap();
    assert_eq!(
        work.show_goal_detail("FAIRDELIVERY").unwrap()["status"],
        "backlog"
    );
    service.dispatch_goal_events(&fixture.0).unwrap();
    assert_eq!(
        work.show_goal_detail("FAIRDELIVERY").unwrap()["status"],
        "failed"
    );
}

#[test]
fn accepted_decision_survives_delivery_index_write_failure() {
    let fixture = Fixture::new();
    fixture.repository();
    let service = fixture.service();
    add_gate(&service, "workflow.cancelled.enter", false);
    let work = FileWorkItemService::new(&service.refine_dir);
    work.create_goal_summary("Commit before delivery", Some("DURABLEDECISION"))
        .unwrap();
    work.append_goal_round_summary("DURABLEDECISION", "Reporter", "Do work")
        .unwrap();
    let queue_root = service.refine_dir.join("automation/occurrences");
    std::fs::write(&queue_root, "Injected derived-index storage failure").unwrap();
    assign(&work, "DURABLEDECISION", GoalStatus::Cancelled);
    let accepted = work.show_goal_detail("DURABLEDECISION").unwrap();
    assert_eq!(accepted["status"], "cancelled");
    assert_eq!(
        accepted["pending_event_dispatches"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_file(&queue_root).unwrap();
    fixture.service().dispatch_goal_events(&fixture.0).unwrap();
    assert_eq!(
        service.goal_invocations("DURABLEDECISION", 0, 100).unwrap()["total"],
        1
    );
    assert_eq!(
        work.show_goal_detail("DURABLEDECISION").unwrap()["status"],
        "cancelled"
    );
}

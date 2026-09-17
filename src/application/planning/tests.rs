use super::*;
struct Fixture {
    path: PathBuf,
    service: FilePlanningService,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("refine-planning-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        let root = path.join(".refine");
        let runtime = path.join("runtime");
        let service = FilePlanningService::new(&root, &path, &runtime).unwrap();
        Self { path, service }
    }
    fn apply(
        &self,
        operation: &str,
        board: Option<&str>,
        lane: Option<&str>,
        goal: Option<&str>,
        revision: Option<u64>,
        data: Value,
    ) -> PlanningAction {
        let command = PlanningCommand {
            request_id: uuid::Uuid::new_v4().to_string(),
            operation: operation.into(),
            expected_revision: revision,
            board_id: board.map(str::to_string),
            lane_id: lane.map(str::to_string),
            goal_id: goal.map(str::to_string),
            actor: "Buddy".into(),
            data,
        };
        let action = self.service.submit(command).unwrap();
        self.service.process_pending().unwrap();
        self.service.action(&action.id).unwrap()
    }
    fn board(&self) -> Board {
        let a = self.apply(
            "board.create",
            None,
            None,
            None,
            None,
            json!({"name":"Shared"}),
        );
        assert_eq!(a.state, "complete", "{:?}", a.message);
        serde_json::from_value(a.result).unwrap()
    }
    fn card(&self, b: &Board) -> String {
        let a = self.apply(
            "card.create",
            Some(&b.id),
            Some(&b.lanes[0].id),
            None,
            None,
            json!({"name":"An idea","reporter":"Buddy"}),
        );
        assert_eq!(a.state, "complete", "{:?}", a.message);
        a.result["goal_id"].as_str().unwrap().into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
#[test]
fn draft_card_is_one_inert_goal_and_personal_done_only_moves_placement() {
    let f = Fixture::new();
    let b = f.board();
    let id = f.card(&b);
    let before = f.service.work().show_goal_detail(&id).unwrap();
    assert_eq!(before["status"], "draft");
    assert_eq!(before["round_count"], 0);
    let a = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(a.state, "complete", "{:?}", a.message);
    let after = f.service.work().show_goal_detail(&id).unwrap();
    assert_eq!(before["workflow_revision"], after["workflow_revision"]);
    assert_eq!(before["status"], after["status"]);
    assert_eq!(before["rounds"], after["rounds"]);
    assert_eq!(f.service.placement(&id).unwrap().lane_id, b.lanes[1].id);
}
#[test]
fn command_retry_is_idempotent_and_rejects_different_input() {
    let f = Fixture::new();
    let command = PlanningCommand {
        request_id: "request-one".into(),
        operation: "board.create".into(),
        expected_revision: None,
        board_id: None,
        lane_id: None,
        goal_id: None,
        actor: "Buddy".into(),
        data: json!({"name":"Team"}),
    };
    f.service.submit(command.clone()).unwrap();
    f.service.process_pending().unwrap();
    assert_eq!(f.service.submit(command.clone()).unwrap().state, "complete");
    assert_eq!(f.service.records::<Board>("boards").unwrap().len(), 1);
    let mut changed = command;
    changed.data["name"] = json!("Other");
    assert!(f.service.submit(changed).is_err());
}
#[test]
fn competing_moves_conflict_without_rewriting_goal() {
    let f = Fixture::new();
    let b = f.board();
    let id = f.card(&b);
    let a = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(a.state, "complete");
    let stale = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[0].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(stale.state, "failed");
    assert_eq!(f.service.placement(&id).unwrap().lane_id, b.lanes[1].id);
}
#[test]
fn shared_requests_wait_for_current_owner() {
    let f = Fixture::new();
    let b = f.board();
    let id = f.card(&b);
    let mut other = FilePlanningService::new(&f.service.root, &f.path, &f.service.runtime).unwrap();
    other.node = "other".into();
    let command = PlanningCommand {
        request_id: "remote-move".into(),
        operation: "card.move".into(),
        expected_revision: Some(1),
        board_id: Some(b.id.clone()),
        lane_id: Some(b.lanes[1].id.clone()),
        goal_id: Some(id.clone()),
        actor: "Buddy".into(),
        data: json!({}),
    };
    other.submit(command).unwrap();
    assert_eq!(other.process_pending().unwrap(), 0);
    assert_eq!(other.action("remote-move").unwrap().state, "queued");
    f.service.process_pending().unwrap();
    assert_eq!(other.action("remote-move").unwrap().state, "complete");
}
#[test]
fn migration_preserves_completed_tasks_as_drafts_and_replays_safely() {
    let f = Fixture::new();
    write_json(&f.service.root.join("todo-lists.json"),&json!({"lists":[{"id":"old-list","name":"Personal","reporter":"Buddy","created":"2026-01-01","items":[{"id":"old-item","text":"Read paper","done":true,"created":"2026-01-02","updated":"2026-01-03"}]}]})).unwrap();
    for _ in 0..2 {
        let a = f.apply("migrate", None, None, None, None, json!({}));
        assert_eq!(a.state, "complete", "{:?}", a.message)
    }
    let snapshot = f.service.snapshot().unwrap();
    assert_eq!(snapshot["boards"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["cards"].as_array().unwrap().len(), 1);
    let card = &snapshot["cards"][0];
    assert_eq!(card["goal"]["status"], "draft");
    assert_eq!(card["goal"]["created"], "2026-01-02");
    assert!(
        card["placement"]["lane_id"]
            .as_str()
            .unwrap()
            .ends_with("done")
    );
    assert!(card["goal"]["workflow_events"].is_null());
}
#[test]
fn accept_lane_promotes_without_round_or_execution() {
    let f = Fixture::new();
    let b = f.board();
    let id = f.card(&b);
    let update = f.apply(
        "lane.update",
        Some(&b.id),
        Some(&b.lanes[1].id),
        None,
        Some(b.revision),
        json!({"action":"accept_into_backlog"}),
    );
    assert_eq!(update.state, "complete");
    let a = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(a.state, "complete", "{:?}", a.message);
    let goal = f.service.work().show_goal_detail(&id).unwrap();
    assert_eq!(goal["status"], "backlog");
    assert_eq!(goal["round_count"], 0);
}
#[test]
fn release_uses_same_goal_and_one_round_even_after_replay() {
    let f = Fixture::new();
    let b = f.board();
    let id = f.card(&b);
    let update = f.apply(
        "lane.update",
        Some(&b.id),
        Some(&b.lanes[1].id),
        None,
        Some(b.revision),
        json!({"action":"release","routing":f.service.node}),
    );
    assert_eq!(update.state, "complete");
    let a = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(a.state, "complete", "{:?}", a.message);
    let goal = f.service.work().show_goal_detail(&id).unwrap();
    assert_eq!(goal["status"], "todo");
    assert_eq!(goal["round_count"], 1);
    f.service.submit(a.command.clone()).unwrap();
    f.service.process_pending().unwrap();
    assert_eq!(
        f.service.work().show_goal_detail(&id).unwrap()["round_count"],
        1
    );
    let again = f.apply(
        "card.apply",
        Some(&b.id),
        None,
        Some(&id),
        Some(2),
        json!({}),
    );
    assert_eq!(again.state, "complete");
    assert_eq!(
        f.service.work().show_goal_detail(&id).unwrap()["workflow_revision"],
        goal["workflow_revision"]
    );
}
#[test]
fn missing_route_waits_and_cancellation_preserves_accepted_card() {
    let f = Fixture::new();
    let b = f.board();
    let id = f.card(&b);
    f.apply(
        "lane.update",
        Some(&b.id),
        Some(&b.lanes[1].id),
        None,
        Some(b.revision),
        json!({"action":"release"}),
    );
    let a = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(a.state, "waiting");
    assert!(a.message.unwrap().contains("routing"));
    f.service.cancel(&a.id).unwrap();
    f.service.process_pending().unwrap();
    assert_eq!(
        f.service.work().show_goal_detail(&id).unwrap()["status"],
        "backlog"
    );
    assert_eq!(f.service.action(&a.id).unwrap().state, "cancelled");
}
#[cfg(unix)]
#[test]
fn lane_skill_runs_in_managed_workspace_and_gates_the_move() {
    use crate::application::events::{FileEventService, InvocationState, test_support::SmokeSkill};
    use crate::model::automation::{BindingMode, PlanningFilter};
    let f = Fixture::new();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "test@example.test"],
        &["config", "user.name", "Test"],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&f.path)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(f.path.join("base.txt"), "base").unwrap();
    for args in [&["add", "base.txt"][..], &["commit", "-qm", "base"]] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&f.path)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let b = f.board();
    let id = f.card(&b);
    let events = FileEventService::with_runtime_root(&f.service.root, &f.service.runtime);
    let _smoke = SmokeSkill::install(&events, &f.path);
    let mut config = (*events.config().unwrap()).clone();
    let mut skill = config.skills["default-plan"].clone();
    skill.id = "lane-check".into();
    skill.parameters = vec![];
    config.skills.insert(skill.id.clone(), skill);
    let mut binding = config.events["workflow.plan.enter"].bindings[0].clone();
    binding.id = "lane-check".into();
    binding.skill_id = "lane-check".into();
    binding.mode = BindingMode::Blocking;
    binding.inputs.clear();
    binding.planning = Some(PlanningFilter {
        board_id: Some(b.id.clone()),
        lane_id: Some(b.lanes[0].id.clone()),
    });
    config
        .events
        .get_mut("planning.lane.exit")
        .unwrap()
        .bindings
        .push(binding);
    crate::infrastructure::storage::automation::AutomationStore::new(&f.service.root)
        .update(config.revision, |stored| {
            *stored = config;
            Ok(())
        })
        .unwrap();
    let a = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(a.state, "waiting", "{:?}", a.message);
    assert_eq!(f.service.placement(&id).unwrap().lane_id, b.lanes[0].id);
    let invocation = events.invocation(&a.invocations["exit"]).unwrap();
    assert!(invocation.context.lifecycle.is_some());
    let result = events
        .execute(&invocation.id, || {
            events.validate_manual_authority(&invocation)
        })
        .unwrap();
    assert_eq!(result.state, InvocationState::Succeeded, "{result:?}");
    f.service.process_pending().unwrap();
    assert_eq!(f.service.action(&a.id).unwrap().state, "complete");
    assert_eq!(f.service.placement(&id).unwrap().lane_id, b.lanes[1].id);
    assert_eq!(
        f.service.work().show_goal_detail(&id).unwrap()["round_count"],
        0
    );
}

#[test]
fn planning_edit_and_detach_resume_after_durable_side_effects() {
    let f = Fixture::new();
    let board = f.board();
    let id = f.card(&board);
    let revision = f.service.work().show_goal_detail(&id).unwrap()["workflow_revision"]
        .as_u64()
        .unwrap();
    let command = PlanningCommand {
        request_id: "interrupted-edit".into(),
        operation: "card.update".into(),
        expected_revision: Some(1),
        board_id: Some(board.id.clone()),
        lane_id: None,
        goal_id: Some(id.clone()),
        actor: "Buddy".into(),
        data: json!({"name":"Retained edit","expected_goal_revision":revision}),
    };
    let action = f.service.submit(command.clone()).unwrap();
    f.service
        .work()
        .edit_planning_goal(&id, &command.data, &action.id)
        .unwrap();
    let edited_revision =
        f.service.work().show_goal_detail(&id).unwrap()["workflow_revision"].clone();
    f.service.process_action(&action.id).unwrap();
    assert_eq!(f.service.action(&action.id).unwrap().state, "complete");
    assert_eq!(
        f.service.work().show_goal_detail(&id).unwrap()["workflow_revision"],
        edited_revision
    );
    let mut detach = f
        .service
        .submit(PlanningCommand {
            request_id: "interrupted-detach".into(),
            operation: "card.detach".into(),
            expected_revision: Some(2),
            data: json!({}),
            ..command
        })
        .unwrap();
    detach.phase = "detach".into();
    f.service.save_action(&detach).unwrap();
    fs::remove_file(f.service.path("cards", &id).unwrap()).unwrap();
    f.service.process_action(&detach.id).unwrap();
    assert_eq!(f.service.action(&detach.id).unwrap().state, "complete");
    assert_eq!(
        f.service.work().show_goal_detail(&id).unwrap()["name"],
        "Retained edit"
    );
}

#[test]
fn waiting_action_prevents_later_moves_until_explicit_cancellation() {
    let f = Fixture::new();
    let b = f.board();
    let id = f.card(&b);
    f.apply(
        "lane.update",
        Some(&b.id),
        Some(&b.lanes[1].id),
        None,
        Some(b.revision),
        json!({"action":"release"}),
    );
    let waiting = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(waiting.state, "waiting");
    let next = f.apply(
        "card.move",
        Some(&b.id),
        Some(&b.lanes[0].id),
        Some(&id),
        Some(2),
        json!({}),
    );
    assert_eq!(next.state, "waiting");
    assert!(next.invocations.is_empty());
    assert_eq!(f.service.placement(&id).unwrap().lane_id, b.lanes[1].id);
    f.service.cancel(&waiting.id).unwrap();
    f.service.process_pending().unwrap();
    assert_eq!(f.service.action(&next.id).unwrap().state, "complete");
    assert_eq!(f.service.placement(&id).unwrap().lane_id, b.lanes[0].id);
}

#[cfg(unix)]
#[test]
fn draft_release_runs_lifecycle_gates_once_before_creating_round_and_reuses_entry() {
    use crate::application::events::{FileEventService, InvocationState, test_support::SmokeSkill};
    use crate::model::automation::BindingMode;
    let f = Fixture::new();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["config", "user.email", "test@example.test"],
        vec!["config", "user.name", "Test"],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&f.path)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    fs::write(f.path.join("base.txt"), "base").unwrap();
    for args in [vec!["add", "base.txt"], vec!["commit", "-qm", "base"]] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&f.path)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let board = f.board();
    let id = f.card(&board);
    let events = FileEventService::with_runtime_root(&f.service.root, &f.service.runtime);
    let _smoke = SmokeSkill::install(&events, &f.path);
    let mut config = (*events.config().unwrap()).clone();
    let mut skill = config.skills["default-plan"].clone();
    skill.id = "release-check".into();
    skill.parameters.clear();
    config.skills.insert(skill.id.clone(), skill);
    for source in [
        "workflow.draft.exit",
        "workflow.backlog.enter",
        "workflow.backlog.success",
        "workflow.backlog.exit",
        "workflow.todo.enter",
    ] {
        let mut binding = config.events["workflow.plan.enter"].bindings[0].clone();
        binding.id = source.replace('.', "-");
        binding.skill_id = "release-check".into();
        binding.mode = BindingMode::Blocking;
        binding.inputs.clear();
        config
            .events
            .get_mut(source)
            .unwrap()
            .bindings
            .push(binding);
    }
    crate::infrastructure::storage::automation::AutomationStore::new(&f.service.root)
        .update(config.revision, |stored| {
            *stored = config;
            Ok(())
        })
        .unwrap();
    f.apply(
        "lane.update",
        Some(&board.id),
        Some(&board.lanes[1].id),
        None,
        Some(board.revision),
        json!({"action":"release","routing":f.service.node}),
    );
    let action = f.apply(
        "card.move",
        Some(&board.id),
        Some(&board.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(action.state, "waiting");
    let mut executed = std::collections::BTreeMap::<String, usize>::new();
    for _ in 0..20 {
        events.dispatch_goal_events(&f.path).unwrap();
        let history = events.goal_invocations(&id, 0, 100).unwrap();
        for item in history["items"].as_array().unwrap() {
            let invocation = events.invocation(item["id"].as_str().unwrap()).unwrap();
            if !invocation.state.terminal() {
                let source = invocation.event.source.clone().unwrap();
                *executed.entry(source).or_default() += 1;
                let result = events
                    .execute(&invocation.id, || {
                        events.validate_manual_authority(&invocation)
                    })
                    .unwrap();
                assert_eq!(result.state, InvocationState::Succeeded, "{result:?}");
            }
        }
        f.service.process_pending().unwrap();
        let current = f.service.action(&action.id).unwrap();
        assert_ne!(current.state, "failed", "{:?}", current.message);
        if current.terminal() && executed.contains_key("workflow.todo.enter") {
            break;
        }
    }
    assert_eq!(f.service.action(&action.id).unwrap().state, "complete");
    for source in [
        "workflow.draft.exit",
        "workflow.backlog.enter",
        "workflow.backlog.success",
        "workflow.backlog.exit",
        "workflow.todo.enter",
    ] {
        assert_eq!(executed.get(source), Some(&1), "{executed:?}");
    }
    let goal = f.service.work().show_goal_detail(&id).unwrap();
    assert_eq!(goal["status"], "todo");
    assert_eq!(goal["round_count"], 1);
}

#[test]
fn malformed_legacy_store_keeps_migration_retryable() {
    let f = Fixture::new();
    write_json(
        &f.service.root.join("todo-lists.json"),
        &json!({"lists":"damaged"}),
    )
    .unwrap();
    let failed = f.apply("migrate", None, None, None, None, json!({}));
    assert_eq!(failed.state, "failed");
    assert!(!f.service.root.join("planning/migration.json").exists());
    write_json(
        &f.service.root.join("todo-lists.json"),
        &json!({"lists":[]}),
    )
    .unwrap();
    assert_eq!(
        f.apply("migrate", None, None, None, None, json!({})).state,
        "complete"
    );
}

#[test]
fn deleted_goal_placement_can_be_removed_explicitly() {
    let f = Fixture::new();
    let board = f.board();
    let id = f.card(&board);
    f.service.work().delete_goal_record(&id).unwrap();
    assert!(f.service.snapshot().unwrap()["cards"][0]["error"].is_string());
    let result = f.apply(
        "card.detach",
        Some(&board.id),
        None,
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(result.state, "complete", "{:?}", result.message);
    assert!(
        f.service.snapshot().unwrap()["cards"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn release_resumes_after_round_creation_without_creating_another_round() {
    let f = Fixture::new();
    let board = f.board();
    let id = f.card(&board);
    f.apply(
        "lane.update",
        Some(&board.id),
        Some(&board.lanes[1].id),
        None,
        Some(board.revision),
        json!({"action":"release","routing":f.service.node}),
    );
    let mut action = f.apply(
        "card.move",
        Some(&board.id),
        Some(&board.lanes[1].id),
        Some(&id),
        Some(1),
        json!({}),
    );
    assert_eq!(action.state, "complete", "{:?}", action.message);
    f.service
        .work()
        .transition_goal_status(&id, GoalStatus::Backlog)
        .unwrap();
    // Reconstruct the durable boundary immediately after the initial Round write.
    action.state = "running".into();
    action.phase = "materialize".into();
    f.service.save_action(&action).unwrap();
    f.service.process_action(&action.id).unwrap();
    assert_eq!(f.service.action(&action.id).unwrap().state, "complete");
    assert_eq!(
        f.service.work().show_goal_detail(&id).unwrap()["round_count"],
        1
    );
}

#[test]
fn deleting_board_preserves_goals_detaches_cards_and_replays_after_interruption() {
    let f = Fixture::new();
    let board = f.board();
    let goal = f.card(&board);
    let before = f.service.work().show_goal_detail(&goal).unwrap();
    let stale = f.apply(
        "board.delete",
        Some(&board.id),
        None,
        None,
        Some(0),
        json!({}),
    );
    assert_eq!(stale.state, "failed");
    assert!(f.service.board(&board.id).is_ok());
    let deleted = f.apply(
        "board.delete",
        Some(&board.id),
        None,
        None,
        Some(board.revision),
        json!({}),
    );
    assert_eq!(deleted.state, "complete", "{:?}", deleted.message);
    assert!(f.service.board(&board.id).is_err());
    assert!(f.service.placement(&goal).is_err());
    let snapshot = f.service.snapshot().unwrap();
    assert_eq!(snapshot["boards"].as_array().unwrap().len(), 0);
    assert_eq!(snapshot["cards"].as_array().unwrap().len(), 0);
    let after = f.service.work().show_goal_detail(&goal).unwrap();
    assert_eq!(before["status"], after["status"]);
    assert_eq!(before["workflow_revision"], after["workflow_revision"]);
    assert!(after.get("planning").is_none());
    let mut interrupted = deleted.clone();
    interrupted.state = "queued".into();
    f.service.save_action(&interrupted).unwrap();
    f.service.process_action(&interrupted.id).unwrap();
    assert_eq!(f.service.action(&interrupted.id).unwrap().state, "complete");
    let replacement = f.board();
    let attach = f.apply(
        "card.attach",
        Some(&replacement.id),
        Some(&replacement.lanes[0].id),
        Some(&goal),
        None,
        json!({}),
    );
    assert_eq!(attach.state, "complete", "{:?}", attach.message);
    let placement = f.service.placement(&goal).unwrap();
    assert_eq!(placement.board_id, replacement.id);
    assert_eq!(placement.revision, 2);
}

#[test]
fn deleting_board_rejects_pending_card_actions() {
    let f = Fixture::new();
    let board = f.board();
    let pending = f
        .service
        .submit(PlanningCommand {
            request_id: "pending-delete-test".into(),
            operation: "card.create".into(),
            expected_revision: None,
            board_id: Some(board.id.clone()),
            lane_id: Some(board.lanes[0].id.clone()),
            goal_id: None,
            actor: "Buddy".into(),
            data: json!({"name":"Pending","reporter":"Buddy"}),
        })
        .unwrap();
    let deletion = f
        .service
        .submit(PlanningCommand {
            request_id: "blocked-delete-test".into(),
            operation: "board.delete".into(),
            expected_revision: Some(board.revision),
            board_id: Some(board.id.clone()),
            lane_id: None,
            goal_id: None,
            actor: "Buddy".into(),
            data: json!({}),
        })
        .unwrap();
    f.service.process_action(&deletion.id).unwrap();
    let failed = f.service.action(&deletion.id).unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed.message.unwrap().contains("pending board actions"));
    f.service.cancel(&pending.id).unwrap();
    assert_eq!(
        f.apply(
            "board.delete",
            Some(&board.id),
            None,
            None,
            Some(board.revision),
            json!({})
        )
        .state,
        "complete"
    );
}

#[test]
fn lane_settings_and_position_save_atomically() {
    let f = Fixture::new();
    let board = f.board();
    let lane_id = &board.lanes[0].id;
    for position in [json!(-1), json!(0.5), json!(board.lanes.len()), json!("1")] {
        let action = f.apply(
            "lane.update",
            Some(&board.id),
            Some(lane_id),
            None,
            Some(board.revision),
            json!({"name":"Changed", "position":position}),
        );
        assert_eq!(action.state, "failed");
        let snapshot = f.service.snapshot().unwrap();
        assert_eq!(snapshot["boards"][0]["revision"], board.revision);
        assert_eq!(
            snapshot["boards"][0]["lanes"][0]["name"],
            board.lanes[0].name
        );
    }
    let action = f.apply(
        "lane.update",
        Some(&board.id),
        Some(lane_id),
        None,
        Some(board.revision),
        json!({"name":"Ready", "action":"release", "position":1}),
    );
    assert_eq!(action.state, "complete", "{:?}", action.message);
    assert_eq!(action.result["revision"], board.revision + 1);
    assert_eq!(action.result["lanes"][1]["id"], *lane_id);
    assert_eq!(action.result["lanes"][1]["name"], "Ready");
    assert_eq!(action.result["lanes"][1]["action"], "release");
    let stale = f.apply(
        "lane.update",
        Some(&board.id),
        Some(lane_id),
        None,
        Some(board.revision),
        json!({"name":"Stale", "position":0}),
    );
    assert_eq!(stale.state, "failed");
    assert_eq!(
        f.service.snapshot().unwrap()["boards"][0]["lanes"][1]["name"],
        "Ready"
    );
}

#[test]
fn delete_draft_card_is_owned_revision_checked_and_replayable() {
    let f = Fixture::new();
    let board = f.board();
    let id = f.card(&board);
    let goal_revision = f.service.work().show_goal_detail(&id).unwrap()["workflow_revision"]
        .as_u64()
        .unwrap();
    for (placement_revision, revision) in [(0, goal_revision), (1, goal_revision + 1)] {
        let failed = f.apply(
            "card.delete",
            Some(&board.id),
            None,
            Some(&id),
            Some(placement_revision),
            json!({"expected_goal_revision":revision}),
        );
        assert_eq!(failed.state, "failed");
        assert!(f.service.work().show_goal_detail(&id).is_ok());
        assert!(f.service.placement(&id).is_ok());
    }
    let command = PlanningCommand {
        request_id: "delete-draft".into(),
        operation: "card.delete".into(),
        board_id: Some(board.id.clone()),
        lane_id: None,
        goal_id: Some(id.clone()),
        expected_revision: Some(1),
        actor: "Buddy".into(),
        data: json!({"expected_goal_revision":goal_revision}),
    };
    let mut other = FilePlanningService::new(&f.service.root, &f.path, &f.service.runtime).unwrap();
    other.node = "other".into();
    other.submit(command.clone()).unwrap();
    other.process_action(&command.request_id).unwrap();
    assert_eq!(other.action(&command.request_id).unwrap().state, "queued");
    assert!(f.service.work().show_goal_detail(&id).is_ok());
    f.service.process_action(&command.request_id).unwrap();
    assert_eq!(
        f.service.action(&command.request_id).unwrap().state,
        "complete"
    );
    assert!(matches!(
        f.service.work().show_goal_detail(&id),
        Err(RefineError::NotFound(_))
    ));
    assert!(
        f.service.snapshot().unwrap()["cards"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(other.submit(command).unwrap().state, "complete");
}

#[test]
fn draft_deletion_rejects_promoted_goals_and_resumes_after_goal_removal() {
    let f = Fixture::new();
    let board = f.board();
    let id = f.card(&board);
    let goal_revision = f.service.work().show_goal_detail(&id).unwrap()["workflow_revision"]
        .as_u64()
        .unwrap();
    let command = PlanningCommand {
        request_id: "resume-delete".into(),
        operation: "card.delete".into(),
        board_id: Some(board.id.clone()),
        lane_id: None,
        goal_id: Some(id.clone()),
        expected_revision: Some(1),
        actor: "Buddy".into(),
        data: json!({"expected_goal_revision":goal_revision}),
    };
    let mut action = f.service.submit(command.clone()).unwrap();
    action.phase = "delete".into();
    action.before = Some(f.service.placement(&id).unwrap());
    f.service.save_action(&action).unwrap();
    f.service
        .work()
        .delete_draft_goal_record(&id, goal_revision)
        .unwrap();
    f.service.process_action(&action.id).unwrap();
    assert_eq!(f.service.action(&action.id).unwrap().state, "complete");
    assert!(
        f.service.snapshot().unwrap()["cards"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let id = f.card(&board);
    f.service
        .work()
        .set_goal_status_unchecked(&id, &GoalStatus::Backlog)
        .unwrap();
    let revision = f.service.work().show_goal_detail(&id).unwrap()["workflow_revision"].clone();
    let rejected = f.apply(
        "card.delete",
        Some(&board.id),
        None,
        Some(&id),
        Some(1),
        json!({"expected_goal_revision":revision}),
    );
    assert_eq!(rejected.state, "failed");
    assert!(rejected.message.unwrap().contains("Only Draft"));
    assert_eq!(
        f.service.work().show_goal_detail(&id).unwrap()["status"],
        "backlog"
    );
    assert!(f.service.placement(&id).is_ok());
}

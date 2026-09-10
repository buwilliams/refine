use super::*;

fn edit_goal_fixture(f: &Fixture, edit: impl FnOnce(&mut Value)) {
    // Simulate a competing synchronized edit, including edits that retain the revision.
    let summary = f.work().show_goal_summary("FRESH").unwrap();
    let path = f.service.refine_dir.join(summary.goal.json_path);
    let mut value: Value = read_json(&path).unwrap();
    edit(&mut value);
    write_json(&path, &value).unwrap();
}

#[test]
fn changed_round_request_node_transition_and_cancellation_fence_launch_and_cached_settlement() {
    for completed in [false, true] {
        for changed in [
            "round",
            "request",
            "node",
            "transition",
            "revision",
            "cancel",
        ] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            f.gate("workflow.backlog.exit", BindingMode::Blocking);
            let before = f.snapshot();
            f.request_todo();
            f.dispatch();
            let invocation = f.invocation("workflow.backlog.exit");
            if completed {
                assert_eq!(f.execute(&invocation).state, InvocationState::Succeeded);
            }
            let retained = f.service.invocation(&invocation.id).unwrap();
            let launches = fs::read(invocation.context.cwd.join("launches.txt")).ok();
            match changed {
                "round" => edit_goal_fixture(&f, |v| {
                    let round = v["rounds"][0].clone();
                    v["rounds"].as_array_mut().unwrap().push(round);
                }),
                "request" => edit_goal_fixture(&f, |v| {
                    v["rounds"][0]["prompt"] = json!("Changed authored request")
                }),
                "node" => edit_goal_fixture(&f, |v| v["node_id"] = json!("different-node")),
                "transition" => edit_goal_fixture(&f, |v| {
                    v["pending_event_transition"]["requested"]["name"] = json!("Changed request")
                }),
                "revision" => edit_goal_fixture(&f, |v| {
                    v["workflow_revision"] = json!(v["workflow_revision"].as_u64().unwrap() + 1)
                }),
                "cancel" => {
                    f.work().cancel_goal_summary("FRESH").unwrap();
                }
                _ => unreachable!(),
            }
            assert!(
                f.service.execute(&invocation.id, || Ok(())).is_err(),
                "{completed} {changed}"
            );
            f.dispatch();
            assert_ne!(
                f.work().show_goal_detail("FRESH").unwrap()["status"],
                "todo"
            );
            if completed {
                assert_eq!(f.service.invocation(&invocation.id).unwrap(), retained);
            }
            assert_eq!(
                launches,
                fs::read(invocation.context.cwd.join("launches.txt")).ok()
            );
            assert_eq!(before, f.snapshot());
        }
    }
}

#[test]
fn result_acceptance_rechecks_round_and_does_not_accept_a_late_response() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.backlog.exit", BindingMode::Blocking);
    f.request_todo();
    f.dispatch();
    let invocation = f.invocation("workflow.backlog.exit");
    let changed = std::cell::Cell::new(false);
    let result = f.service.execute(&invocation.id, || {
        if invocation.context.cwd.join("launches.txt").exists() && !changed.replace(true) {
            edit_goal_fixture(&f, |v| {
                v["rounds"][0]["prompt"] = json!("New request after provider launch")
            });
        }
        Ok(())
    });
    assert!(result.is_err());
    let retained = f.service.invocation(&invocation.id).unwrap();
    assert_eq!(retained.state, InvocationState::Error);
    assert_eq!(retained.attempts.len(), 1);
    assert!(!retained.results.values().any(|r| r.outcome == "success"));
    f.dispatch();
    assert_ne!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "todo"
    );
}

#[test]
fn cancelled_invocation_stays_terminal_and_retains_its_workspace() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.backlog.exit", BindingMode::Blocking);
    f.request_todo();
    f.dispatch();
    let invocation = f.invocation("workflow.backlog.exit");
    f.service.cancel_invocation(&invocation.id).unwrap();
    let result = f
        .service
        .execute(&invocation.id, || panic!("cancelled work must not launch"))
        .unwrap();
    assert_eq!(result.state, InvocationState::Cancelled);
    assert!(invocation.context.cwd.exists());
    assert!(!invocation.context.cwd.join("launches.txt").exists());
    f.dispatch();
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "backlog"
    );
}

#[test]
fn durable_occurrence_is_rechecked_even_after_queue_index_is_consumed() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.todo.enter", BindingMode::Background);
    f.work()
        .transition_goal_status("FRESH", crate::model::workflow::GoalStatus::Todo)
        .unwrap();
    f.dispatch();
    let invocation = f.invocation("workflow.todo.enter");
    assert!(matches!(
        invocation.context.lifecycle.as_ref().unwrap().authority,
        lifecycle::LifecycleAuthority::Occurrence { .. }
    ));
    edit_goal_fixture(&f, |v| v["workflow_events"] = json!([]));
    assert!(f.service.execute(&invocation.id, || Ok(())).is_err());
    assert!(!invocation.context.cwd.join("launches.txt").exists());
}

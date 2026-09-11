use super::*;

#[test]
fn terminal_success_and_error_handler_failures_do_not_undo_done_or_cancelled() {
    use crate::model::workflow::{GoalStatus, WorkflowControl};
    for status in [GoalStatus::Done, GoalStatus::Cancelled] {
        let f = Fixture::new();
        let _smoke = SmokeSkill::install(&f.service, &f.temp);
        let success = format!("workflow.{}.success", status.as_str());
        let error = format!("workflow.{}.error", status.as_str());
        f.gate(&success, BindingMode::Blocking);
        f.gate(&error, BindingMode::Blocking);
        let script = f.temp.join("lifecycle-smoke.py");
        fs::write(
            &script,
            fs::read_to_string(&script).unwrap().replace(
                "result['outcome'] = 'success'",
                "result['outcome'] = 'failure'",
            ),
        )
        .unwrap();
        let goal = f.work().show_goal_detail("FRESH").unwrap();
        f.work()
            .control_workflow(
                "FRESH",
                &WorkflowControl {
                    to: status.clone(),
                    reason: "Explicit terminal decision".into(),
                    context: String::new(),
                    expected_revision: goal["workflow_revision"].as_u64().unwrap_or(0),
                    request_id: "terminal".into(),
                    force: true,
                    actor: "Operator".into(),
                    invocation_id: None,
                },
            )
            .unwrap();
        f.dispatch();
        f.dispatch();
        assert_eq!(
            f.execute(&f.invocation(&success)).state,
            InvocationState::Failed
        );
        f.dispatch();
        assert_eq!(
            f.work().show_goal_detail("FRESH").unwrap()["status"],
            status.as_str()
        );
        f.service.dispatch_outcomes(&f.primary).unwrap();
        let handler = f.invocation(&error);
        assert_eq!(f.execute(&handler).state, InvocationState::Failed);
        f.service.dispatch_outcomes(&f.primary).unwrap();
        f.service.dispatch_outcomes(&f.primary).unwrap();
        assert_eq!(
            f.work().show_goal_detail("FRESH").unwrap()["status"],
            status.as_str()
        );
        assert_eq!(f.service.invocation(&handler.id).unwrap().attempts.len(), 1);
    }
}

#[test]
fn required_success_runs_before_a_manual_step_can_depart() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.backlog.success", BindingMode::Blocking);
    f.request_todo();
    f.dispatch();
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "backlog"
    );
    let invocation = f.invocation("workflow.backlog.success");
    assert_eq!(f.execute(&invocation).state, InvocationState::Succeeded);
    f.dispatch();
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "todo"
    );
}

#[test]
fn required_success_failure_opens_error_handling_and_only_an_explicit_decision_redirects() {
    for redirect in [false, true] {
        let f = Fixture::new();
        let _smoke = SmokeSkill::install(&f.service, &f.temp);
        f.gate("workflow.backlog.success", BindingMode::Blocking);
        f.gate("workflow.backlog.error", BindingMode::Blocking);
        let script = f.temp.join("lifecycle-smoke.py");
        fs::write(&script, fs::read_to_string(&script).unwrap().replace(
            "result['outcome'] = 'success'",
            "result['outcome'] = 'failure' if result['binding_id'].endswith('-success-gate') else 'success'",
        )).unwrap();
        f.request_todo();
        f.dispatch();
        assert_eq!(
            f.execute(&f.invocation("workflow.backlog.success")).state,
            InvocationState::Failed
        );
        f.dispatch();
        let goal = f.work().show_goal_detail("FRESH").unwrap();
        assert_eq!(goal["status"], "backlog");
        assert_eq!(goal["pending_workflow_outcome"]["state"], "pending");
        f.service.dispatch_outcomes(&f.primary).unwrap();
        let handler = f.invocation("workflow.backlog.error");
        assert_eq!(
            handler.context.data["outcome"]["id"],
            goal["pending_workflow_outcome"]["id"]
        );
        if redirect {
            let request = crate::model::workflow::WorkflowControl {
                to: crate::model::workflow::GoalStatus::Plan,
                reason: "Operator supplied recovery context".into(),
                context: "Resolve the source finding".into(),
                expected_revision: goal["workflow_revision"].as_u64().unwrap_or(0),
                request_id: "operator-redirect".into(),
                force: false,
                actor: "Operator".into(),
                invocation_id: None,
            };
            f.work().control_workflow("FRESH", &request).unwrap();
            assert!(
                f.service
                    .execute(&handler.id, || panic!("superseded handler must not launch"))
                    .is_err()
            );
        } else {
            assert_eq!(f.execute(&handler).state, InvocationState::Succeeded);
        }
        f.service.dispatch_outcomes(&f.primary).unwrap();
        let settled = f.work().show_goal_detail("FRESH").unwrap();
        assert_eq!(settled["status"], if redirect { "todo" } else { "failed" });
        assert_eq!(
            settled["rounds"].as_array().unwrap().len(),
            if redirect { 2 } else { 1 }
        );
    }
}

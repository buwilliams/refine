//! Complete pinned results are required across scheduler, manual and recovered gates.
use super::*;
use crate::application::workflow::engine::behaviors::{WorkflowTodo, contract::WorkflowBehavior};
use crate::model::workflow::GoalStatus;

#[test]
fn terminal_findings_and_background_verdicts_preserve_gate_semantics() {
    for mode in [BindingMode::Blocking, BindingMode::Background] {
        for outcome in ["failure", "error", "cancelled"] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            f.gate("workflow.todo.enter", mode.clone());
            f.work()
                .transition_goal_status("FRESH", GoalStatus::Todo)
                .unwrap();
            f.dispatch();
            let entry = f.invocation("workflow.todo.enter");
            let retained = if outcome == "cancelled" {
                f.service.cancel_invocation(&entry.id).unwrap()
            } else {
                let script = f.temp.join("lifecycle-smoke.py");
                fs::write(
                    &script,
                    fs::read_to_string(&script).unwrap().replace(
                        "result['outcome'] = 'success'",
                        &format!("result['outcome'] = '{outcome}'"),
                    ),
                )
                .unwrap();
                f.execute(&entry)
            };
            let before = f.snapshot();
            let launches = fs::read(entry.context.cwd.join("launches.txt")).ok();
            let bytes = fs::read(f.service.invocation_path(&entry.id).unwrap()).unwrap();
            let runtime = f.temp.join("runtime");
            let mut ctx = super::workflow::claimed_context(&f, &runtime);
            let result = WorkflowTodo.advance(&mut ctx);
            if mode == BindingMode::Blocking {
                assert!(result.is_err(), "{outcome}: {result:?}");
                assert_eq!(
                    f.work().show_goal_detail("FRESH").unwrap()["status"],
                    "todo"
                );
                f.assert_no_candidate();
            } else {
                result.unwrap();
                assert_eq!(
                    f.work().show_goal_detail("FRESH").unwrap()["status"],
                    "plan"
                );
            }
            assert_eq!(retained, f.service.invocation(&entry.id).unwrap());
            assert_eq!(
                bytes,
                fs::read(f.service.invocation_path(&entry.id).unwrap()).unwrap()
            );
            assert_eq!(
                launches,
                fs::read(entry.context.cwd.join("launches.txt")).ok()
            );
            assert!(entry.context.cwd.exists());
            assert_eq!(before, f.snapshot());
        }
    }
}

#[test]
fn settlement_rechecks_required_entry_records_after_later_skills_complete() {
    for defect in ["missing-result", "missing-binding", "workspace", "state"] {
        let f = Fixture::new();
        let _smoke = SmokeSkill::install(&f.service, &f.temp);
        f.gate("workflow.todo.enter", BindingMode::Blocking);
        f.gate("workflow.todo.exit", BindingMode::Blocking);
        f.work()
            .transition_goal_status("FRESH", GoalStatus::Todo)
            .unwrap();
        f.dispatch();
        let entry = f.execute(&f.invocation("workflow.todo.enter"));
        let script = f.temp.join("lifecycle-smoke.py");
        let change = match defect {
            "missing-result" => "record['results'].clear()",
            "missing-binding" => "record['bindings'].clear(); record['results'].clear()",
            "workspace" => "record['context']['workspace']['registration'] = None",
            "state" => "record['state'] = 'cancelled'",
            _ => unreachable!(),
        };
        let replacement = format!(
            "if execution['binding_id'] == 'workflow-todo-exit-gate':\n p = pathlib.Path({})\n record = json.loads(p.read_text())\n {change}\n p.write_text(json.dumps(record))\nresult['outcome'] = 'success'",
            serde_json::to_string(&f.service.invocation_path(&entry.id).unwrap()).unwrap(),
        );
        fs::write(
            &script,
            fs::read_to_string(&script)
                .unwrap()
                .replace("result['outcome'] = 'success'", &replacement),
        )
        .unwrap();
        let before = f.snapshot();
        let launches = fs::read(entry.context.cwd.join("launches.txt")).unwrap();
        let runtime = f.temp.join("runtime");
        let mut ctx = super::workflow::claimed_context(&f, &runtime);
        let error = WorkflowTodo.advance(&mut ctx).unwrap_err().to_string();
        assert!(error.contains(&entry.id), "{defect}: {error}");
        assert_eq!(
            f.invocation("workflow.todo.exit").state,
            InvocationState::Succeeded
        );
        let goal = f.work().show_goal_detail("FRESH").unwrap();
        assert_eq!(goal["status"], "todo");
        assert!(
            !f.service
                .refine_dir
                .join("automation/approvals")
                .join(format!(
                    "{}.json",
                    crate::application::events::transitions::edge_key(&goal, "plan")
                ))
                .exists()
        );
        assert_eq!(
            launches,
            fs::read(entry.context.cwd.join("launches.txt")).unwrap()
        );
        assert!(entry.context.cwd.join("skill-output.txt").exists());
        f.assert_no_candidate();
        assert_eq!(before, f.snapshot());
    }
}

#[test]
fn cancelled_partial_blocking_results_cannot_admit_todo_after_restart() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.todo.enter", BindingMode::Blocking);
    f.second_gate("workflow.todo.enter");
    f.work()
        .transition_goal_status("FRESH", GoalStatus::Todo)
        .unwrap();
    f.dispatch();
    let entry = f.invocation("workflow.todo.enter");
    let before = f.snapshot();
    let result = f.service.execute(&entry.id, || {
        if f.service.invocation(&entry.id)?.results.len() == 1 {
            f.service.cancel_invocation(&entry.id)?;
        }
        Ok(())
    });
    assert!(result.is_err());
    let cancelled = f.service.invocation(&entry.id).unwrap();
    assert_eq!(cancelled.state, InvocationState::Cancelled);
    assert_eq!(cancelled.results.len(), 1);
    assert_eq!(cancelled.attempts.len(), 1);
    let launches = fs::read(entry.context.cwd.join("launches.txt")).unwrap();
    let runtime = f.temp.join("runtime");
    let restarted = FileEventService::with_runtime_root(&f.service.refine_dir, &runtime);
    restarted.dispatch_goal_events(&f.primary).unwrap();
    assert_eq!(
        restarted
            .execute(&entry.id, || panic!("terminal invocation relaunched"))
            .unwrap(),
        cancelled
    );
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "failed"
    );
    assert_eq!(restarted.invocation(&entry.id).unwrap(), cancelled);
    assert_eq!(
        launches,
        fs::read(entry.context.cwd.join("launches.txt")).unwrap()
    );
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "failed"
    );
    f.assert_no_candidate();
    assert_eq!(before, f.snapshot());
}

#[test]
fn incomplete_or_invalid_retained_success_cannot_settle_scheduler_or_manual_gates() {
    for scheduler in [false, true] {
        for defect in ["missing", "identity", "role", "outcome"] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            let source = if scheduler {
                "workflow.todo.enter"
            } else {
                "workflow.backlog.exit"
            };
            f.gate(source, BindingMode::Blocking);
            f.second_gate(source);
            if scheduler {
                f.work()
                    .transition_goal_status("FRESH", GoalStatus::Todo)
                    .unwrap();
            } else {
                f.request_todo();
            }
            f.dispatch();
            let entry = f.execute(&f.invocation(source));
            assert_eq!(entry.state, InvocationState::Succeeded);
            let mut incomplete = entry.clone();
            match defect {
                "missing" => {
                    incomplete.results.remove("second");
                }
                "identity" => {
                    incomplete.results.get_mut("second").unwrap().invocation_id = "other".into()
                }
                "role" => incomplete.results.get_mut("second").unwrap().role = "plan".into(),
                "outcome" => incomplete
                    .results
                    .get_mut("second")
                    .unwrap()
                    .outcome
                    .clear(),
                "failure" => {
                    incomplete.results.get_mut("second").unwrap().outcome = "failure".into()
                }
                _ => unreachable!(),
            }
            f.service.save_invocation(&incomplete).unwrap();
            let before = f.snapshot();
            let launches = fs::read(entry.context.cwd.join("launches.txt")).unwrap();
            let bytes = fs::read(f.service.invocation_path(&entry.id).unwrap()).unwrap();
            let runtime = f.temp.join("runtime");
            let restarted = FileEventService::with_runtime_root(&f.service.refine_dir, &runtime);
            let error = restarted
                .execute(&entry.id, || Ok(()))
                .unwrap_err()
                .to_string();
            assert!(
                error.contains(&entry.id) && error.contains("second"),
                "{defect}: {error}"
            );
            if defect == "missing" {
                assert!(
                    error.contains("missing required completion result"),
                    "{error}"
                );
            }
            if scheduler {
                let mut ctx = super::workflow::claimed_context(&f, &runtime);
                assert!(WorkflowTodo.advance(&mut ctx).is_err());
            } else {
                restarted.dispatch_goal_events(&f.primary).unwrap();
            }
            assert_eq!(
                f.work().show_goal_detail("FRESH").unwrap()["status"],
                if scheduler { "todo" } else { "failed" }
            );
            assert_eq!(
                bytes,
                fs::read(f.service.invocation_path(&entry.id).unwrap()).unwrap()
            );
            assert_eq!(
                launches,
                fs::read(entry.context.cwd.join("launches.txt")).unwrap()
            );
            assert!(entry.context.cwd.join("skill-output.txt").exists());
            f.assert_no_candidate();
            assert_eq!(before, f.snapshot());
        }
    }
}

#[test]
fn forced_cancellation_does_not_require_readable_entry_gate_evidence() {
    let f = Fixture::new();
    f.gate("workflow.todo.enter", BindingMode::Blocking);
    f.work()
        .transition_goal_status("FRESH", GoalStatus::Todo)
        .unwrap();
    let summary = f.work().show_goal_summary("FRESH").unwrap();
    let path = f.service.refine_dir.join(summary.goal.json_path);
    let mut goal: Value = read_json(&path).unwrap();
    for value in goal["rounds"][0]["gate_configurations"]
        .as_object_mut()
        .unwrap()
        .values_mut()
    {
        *value = json!({"invalid": "retained configuration"});
    }
    write_json(&path, &goal).unwrap();
    let before = f.snapshot();
    f.work().cancel_goal_summary("FRESH").unwrap();
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "cancelled"
    );
    assert_eq!(before, f.snapshot());
    f.assert_no_candidate();
}

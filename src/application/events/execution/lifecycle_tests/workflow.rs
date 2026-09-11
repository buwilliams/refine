//! Scheduler-owned Todo admission must run lifecycle Skills before creating the candidate.
use super::*;
use crate::application::workflow::engine::{
    behaviors::{WorkflowTodo, contract::WorkflowBehavior},
    context::WorkflowContext,
};
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::model::workflow::GoalStatus;

#[test]
fn cancelled_todo_entry_rejects_scheduler_admission_without_materializing_implementation() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.todo.enter", BindingMode::Blocking);
    f.work()
        .transition_goal_status("FRESH", GoalStatus::Todo)
        .unwrap();
    f.assert_no_candidate();
    f.dispatch();
    let entry = f.invocation("workflow.todo.enter");
    let cancelled = f.service.cancel_invocation(&entry.id).unwrap();
    assert_eq!(cancelled.state, InvocationState::Cancelled);
    let before = f.snapshot();
    let runtime = f.temp.join("runtime");
    let mut ctx = claimed_context(&f, &runtime);
    for restart in [false, true, true] {
        f.dispatch();
        if restart {
            let restarted = FileEventService::with_runtime_root(&f.service.refine_dir, &runtime);
            restarted.dispatch_goal_events(&f.primary).unwrap();
            assert_eq!(
                restarted
                    .execute(&entry.id, || panic!("cancelled invocation relaunched"))
                    .unwrap(),
                cancelled
            );
        }
        let result = WorkflowTodo.advance(&mut ctx);
        assert!(
            result.is_err(),
            "cancelled blocking Entry admitted implementation: {result:?}"
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("no longer authorizes todo"), "{error}");
        let goal = f.work().show_goal_detail("FRESH").unwrap();
        assert_eq!(goal["status"], "failed");
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
        assert_eq!(f.service.invocation(&entry.id).unwrap(), cancelled);
        assert!(entry.context.cwd.exists());
        assert!(!entry.context.cwd.join("launches.txt").exists());
        f.assert_no_candidate();
        assert_eq!(before, f.snapshot());
    }
}

#[test]
fn authored_todo_start_runs_lifecycle_skills_before_materializing_the_implementation() {
    for source in ["workflow.todo.enter", "workflow.todo.exit"] {
        for queued_entry in ["absent", "pending", "completed"] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            if queued_entry != "absent" {
                f.gate(source, BindingMode::Blocking);
            }
            f.work()
                .transition_goal_status("FRESH", GoalStatus::Todo)
                .unwrap();
            if queued_entry == "absent" {
                f.gate(source, BindingMode::Blocking);
            } else {
                f.dispatch();
                if queued_entry == "completed" && source.ends_with(".enter") {
                    f.execute(&f.invocation(source));
                }
            }
            let before = f.snapshot();
            f.assert_no_candidate();
            let runtime = f.temp.join("runtime");
            let mut ctx = claimed_context(&f, &runtime);
            WorkflowTodo.advance(&mut ctx).unwrap();
            let goal = f.work().show_goal_detail("FRESH").unwrap();
            assert_eq!(goal["status"], "plan", "{source} queued={queued_entry}");
            let invocation = f.invocation(source);
            assert_eq!(invocation.state, InvocationState::Succeeded);
            assert_eq!(invocation.attempts.len(), 1);
            let owner = invocation.context.lifecycle.as_ref().unwrap();
            let implementation = PathBuf::from(ctx.worktree_path.as_ref().unwrap());
            assert_ne!(owner.path, implementation);
            assert_eq!(owner.source_commit, f.base);
            assert!(owner.path.join("skill-output.txt").exists());
            assert!(!implementation.join("skill-output.txt").exists());
            assert_eq!(git(&implementation, &["rev-parse", "HEAD"]), f.base);
            assert_eq!(before, f.snapshot());
        }
    }
}

#[test]
fn claimed_todo_lifecycle_rechecks_superseded_claims_and_cancellation_on_resume() {
    for completed in [false, true] {
        for change in ["claim", "cancel"] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            f.gate("workflow.todo.exit", BindingMode::Blocking);
            f.work()
                .transition_goal_status("FRESH", GoalStatus::Todo)
                .unwrap();
            let before = f.snapshot();
            let goal = f.work().show_goal_detail("FRESH").unwrap();
            let revision = goal["workflow_revision"].as_u64().unwrap();
            f.work()
                .claim_workflow_attempt(
                    "FRESH",
                    GoalStatus::Todo,
                    0,
                    revision,
                    goal["rounds"][0]["prompt"].as_str().unwrap(),
                )
                .unwrap();
            let mut context = f
                .service
                .manual_context(&f.primary, &json!({"goal_id":"FRESH"}))
                .unwrap();
            context.workflow_revision = Some(revision);
            context.data["context"] = json!({"destination":"plan"});
            let invocation = f
                .service
                .prepare(
                    "workflow.todo.exit",
                    context,
                    BTreeMap::new(),
                    "claimed-todo",
                )
                .unwrap();
            if completed {
                f.execute(&invocation);
            }
            let retained = f.service.invocation(&invocation.id).unwrap();
            let launches = fs::read(invocation.context.cwd.join("launches.txt")).ok();
            if change == "cancel" {
                f.work().cancel_goal_summary("FRESH").unwrap();
            } else {
                let summary = f.work().show_goal_summary("FRESH").unwrap();
                let path = f.service.refine_dir.join(summary.goal.json_path);
                let mut goal: Value = read_json(&path).unwrap();
                goal["rounds"][0]["workflow_attempt_authority"]["workflow_revision"] =
                    json!(revision + 1);
                write_json(&path, &goal).unwrap();
            }
            let restarted =
                FileEventService::with_runtime_root(&f.service.refine_dir, f.temp.join("runtime"));
            assert!(restarted.execute(&invocation.id, || Ok(())).is_err());
            if completed {
                assert_eq!(restarted.invocation(&invocation.id).unwrap(), retained);
            }
            assert_eq!(
                launches,
                fs::read(invocation.context.cwd.join("launches.txt")).ok()
            );
            f.assert_no_candidate();
            assert_eq!(before, f.snapshot());
        }
    }
}

pub(super) fn claimed_context<'a>(
    f: &'a Fixture,
    runtime: &'a std::path::Path,
) -> WorkflowContext<'a> {
    let goal = f.work().show_goal_detail("FRESH").unwrap();
    let authority = f
        .work()
        .claim_workflow_attempt(
            "FRESH",
            GoalStatus::Todo,
            0,
            goal["workflow_revision"].as_u64().unwrap(),
            goal["rounds"][0]["prompt"].as_str().unwrap(),
        )
        .unwrap();
    let settings = FileSettingsService::with_active_root(&f.service.refine_dir, runtime)
        .load()
        .unwrap();
    WorkflowContext::new(
        runtime,
        &f.primary,
        "FRESH".into(),
        "default".into(),
        "smoke-ai".into(),
        0,
        authority,
        settings,
        f.work(),
    )
}

#[test]
fn todo_admission_rechecks_entry_workspace_after_exit_skills_complete() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.todo.enter", BindingMode::Blocking);
    f.gate("workflow.todo.exit", BindingMode::Blocking);
    f.work()
        .transition_goal_status("FRESH", GoalStatus::Todo)
        .unwrap();
    f.dispatch();
    let entry = f.invocation("workflow.todo.enter");
    let entry = f.execute(&entry);
    let retained = f.temp.join("retained-entry-workspace");
    let script = f.temp.join("lifecycle-smoke.py");
    let replacement = format!(
        "if result['binding_id'] == 'workflow-todo-exit-gate':\n pathlib.Path({}).rename({})\nresult['outcome'] = 'success'",
        serde_json::to_string(&entry.context.cwd).unwrap(),
        serde_json::to_string(&retained).unwrap(),
    );
    fs::write(
        &script,
        fs::read_to_string(&script)
            .unwrap()
            .replace("result['outcome'] = 'success'", &replacement),
    )
    .unwrap();
    let before = f.snapshot();
    let runtime = f.temp.join("runtime");
    let mut ctx = claimed_context(&f, &runtime);
    let result = WorkflowTodo.advance(&mut ctx);
    assert!(
        result.is_err(),
        "settlement accepted a missing Entry workspace: {result:?}"
    );
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "todo"
    );
    assert_eq!(f.service.invocation(&entry.id).unwrap(), entry);
    assert!(retained.join("skill-output.txt").exists());
    f.assert_no_candidate();
    assert_eq!(before, f.snapshot());
}

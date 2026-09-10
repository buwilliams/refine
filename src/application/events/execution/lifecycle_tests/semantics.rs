//! End-to-end gate permission and occurrence reuse across both transition adapters.
use super::*;
use crate::application::workflow::engine::behaviors::{WorkflowTodo, contract::WorkflowBehavior};
use crate::application::workflow::gates::GateAssessment;
use crate::infrastructure::storage::automation::AutomationStore;
use crate::model::workflow::GoalStatus;

#[test]
fn successful_blocking_results_allow_scheduler_and_manual_transitions_with_background_faults() {
    for scheduler in [false, true] {
        for outcome in ["failure", "error"] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            let source = if scheduler {
                "workflow.todo.enter"
            } else {
                "workflow.backlog.exit"
            };
            f.gate(source, BindingMode::Blocking);
            f.second_gate(source);
            let revision = f.service.config().unwrap().revision;
            AutomationStore::new(&f.service.refine_dir)
                .update(revision, |config| {
                    config.events.get_mut(source).unwrap().bindings[1].mode =
                        BindingMode::Background;
                    Ok(())
                })
                .unwrap();
            let provider = f.temp.join("lifecycle-smoke.py");
            fs::write(&provider, fs::read_to_string(&provider).unwrap().replace(
                "result['outcome'] = 'success'",
                &format!("result['outcome'] = '{outcome}' if result['binding_id'] == 'second' else 'success'"),
            )).unwrap();
            if scheduler {
                f.work()
                    .transition_goal_status("FRESH", GoalStatus::Todo)
                    .unwrap();
            } else {
                f.request_todo();
            }
            f.dispatch();
            let pending = f.invocation(source);
            let completed = f.execute(&pending);
            assert_eq!(completed.gate_assessment(), GateAssessment::Satisfied);
            assert_eq!(
                completed.execution_state(),
                if outcome == "failure" {
                    InvocationState::Failed
                } else {
                    InvocationState::Error
                }
            );
            assert_eq!(completed.results.len(), 2);
            assert_eq!(completed.attempts.len(), 4);
            assert_eq!(
                fs::read_to_string(completed.context.cwd.join("launches.txt"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
            let before = f.snapshot();
            let original = fs::read(f.service.invocation_path(&completed.id).unwrap()).unwrap();
            if scheduler {
                let runtime = f.temp.join("runtime");
                let mut ctx = super::workflow::claimed_context(&f, &runtime);
                WorkflowTodo.advance(&mut ctx).unwrap();
                assert!(
                    !PathBuf::from(ctx.worktree_path.unwrap())
                        .join("skill-output.txt")
                        .exists()
                );
            } else {
                f.dispatch();
                f.assert_no_candidate();
            }
            assert_eq!(
                f.work().show_goal_detail("FRESH").unwrap()["status"],
                if scheduler { "plan" } else { "todo" }
            );
            assert_eq!(
                original,
                fs::read(f.service.invocation_path(&completed.id).unwrap()).unwrap()
            );
            assert_eq!(before, f.snapshot());
        }
    }
}

#[test]
fn lifecycle_and_scheduler_reuse_occurrence_configuration_after_disabled_skill_edit() {
    for scheduler in [false, true] {
        for dispatch_before_edit in [false, true] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            f.gate("workflow.todo.enter", BindingMode::Blocking);
            let revision = f.service.config().unwrap().revision;
            f.work()
                .transition_goal_status("FRESH", GoalStatus::Todo)
                .unwrap();
            if dispatch_before_edit {
                f.dispatch();
            }
            AutomationStore::new(&f.service.refine_dir)
                .update(revision, |config| {
                    config
                        .events
                        .get_mut("workflow.todo.enter")
                        .unwrap()
                        .bindings
                        .clear();
                    Ok(())
                })
                .unwrap();
            let before = f.snapshot();
            if scheduler {
                let runtime = f.temp.join("runtime");
                let mut ctx = super::workflow::claimed_context(&f, &runtime);
                WorkflowTodo.advance(&mut ctx).unwrap();
            } else {
                let error = f
                    .work()
                    .transition_goal_status("FRESH", GoalStatus::Backlog)
                    .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains(crate::application::events::transitions::PENDING)
                );
                f.dispatch();
                let entry = f.invocation("workflow.todo.enter");
                assert_eq!(f.execute(&entry).state, InvocationState::Succeeded);
                f.dispatch();
            }
            let entry = f.invocation("workflow.todo.enter");
            assert_eq!(entry.config_revision, revision);
            assert_eq!(entry.bindings.len(), 1);
            assert_eq!(entry.state, InvocationState::Succeeded);
            assert_eq!(entry.attempts.len(), 2);
            assert_eq!(
                fs::read_to_string(entry.context.cwd.join("launches.txt"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
            assert_eq!(
                f.work().show_goal_detail("FRESH").unwrap()["status"],
                if scheduler { "plan" } else { "backlog" }
            );
            assert_eq!(before, f.snapshot());
        }
    }
}

#[test]
fn manual_transition_preserves_entry_definition_when_skill_moves_to_exit() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.todo.enter", BindingMode::Blocking);
    let revision = f.service.config().unwrap().revision;
    let skill_id = f.service.config().unwrap().events["workflow.todo.enter"].bindings[0]
        .skill_id
        .clone();
    AutomationStore::new(&f.service.refine_dir)
        .update(revision, |config| {
            config.skills.get_mut(&skill_id).unwrap().prompt = "Original Entry definition".into();
            Ok(())
        })
        .unwrap();
    f.work()
        .transition_goal_status("FRESH", GoalStatus::Todo)
        .unwrap();
    let revision = f.service.config().unwrap().revision;
    AutomationStore::new(&f.service.refine_dir)
        .update(revision, |config| {
            let binding = config
                .events
                .get_mut("workflow.todo.enter")
                .unwrap()
                .bindings
                .remove(0);
            config
                .events
                .get_mut("workflow.todo.exit")
                .unwrap()
                .bindings
                .push(binding);
            config.skills.get_mut(&skill_id).unwrap().prompt = "Updated Exit definition".into();
            Ok(())
        })
        .unwrap();
    let before = f.snapshot();
    assert!(
        f.work()
            .transition_goal_status("FRESH", GoalStatus::Backlog)
            .is_err()
    );
    f.dispatch();
    let entry = f.invocation("workflow.todo.enter");
    let exit = f.invocation("workflow.todo.exit");
    assert_eq!(entry.bindings[0].skill.prompt, "Original Entry definition");
    assert_eq!(exit.bindings[0].skill.prompt, "Updated Exit definition");
    assert_ne!(entry.config_revision, exit.config_revision);
    assert_eq!(f.execute(&entry).state, InvocationState::Succeeded);
    assert_eq!(f.execute(&exit).state, InvocationState::Succeeded);
    f.dispatch();
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "backlog"
    );
    assert_eq!(before, f.snapshot());
    f.assert_no_candidate();
}

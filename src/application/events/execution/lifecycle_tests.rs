//! Real lifecycle Skills before implementation admission, including restart and stale evidence.
use super::*;
use crate::application::events::test_support::SmokeSkill;
use crate::infrastructure::git::worktrees::FileGitWorktreeService;
use crate::infrastructure::storage::automation::{read_json, write_json};
use std::fs;
use std::path::PathBuf;
mod support;
use support::*;

#[test]
fn fresh_goal_backlog_exit_skill_remains_runnable_before_round_materialization() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    let before = f.snapshot();
    f.gate("workflow.backlog.exit", BindingMode::Blocking);
    f.assert_no_candidate();
    f.request_todo();
    f.dispatch();
    let invocation = f.invocation("workflow.backlog.exit");
    assert_eq!(
        invocation.state,
        InvocationState::Pending,
        "{:?}",
        invocation.error
    );
    let owner = invocation.context.lifecycle.as_ref().unwrap();
    assert_eq!(owner.source_commit, f.base);
    assert_ne!(owner.source_commit, git(&f.primary, &["rev-parse", "HEAD"]));
    assert_eq!(
        invocation.context.data["system"]["workspace"],
        json!(invocation.context.cwd)
    );
    let result = f.execute(&invocation);
    assert_eq!(result.state, InvocationState::Succeeded, "{result:?}");
    assert_eq!(result.attempts.len(), 2);
    f.dispatch();
    assert_eq!(
        f.work().show_goal_detail("FRESH").unwrap()["status"],
        "todo"
    );
    f.assert_no_candidate();
    assert_eq!(before, f.snapshot());
}

#[test]
fn preplan_enter_exit_bindings_execute_blocking_and_background_without_candidate_refs() {
    for source in [
        "workflow.backlog.enter",
        "workflow.backlog.exit",
        "workflow.todo.enter",
        "workflow.todo.exit",
    ] {
        for mode in [BindingMode::Blocking, BindingMode::Background] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            let before = f.snapshot();
            let todo_source = source.starts_with("workflow.todo");
            if source == "workflow.todo.exit" {
                f.work()
                    .transition_goal_status("FRESH", crate::model::workflow::GoalStatus::Todo)
                    .unwrap();
            }
            f.gate(source, mode.clone());
            let destination = if source == "workflow.todo.exit" {
                crate::model::workflow::GoalStatus::Backlog
            } else {
                crate::model::workflow::GoalStatus::Todo
            };
            if source == "workflow.backlog.enter" && mode == BindingMode::Background {
                f.work()
                    .transition_goal_status("FRESH", crate::model::workflow::GoalStatus::Todo)
                    .unwrap();
                f.work()
                    .transition_goal_status("FRESH", crate::model::workflow::GoalStatus::Backlog)
                    .unwrap();
            } else {
                let result = f.work().transition_goal_status("FRESH", destination);
                if mode == BindingMode::Blocking && source != "workflow.todo.enter" {
                    assert!(
                        result
                            .unwrap_err()
                            .to_string()
                            .contains(crate::application::events::transitions::PENDING)
                    );
                } else {
                    result.unwrap();
                }
            }
            f.dispatch();
            let invocation = f.invocation(source);
            assert_eq!(
                invocation.state,
                InvocationState::Pending,
                "{source}: {invocation:?}"
            );
            assert!(invocation.context.lifecycle.is_some());
            let result = f.execute(&invocation);
            assert_eq!(
                result.state,
                InvocationState::Succeeded,
                "{source} {todo_source}: {result:?}"
            );
            f.dispatch();
            f.assert_no_candidate();
            assert_eq!(before, f.snapshot(), "{source}");
        }
    }
}

#[test]
fn ordered_bindings_repairs_and_repeated_dispatch_reuse_one_registered_checkout() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.backlog.exit", BindingMode::Blocking);
    let mut config = (*f.service.config().unwrap()).clone();
    let first = config.events["workflow.backlog.exit"].bindings[0].clone();
    let mut skill = config.skills[&first.skill_id].clone();
    skill.id = "second-skill".into();
    config.skills.insert(skill.id.clone(), skill);
    let event = config.events.get_mut("workflow.backlog.exit").unwrap();
    let mut second = first;
    second.id = "second".into();
    second.skill_id = "second-skill".into();
    second.order += 1;
    event.bindings.push(second);
    crate::infrastructure::storage::automation::AutomationStore::new(&f.service.refine_dir)
        .update(config.revision, |stored| {
            *stored = config;
            Ok(())
        })
        .unwrap();
    let before = f.snapshot();
    f.request_todo();
    f.dispatch();
    let invocation = f.invocation("workflow.backlog.exit");
    f.dispatch();
    assert_eq!(invocation, f.invocation("workflow.backlog.exit"));
    let result = f.execute(&invocation);
    assert_eq!(result.state, InvocationState::Succeeded, "{result:?}");
    assert_eq!(result.attempts.len(), 4);
    assert_eq!(result.results.len(), 2);
    let launches = fs::read(invocation.context.cwd.join("launches.txt")).unwrap();
    let restarted =
        FileEventService::with_runtime_root(&f.service.refine_dir, f.temp.join("runtime"));
    assert_eq!(
        restarted.execute(&invocation.id, || Ok(())).unwrap(),
        result
    );
    assert_eq!(
        launches,
        fs::read(invocation.context.cwd.join("launches.txt")).unwrap()
    );
    f.dispatch();
    let goal = f.work().show_goal_detail("FRESH").unwrap();
    f.dispatch();
    assert_eq!(
        goal["event_generation"],
        f.work().show_goal_detail("FRESH").unwrap()["event_generation"]
    );
    f.assert_no_candidate();
    assert_eq!(before, f.snapshot());
}

#[test]
fn disabled_context_only_and_missing_inputs_create_no_checkout() {
    for variant in ["disabled", "context", "missing"] {
        let f = Fixture::new();
        f.gate(
            "workflow.backlog.exit",
            if variant == "context" {
                BindingMode::Context
            } else {
                BindingMode::Blocking
            },
        );
        let mut config = (*f.service.config().unwrap()).clone();
        let event = config.events.get_mut("workflow.backlog.exit").unwrap();
        if variant == "disabled" {
            event.bindings[0].enabled = false;
        }
        if variant == "missing" {
            event.bindings[0].inputs.clear();
        }
        crate::infrastructure::storage::automation::AutomationStore::new(&f.service.refine_dir)
            .update(config.revision, |stored| {
                *stored = config;
                Ok(())
            })
            .unwrap();
        let before = f.snapshot();
        if variant == "missing" {
            f.request_todo();
        } else {
            f.work()
                .transition_goal_status("FRESH", crate::model::workflow::GoalStatus::Todo)
                .unwrap();
        }
        f.dispatch();
        if variant != "disabled" {
            let invocation = f.invocation("workflow.backlog.exit");
            assert!(invocation.context.workspace.is_none());
            if variant == "context" {
                assert_eq!(
                    f.service.execute(&invocation.id, || Ok(())).unwrap().state,
                    InvocationState::Succeeded
                );
            } else {
                assert_eq!(invocation.state, InvocationState::Error);
            }
        }
        assert!(!f.primary.join(".git/refine-worktrees").exists());
        assert_eq!(before, f.snapshot());
        f.assert_no_candidate();
    }
}

#[test]
fn lifecycle_requires_explicit_authority_and_valid_subpath() {
    for subpath in ["../outside", "missing", "app"] {
        let f = Fixture::new();
        let _smoke = SmokeSkill::install(&f.service, &f.temp);
        crate::infrastructure::process::supervisor::config::FileSettingsService::for_node(
            &f.service.refine_dir,
            "default",
        )
        .update(&json!({"agent_subpath":subpath}))
        .unwrap();
        f.gate("workflow.backlog.exit", BindingMode::Blocking);
        let context = f
            .service
            .manual_context(&f.primary, &json!({"goal_id":"FRESH"}))
            .unwrap();
        assert!(
            f.service
                .prepare(
                    "workflow.backlog.exit",
                    context,
                    BTreeMap::new(),
                    "invented"
                )
                .is_err()
        );
        let before = f.snapshot();
        f.request_todo();
        f.dispatch();
        let invocation = f.invocation("workflow.backlog.exit");
        if subpath == "app" {
            assert_eq!(invocation.state, InvocationState::Pending, "{invocation:?}");
            assert!(invocation.context.cwd.ends_with("app"));
            assert_eq!(f.execute(&invocation).state, InvocationState::Succeeded);
        } else {
            assert_eq!(invocation.state, InvocationState::Error);
            assert!(
                invocation
                    .context
                    .workspace
                    .as_ref()
                    .unwrap()
                    .registration
                    .is_some()
            );
        }
        assert_eq!(before, f.snapshot());
    }
}

mod authority;
mod completion;
mod recovery;
mod workflow;

mod semantics;

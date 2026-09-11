//! Failure containment through the real scheduler and durable Goal claims.
use super::*;
use crate::application::workflow::engine::test_hooks;
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

fn fixture(name: &str, count: usize) -> (PathBuf, WorkflowEngine, FileWorkItemService) {
    let root = unique_temp_dir(name);
    let target = root.join("target");
    fs::create_dir_all(&target).unwrap();
    git(&target, &["init", "-b", "main"]).unwrap();
    git(&target, &["config", "user.email", "test@example.invalid"]).unwrap();
    git(&target, &["config", "user.name", "test"]).unwrap();
    git(&target, &["commit", "--allow-empty", "-m", "base"]).unwrap();
    let refine = test_refine_dir(&target);
    let items = FileWorkItemService::new(&refine);
    for i in 1..=count {
        let id = format!("GOAL{i}");
        items
            .create_goal_summary("Loop regression", Some(&id))
            .unwrap();
        items
            .append_goal_round_summary(&id, "Reporter", "Test worker isolation")
            .unwrap();
        items.transition_goal_status(&id, GoalStatus::Todo).unwrap();
    }
    FileSettingsService::new(&refine)
        .update(&json!({"parallel_run_cap":"1"}))
        .unwrap();
    let workflow = WorkflowEngine::with_target_root(root.join("run/8080"), &target);
    (root, workflow, items)
}

#[test]
fn explicit_new_round_is_admitted_in_the_same_pass_after_a_superseded_failure() {
    let (root, workflow, items) = fixture("superseded-occurrence-admission", 1);
    let records = items.clone();
    let executions = Arc::new(AtomicUsize::new(0));
    let observed = executions.clone();
    test_hooks::install(
        &workflow.runtime_root,
        Arc::new(move |_, goal, stage, _| {
            if stage != "executing" {
                return Ok(());
            }
            let attempt = observed.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                let current = records.show_goal_detail(goal)?;
                records.control_workflow(
                    goal,
                    &crate::application::work_items::WorkflowControl {
                        to: GoalStatus::Plan,
                        reason: "Explicit replacement Round".into(),
                        context: "Preserve prior evidence".into(),
                        expected_revision: current["workflow_revision"].as_u64().unwrap(),
                        request_id: "replace-once".into(),
                        actor: "Operator".into(),
                        force: false,
                        invocation_id: None,
                    },
                )?;
                Err(RefineError::Conflict("old attempt returned late".into()))
            } else {
                assert_eq!(
                    attempt, 1,
                    "a failed replacement must not retry automatically"
                );
                Err(RefineError::Conflict("replacement failure is final".into()))
            }
        }),
    );
    assert!(workflow.execute_work().is_err());
    assert_eq!(executions.load(Ordering::SeqCst), 2);
    let goal = items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 2);
    assert!(goal["rounds"][0]["workflow_attempt_authority"].is_object());
    assert_eq!(
        goal["rounds"][1]["failure_message"],
        "replacement failure is final"
    );
    assert_eq!(goal["status"], "failed");
    test_hooks::remove(&workflow.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn superseded_failure_preserves_new_intent_and_admits_followup_within_poll() {
    let (root, workflow, items) = fixture("superseded-loop", 2);
    let ended = Arc::new(Mutex::new(None));
    let admitted = Arc::new(Mutex::new(None));
    let end_clock = ended.clone();
    let next_clock = admitted.clone();
    let records = items.clone();
    test_hooks::install(
        &workflow.runtime_root,
        Arc::new(move |engine, goal, stage, _| {
            if goal == "GOAL1" && stage == "before_claim" && end_clock.lock().unwrap().is_some() {
                // Isolate old-result settlement from executing the explicitly reopened
                // Goal; the separate same-pass admission regression exercises that work.
                return Err(RefineError::Degraded(
                    "Reopened occurrence held for inspection".into(),
                ));
            }
            if stage == "executing" {
                assert_eq!(
                    records.show_goal_summary(goal)?.goal.status,
                    GoalStatus::Plan
                );
                if goal == "GOAL1" {
                    // Reproduce the integration error with a real divergent Git candidate.
                    let target = engine.target_root.as_ref().unwrap();
                    let detail = records.show_goal_detail(goal)?;
                    let branch = detail["branch_name"].as_str().unwrap();
                    let common =
                        crate::infrastructure::storage::project_layout::git_common_dir(target)?;
                    let worktree = common.join("refine-worktrees/refine-GOAL1-round-1");
                    git(&worktree, &["commit", "--allow-empty", "-m", "candidate"])?;
                    let candidate = git_output(&worktree, &["rev-parse", "HEAD"])
                        .trim()
                        .to_string();
                    git(
                        target,
                        &["commit", "--allow-empty", "-m", "target advancement"],
                    )?;
                    let base = git_output(target, &["rev-parse", "HEAD"])
                        .trim()
                        .to_string();
                    records.update_goal_git_refs(goal, branch, "main", &base, Some(&candidate))?;
                    records.update_goal_round_evaluation_summary(
                        goal,
                        0,
                        &json!({"workflow_git_remote":"origin"}),
                    )?;
                    for status in [
                        GoalStatus::Implement,
                        GoalStatus::Quality,
                        GoalStatus::Governance,
                    ] {
                        records.advance_automated_goal_status(goal, status)?;
                    }
                    let service = crate::application::workflow::governance::integration::FileGovernanceIntegrationService::with_target_root(&engine.runtime_root, &records.refine_dir, target);
                    let error = service
                        .integrate_workflow_candidate(
                            goal, 0, "default", branch, &candidate, "origin",
                        )
                        .unwrap_err();
                    assert!(
                        matches!(error, RefineError::StaleCandidate { .. }),
                        "{error}"
                    );
                    records.cancel_goal_summary(goal)?;
                    records.undo_goal_summary(goal)?;
                    Err::<(), _>(error)?;
                }
                *next_clock.lock().unwrap() = Some(Instant::now());
                return Err(RefineError::Conflict("follow-up entered Plan".into()));
            }
            if goal == "GOAL1" && stage == "settlement" {
                *end_clock.lock().unwrap() = Some(Instant::now());
            }
            Ok(())
        }),
    );
    assert!(workflow.execute_work().is_err());
    assert!(
        admitted
            .lock()
            .unwrap()
            .unwrap()
            .duration_since(ended.lock().unwrap().unwrap())
            < Duration::from_secs(1)
    );
    assert_eq!(
        items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    assert_eq!(
        items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["failure_category"],
        ""
    );
    assert_eq!(
        items.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Failed
    );
    test_hooks::remove(&workflow.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn preparation_execution_and_settlement_panics_cannot_strand_active_entries() {
    for panic_stage in ["claimed", "executing", "settlement", "delivery"] {
        let (root, workflow, items) = fixture(&format!("panic-{panic_stage}"), 2);
        let followup = Arc::new(AtomicUsize::new(0));
        let seen = followup.clone();
        let completion = Arc::new(Mutex::new(None::<Instant>));
        let ended = completion.clone();
        let origin = Arc::new(Mutex::new(None));
        let claimed = origin.clone();
        test_hooks::install(
            &workflow.runtime_root,
            Arc::new(move |_, goal, stage, authority| {
                if goal == "GOAL1" && stage == "claimed" {
                    *claimed.lock().unwrap() = Some(authority);
                }
                if goal == "GOAL1" && stage == "delivery" {
                    *ended.lock().unwrap() = Some(Instant::now());
                }
                if goal == "GOAL1" && stage == panic_stage {
                    panic!("injected {panic_stage}");
                }
                if stage == "executing" {
                    if goal == "GOAL2" {
                        assert!(ended.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1));
                        seen.fetch_add(1, Ordering::SeqCst);
                    }
                    return Err(RefineError::Conflict("behavior error".into()));
                }
                Ok(())
            }),
        );
        assert!(workflow.execute_work().is_err());
        assert_eq!(followup.load(Ordering::SeqCst), 1, "{panic_stage}");
        assert_eq!(
            items.show_goal_summary("GOAL2").unwrap().goal.status,
            GoalStatus::Failed
        );
        let evidence = originating_evidence(&workflow, "GOAL1", origin.lock().unwrap().unwrap());
        if panic_stage == "settlement" {
            assert_origin(
                &evidence,
                "GOAL1",
                origin.lock().unwrap().unwrap(),
                "workflow",
                "behavior error",
            );
            assert_eq!(
                evidence["settlement"]["unpersisted_evidence"],
                "settlement panicked: injected settlement"
            );
            assert_eq!(evidence["runtime_evidence_persisted"], true);
            assert_eq!(
                items.show_goal_summary("GOAL1").unwrap().goal.status,
                GoalStatus::Plan
            );
        }
        test_hooks::remove(&workflow.runtime_root);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn transient_settlement_rechecks_authority_and_exhausted_writes_release_capacity() {
    for mode in ["retry", "supersede", "exhaust"] {
        let (root, workflow, items) = fixture(mode, 2);
        let writes = Arc::new(AtomicUsize::new(0));
        let count = writes.clone();
        let records = items.clone();
        let completion = Arc::new(Mutex::new(None::<Instant>));
        let ended = completion.clone();
        test_hooks::install(
            &workflow.runtime_root,
            Arc::new(move |_, goal, stage, _| {
                if goal == "GOAL1" && stage == "delivery" {
                    *ended.lock().unwrap() = Some(Instant::now());
                }
                if stage == "executing" {
                    if goal == "GOAL2" {
                        assert!(ended.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1));
                    }
                    return Err(RefineError::Conflict("integration failure".into()));
                }
                if goal == "GOAL1" && stage == "settlement" {
                    let attempt = count.fetch_add(1, Ordering::SeqCst);
                    if mode == "exhaust" || attempt == 0 {
                        if mode == "supersede" {
                            records.cancel_goal_summary(goal)?;
                        }
                        return Err(RefineError::Io("injected persistence fault".into()));
                    }
                }
                Ok(())
            }),
        );
        assert!(workflow.execute_work().is_err());
        assert_eq!(
            writes.load(Ordering::SeqCst),
            if mode == "exhaust" { 3 } else { 2 }
        );
        assert_eq!(
            items.show_goal_summary("GOAL1").unwrap().goal.status,
            match mode {
                "exhaust" => GoalStatus::Plan,
                "supersede" => GoalStatus::Cancelled,
                _ => GoalStatus::Failed,
            }
        );
        assert_eq!(
            items.show_goal_summary("GOAL2").unwrap().goal.status,
            GoalStatus::Failed
        );
        let evidence = fs::read_dir(workflow.runtime_root.join("workflow-failures"))
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path()).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(evidence.contains("integration failure"));
        if mode == "exhaust" {
            assert!(evidence.contains("unpersisted_evidence"));
        }
        test_hooks::remove(&workflow.runtime_root);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn preparation_fault_before_claim_preserves_goal_intent_and_releases_its_slot() {
    for panic in [false, true] {
        let (root, workflow, items) = fixture("preclaim-fault", 2);
        let before = items.show_goal_detail("GOAL1").unwrap();
        let finished = Arc::new(Mutex::new(None::<Instant>));
        let clock = finished.clone();
        let followup = Arc::new(AtomicUsize::new(0));
        let seen = followup.clone();
        test_hooks::install(
            &workflow.runtime_root,
            Arc::new(move |_, goal, stage, _| {
                if goal == "GOAL1" && stage == "before_claim" {
                    *clock.lock().unwrap() = Some(Instant::now());
                    if panic {
                        panic!("preclaim preparation panic");
                    }
                    return Err(RefineError::Io("preclaim preparation fault".into()));
                }
                if goal == "GOAL2" && stage == "executing" {
                    assert!(clock.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1));
                    seen.fetch_add(1, Ordering::SeqCst);
                    return Err(RefineError::Conflict("followup reached Plan".into()));
                }
                Ok(())
            }),
        );
        assert!(workflow.execute_work().is_err());
        assert_eq!(items.show_goal_detail("GOAL1").unwrap(), before);
        assert_eq!(followup.load(Ordering::SeqCst), 1);
        test_hooks::remove(&workflow.runtime_root);
        fs::remove_dir_all(root).unwrap();
    }
}

mod scheduling;

mod settlement_evidence;
use settlement_evidence::{assert_origin, originating_evidence};

#[cfg(target_os = "linux")]
mod pty;

#[cfg(target_os = "linux")]
mod standard_capture;

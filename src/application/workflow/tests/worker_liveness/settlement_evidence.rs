//! Originating settlement evidence and follow-up admission under fault injection.
use super::*;

pub(super) fn originating_evidence(
    workflow: &WorkflowEngine,
    goal: &str,
    authority: crate::application::work_items::WorkflowStepAuthority,
) -> Value {
    let found = fs::read_dir(workflow.runtime_root.join("workflow-failures"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<Value>(&fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .filter(|value| {
            value["goal_id"] == goal
                && value["round_idx"] == authority.round_idx
                && value["workflow_revision"] == authority.workflow_revision
        })
        .collect::<Vec<_>>();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one originating failure: {found:?}"
    );
    found.into_iter().next().unwrap()
}
pub(super) fn assert_origin(
    value: &Value,
    goal: &str,
    authority: crate::application::work_items::WorkflowStepAuthority,
    stage: &str,
    error: &str,
) {
    assert_eq!(value["goal_id"], goal);
    assert_eq!(value["round_idx"], authority.round_idx);
    assert_eq!(value["workflow_revision"], authority.workflow_revision);
    assert_eq!(value["failure_stage"], stage);
    assert_eq!(value["original_error"], error);
    let at = chrono::DateTime::parse_from_rfc3339(value["failure_at"].as_str().unwrap()).unwrap();
    assert!((chrono::Utc::now() - at.with_timezone(&chrono::Utc)).num_seconds() < 30);
}

#[test]
fn settlement_panics_preserve_supersession_and_structured_fallback_without_delaying_followup() {
    for mode in [
        "cancel",
        "reclaim",
        "new_round",
        "storage",
        "write_panic",
        "report_panic",
    ] {
        let (root, workflow, items) = fixture(mode, 2);
        let records = items.clone();
        let origin = Arc::new(Mutex::new(None));
        let claimed = origin.clone();
        let preserved = Arc::new(Mutex::new(None));
        let snapshot = preserved.clone();
        let completed = Arc::new(Mutex::new(None::<Instant>));
        let ended = completed.clone();
        let followups = Arc::new(AtomicUsize::new(0));
        let admitted = followups.clone();
        test_hooks::install(
            &workflow.runtime_root,
            Arc::new(move |engine, goal, stage, authority| {
                if goal == "GOAL1" && stage == "before_claim" && claimed.lock().unwrap().is_some() {
                    // Keep the explicitly authorized replacement unstarted while this
                    // fixture inspects old settlement. Repeated injected decisions would
                    // otherwise create unbounded new occurrences in the same scheduler pass.
                    return Err(RefineError::Degraded(
                        "Replacement held for settlement inspection".into(),
                    ));
                }
                if goal == "GOAL1" && stage == "claimed" {
                    *claimed.lock().unwrap() = Some(authority);
                }
                if goal == "GOAL1" && stage == "delivery" {
                    *ended.lock().unwrap() = Some(Instant::now());
                }
                if stage == "executing" {
                    if goal == "GOAL2" {
                        assert!(ended.lock().unwrap().unwrap().elapsed() < Duration::from_secs(1));
                        admitted.fetch_add(1, Ordering::SeqCst);
                    }
                    return Err(RefineError::Conflict(
                        "originating integration error".into(),
                    ));
                }
                if goal != "GOAL1" {
                    return Ok(());
                }
                if stage == "evidence_write" && mode == "write_panic" {
                    panic!("injected write panic");
                }
                if stage == "evidence_report" && mode == "report_panic" {
                    panic!("injected report panic");
                }
                if stage == "settlement" {
                    if matches!(mode, "cancel" | "reclaim" | "new_round") {
                        records.cancel_goal_summary(goal)?;
                        if mode != "cancel" {
                            records.undo_goal_summary(goal)?;
                            if mode == "new_round" {
                                records.append_goal_round_summary(
                                    goal,
                                    "Reporter",
                                    "New intent",
                                )?;
                            }
                            let (round, revision, request) =
                                records.authored_goal_commitment(goal)?;
                            let replacement = records.claim_workflow_attempt(
                                goal,
                                GoalStatus::Todo,
                                round,
                                revision,
                                &request,
                            )?;
                            records.advance_claimed_goal_status(
                                goal,
                                replacement,
                                GoalStatus::Todo,
                                GoalStatus::Plan,
                            )?;
                        }
                    }
                    if matches!(mode, "storage" | "report_panic") {
                        fs::create_dir_all(&engine.runtime_root).unwrap();
                        fs::write(
                            engine.runtime_root.join("workflow-failures"),
                            "unavailable storage",
                        )
                        .unwrap();
                    }
                    *snapshot.lock().unwrap() = Some(records.show_goal_detail(goal)?);
                    panic!("injected originating settlement fault");
                }
                Ok(())
            }),
        );
        assert!(workflow.execute_work().is_err());
        assert_eq!(followups.load(Ordering::SeqCst), 1, "{mode}");
        let authority = origin.lock().unwrap().unwrap();
        assert_eq!(
            items.show_goal_detail("GOAL1").unwrap(),
            preserved.lock().unwrap().clone().unwrap(),
            "newer intent changed: {mode}"
        );
        let evidence = if matches!(mode, "storage" | "write_panic" | "report_panic") {
            let reports = test_hooks::take_failures(&workflow.runtime_root);
            let mut originating = reports.into_iter().filter(|v| {
                v["goal_id"] == "GOAL1" && v["workflow_revision"] == authority.workflow_revision
            });
            let value = originating.next().expect("complete fallback evidence");
            assert!(originating.next().is_none());
            assert_eq!(value["runtime_evidence_persisted"], false);
            assert!(!value["write_fault"].as_str().unwrap().is_empty());
            assert!(
                value["final_outcome"]["unpersisted_evidence"]
                    .as_str()
                    .unwrap()
                    .contains("runtime evidence")
            );
            value
        } else {
            originating_evidence(&workflow, "GOAL1", authority)
        };
        assert_origin(
            &evidence,
            "GOAL1",
            authority,
            "workflow",
            "originating integration error",
        );
        assert_eq!(
            evidence["settlement"]["unpersisted_evidence"],
            "settlement panicked: injected originating settlement fault"
        );
        test_hooks::remove(&workflow.runtime_root);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn stale_authority_errors_still_finalize_originating_evidence() {
    let (root, workflow, items) = fixture("stale-authority-evidence", 2);
    let records = items.clone();
    let origin = Arc::new(Mutex::new(None));
    let captured = origin.clone();
    let preserved = Arc::new(Mutex::new(None));
    let snapshot = preserved.clone();
    test_hooks::install(
        &workflow.runtime_root,
        Arc::new(move |_, goal, stage, authority| {
            if stage == "executing" {
                if goal == "GOAL1" {
                    *captured.lock().unwrap() = Some(authority);
                    records.cancel_goal_summary(goal)?;
                    *snapshot.lock().unwrap() = Some(records.show_goal_detail(goal)?);
                }
                return Err(RefineError::Conflict(
                    "workflow attempt was superseded".into(),
                ));
            }
            Ok(())
        }),
    );
    assert!(workflow.evaluate_workflow().is_err());
    let authority = origin.lock().unwrap().unwrap();
    let evidence = originating_evidence(&workflow, "GOAL1", authority);
    assert_origin(
        &evidence,
        "GOAL1",
        authority,
        "workflow",
        "workflow attempt was superseded",
    );
    assert_eq!(evidence["settlement"], "superseded_attempt");
    assert_eq!(evidence["runtime_evidence_persisted"], true);
    assert_eq!(
        items.show_goal_detail("GOAL1").unwrap(),
        preserved.lock().unwrap().clone().unwrap()
    );
    assert_eq!(
        items.show_goal_summary("GOAL2").unwrap().goal.status,
        GoalStatus::Failed
    );
    test_hooks::remove(&workflow.runtime_root);
    fs::remove_dir_all(root).unwrap();
}

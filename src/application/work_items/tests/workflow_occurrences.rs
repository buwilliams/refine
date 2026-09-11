use super::*;

fn fixture() -> (PathBuf, FileWorkItemService) {
    let root = unique_temp_dir("workflow-occurrence");
    let items = FileWorkItemService::new(root.join(".refine"));
    items
        .create_goal_summary("Occurrence authority", Some("GOAL1"))
        .unwrap();
    items
        .append_goal_round_summary("GOAL1", "Reporter", "Implement the requested behavior")
        .unwrap();
    items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    (root, items)
}

fn claim(items: &FileWorkItemService, status: GoalStatus) -> WorkflowStepAuthority {
    let (round, revision, prompt) = items.authored_goal_commitment("GOAL1").unwrap();
    items
        .claim_workflow_attempt("GOAL1", status, round, revision, &prompt)
        .unwrap()
}

fn decision(items: &FileWorkItemService, to: GoalStatus) {
    items
        .control_workflow(
            "GOAL1",
            &WorkflowControl {
                to,
                reason: "Explicit recovery decision".into(),
                context: "Retain earlier work".into(),
                expected_revision: items.show_goal_detail("GOAL1").unwrap()["workflow_revision"]
                    .as_u64()
                    .unwrap(),
                request_id: uuid::Uuid::new_v4().to_string(),
                actor: "Operator".into(),
                force: false,
                invocation_id: None,
            },
        )
        .unwrap();
}

fn rejects_late_results(
    items: &FileWorkItemService,
    old: WorkflowStepAuthority,
    status: GoalStatus,
) {
    let before = items.show_goal_detail("GOAL1").unwrap();
    assert!(
        items
            .verify_workflow_attempt("GOAL1", old, status.clone(), "default")
            .is_err()
    );
    assert!(
        items
            .advance_claimed_goal_status("GOAL1", old, status, GoalStatus::Plan)
            .is_err()
    );
    assert_eq!(
        items
            .settle_workflow_attempt_failure(
                "GOAL1",
                old,
                "late",
                "Old failure",
                "2026-09-11T00:00:00Z"
            )
            .unwrap(),
        FailureSettlement::SupersededAttempt
    );
    let mut writer = items.clone();
    writer.bind_workflow_occurrence("GOAL1", old);
    assert!(
        writer
            .update_goal_round_evaluation_summary(
                "GOAL1",
                old.round_idx,
                &json!({"quality_state":"passed", "governance_candidate_commit":"stale"})
            )
            .is_err()
    );
    assert_eq!(items.show_goal_detail("GOAL1").unwrap(), before);
}

#[test]
fn claims_are_provenance_and_concurrent_workers_cannot_accept_conflicting_transitions() {
    let (root, items) = fixture();
    let first = claim(&items, GoalStatus::Todo);
    let before = items.show_goal_detail("GOAL1").unwrap();
    let second = claim(&items, GoalStatus::Todo);
    assert_eq!(first.generation, second.generation);
    items
        .verify_workflow_attempt("GOAL1", first, GoalStatus::Todo, "default")
        .unwrap();
    assert_eq!(
        items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_claim_history"][0],
        before["rounds"][0]["workflow_attempt_authority"]
    );
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            items.advance_claimed_goal_status("GOAL1", first, GoalStatus::Todo, GoalStatus::Plan)
        });
        let b = scope.spawn(|| {
            barrier.wait();
            items.settle_workflow_attempt_failure(
                "GOAL1",
                second,
                "provider",
                "Failure",
                "2026-09-11T00:00:00Z",
            )
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    match items.show_goal_summary("GOAL1").unwrap().goal.status {
        GoalStatus::Plan => {
            assert!(results.0.is_ok());
            assert_eq!(results.1.unwrap(), FailureSettlement::SupersededAttempt);
        }
        GoalStatus::Failed => {
            assert!(results.0.is_err());
            assert_eq!(results.1.unwrap(), FailureSettlement::AuthoritativeFailure);
        }
        other => panic!("Conflicting settlement: {other:?}"),
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_decisions_supersede_old_results_without_erasing_claims() {
    for action in [
        "retry", "reenter", "reopen", "round", "redirect", "reassign",
    ] {
        let (root, items) = fixture();
        let old = claim(&items, GoalStatus::Todo);
        let original =
            items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_attempt_authority"]
                .clone();
        match action {
            "retry" => decision(&items, GoalStatus::Todo),
            "reenter" => {
                decision(&items, GoalStatus::Backlog);
                items.start_goal_workflow("GOAL1").unwrap();
            }
            "reopen" => {
                items.cancel_goal_summary("GOAL1").unwrap();
                items.undo_goal_summary("GOAL1").unwrap();
            }
            "round" => {
                items
                    .settle_workflow_attempt_failure(
                        "GOAL1",
                        old,
                        "provider",
                        "Retain failure",
                        "2026-09-11T00:00:00Z",
                    )
                    .unwrap();
                items
                    .append_goal_round_summary("GOAL1", "Reporter", "Recover with retained work")
                    .unwrap();
            }
            "redirect" => decision(&items, GoalStatus::Plan),
            "reassign" => {
                crate::application::fleet::nodes::FileNodeRegistryService::new(&items.refine_dir)
                    .create("worker")
                    .unwrap();
                items.transfer_goal_to_node("worker", "GOAL1").unwrap();
                FileWorkItemService::for_node(&items.refine_dir, "worker")
                    .transfer_goal_to_node("default", "GOAL1")
                    .unwrap();
            }
            _ => unreachable!(),
        }
        rejects_late_results(&items, old, GoalStatus::Todo);
        assert_eq!(
            items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["workflow_attempt_authority"],
            original,
            "{action}"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn same_named_quality_retry_creates_new_occurrence_and_preserves_candidate_evidence() {
    let (root, items) = fixture();
    items
        .set_goal_status_unchecked("GOAL1", &GoalStatus::Quality)
        .unwrap();
    let old = claim(&items, GoalStatus::Quality);
    items
        .update_goal_round_evaluation_summary(
            "GOAL1",
            0,
            &json!({"quality_details":{"candidate_commit":"exact-candidate"}}),
        )
        .unwrap();
    items.retry_goal_quality_summary("GOAL1").unwrap();
    rejects_late_results(&items, old, GoalStatus::Quality);
    assert_eq!(
        items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["prior_attempts"][0]["quality_details"]
            ["candidate_commit"],
        "exact-candidate"
    );
    fs::remove_dir_all(root).unwrap();
}

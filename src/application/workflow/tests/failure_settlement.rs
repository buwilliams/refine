use super::*;
use crate::application::work_items::{BulkGoalSelection, BulkGoalUpdate, WorkflowStepAuthority};

#[test]
fn authoritative_failure_atomically_fails_goal_and_its_originating_round() {
    let temp_root = unique_temp_dir("authoritative-workflow-failure");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    prepare_todo_goal(&work_items, "GOAL1");
    let authority = claim_and_start(&work_items, "GOAL1");
    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);

    assert_eq!(
        workflow.settle_goal_failure(
            "GOAL1",
            authority,
            "quality",
            &RefineError::Conflict("quality stopped".to_string()),
        ),
        crate::application::work_items::FailureSettlement::AuthoritativeFailure
    );

    let summary = work_items.show_goal_summary("GOAL1").unwrap();
    let detail = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(summary.goal.status, GoalStatus::Failed);
    assert_eq!(detail["rounds"][0]["failure_category"], "quality");
    assert_eq!(detail["rounds"][0]["failure_message"], "quality stopped");
    assert_ne!(detail["rounds"][0]["failure_at"], "");

    let evidence = failure_evidence(&workflow, "GOAL1", authority);
    assert_eq!(evidence["failure_stage"], "quality");
    assert_eq!(evidence["original_error"], "quality stopped");
    assert_eq!(evidence["failure_at"], detail["rounds"][0]["failure_at"]);
    assert_eq!(evidence["settlement"], "authoritative_failure");
    assert_eq!(evidence["runtime_evidence_persisted"], true);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn undo_reclaim_of_cancelled_goal_supersedes_old_same_round_failure() {
    let temp_root = unique_temp_dir("cancelled-undo-reclaim-fence");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    prepare_todo_goal(&work_items, "GOAL1");
    let old_authority = claim_and_start(&work_items, "GOAL1");

    work_items.cancel_goal_summary("GOAL1").unwrap();
    let reopened = work_items.undo_goal_summary("GOAL1").unwrap();
    assert_eq!(reopened.goal.status, GoalStatus::Todo);
    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(
        workflow.settle_goal_failure(
            "GOAL1",
            old_authority,
            "workflow",
            &RefineError::Conflict("old worker stopped after reopen".to_string()),
        ),
        crate::application::work_items::FailureSettlement::SupersededAttempt
    );
    assert_eq!(
        work_items.show_goal_summary("GOAL1").unwrap().goal.status,
        GoalStatus::Todo
    );
    let replacement_authority = claim_and_start(&work_items, "GOAL1");
    assert_ne!(old_authority, replacement_authority);

    assert_eq!(
        workflow.settle_goal_failure(
            "GOAL1",
            old_authority,
            "workflow",
            &RefineError::Conflict("old worker stopped late".to_string()),
        ),
        crate::application::work_items::FailureSettlement::SupersededAttempt
    );

    assert_active_replacement_is_clean(&work_items, "GOAL1", 0, replacement_authority);
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn bulk_todo_reclaim_with_new_round_supersedes_old_failure() {
    let temp_root = unique_temp_dir("cancelled-bulk-new-round-reclaim-fence");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    prepare_todo_goal(&work_items, "GOAL1");
    let old_authority = claim_and_start(&work_items, "GOAL1");

    work_items.cancel_goal_summary("GOAL1").unwrap();
    let moved = work_items
        .bulk_update_goals(
            BulkGoalSelection {
                selected_ids: Some(vec!["GOAL1".to_string()]),
                ..Default::default()
            },
            BulkGoalUpdate::Status("todo".to_string()),
        )
        .unwrap();
    assert_eq!(moved.updated, 1);
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Replacement request")
        .unwrap();
    let replacement_authority = claim_and_start(&work_items, "GOAL1");
    assert_eq!(replacement_authority.round_idx, 1);

    let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(
        workflow.settle_goal_failure(
            "GOAL1",
            old_authority,
            "workflow",
            &RefineError::Conflict("old round stopped late".to_string()),
        ),
        crate::application::work_items::FailureSettlement::SupersededAttempt
    );

    assert_active_replacement_is_clean(&work_items, "GOAL1", 1, replacement_authority);
    fs::remove_dir_all(temp_root).unwrap();
}

fn prepare_todo_goal(work_items: &FileWorkItemService, goal_id: &str) {
    work_items
        .create_goal_summary("Failure fence", Some(goal_id))
        .unwrap();
    work_items
        .append_goal_round_summary(goal_id, "Reporter", "Original request")
        .unwrap();
    work_items
        .transition_goal_status(goal_id, GoalStatus::Todo)
        .unwrap();
}

fn claim_and_start(work_items: &FileWorkItemService, goal_id: &str) -> WorkflowStepAuthority {
    let (round_idx, revision, request) = work_items.authored_goal_commitment(goal_id).unwrap();
    let authority = work_items
        .claim_workflow_attempt(goal_id, GoalStatus::Todo, round_idx, revision, &request)
        .unwrap();
    work_items
        .advance_claimed_goal_status(goal_id, authority, GoalStatus::Todo, GoalStatus::Plan)
        .unwrap()
}

fn assert_active_replacement_is_clean(
    work_items: &FileWorkItemService,
    goal_id: &str,
    round_idx: usize,
    authority: WorkflowStepAuthority,
) {
    let summary = work_items.show_goal_summary(goal_id).unwrap();
    let detail = work_items.show_goal_detail(goal_id).unwrap();
    assert_eq!(summary.goal.status, GoalStatus::Plan);
    assert_eq!(summary.goal.round_count, round_idx + 1);
    assert_eq!(detail["rounds"][round_idx]["failure_category"], "");
    assert_eq!(detail["rounds"][round_idx]["failure_message"], "");
    assert_eq!(detail["rounds"][round_idx]["failure_at"], "");
    assert_eq!(
        detail["rounds"][round_idx]["workflow_attempt_authority"]["round_idx"],
        authority.round_idx
    );
    assert_eq!(
        detail["rounds"][round_idx]["workflow_attempt_authority"]["workflow_revision"],
        authority.workflow_revision
    );
}

fn failure_evidence(
    workflow: &WorkflowEngine,
    goal: &str,
    authority: WorkflowStepAuthority,
) -> serde_json::Value {
    let evidence = fs::read_dir(workflow.runtime_root.join("workflow-failures"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<serde_json::Value>(&fs::read(entry.unwrap().path()).unwrap())
                .unwrap()
        })
        .filter(|v| {
            v["goal_id"] == goal
                && v["round_idx"] == authority.round_idx
                && v["workflow_revision"] == authority.workflow_revision
        })
        .collect::<Vec<_>>();
    assert_eq!(evidence.len(), 1);
    evidence.into_iter().next().unwrap()
}

#[test]
fn settlement_panic_outcomes_keep_exact_origin_and_never_claim_unavailable_storage() {
    use crate::application::work_items::FailureSettlement;
    use crate::application::workflow::engine::test_hooks;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for storage_available in [true, false] {
        let temp_root = unique_temp_dir("settlement-panic-evidence");
        let target_root = temp_root.join("target");
        let refine_dir = test_refine_dir(&target_root);
        let runtime_root = temp_root.join("run/8080");
        let items = FileWorkItemService::new(&refine_dir);
        prepare_todo_goal(&items, "ORIGIN");
        let authority = claim_and_start(&items, "ORIGIN");
        items.cancel_goal_summary("ORIGIN").unwrap();
        let before = items.show_goal_detail("ORIGIN").unwrap();
        let workflow = WorkflowEngine::with_target_root(&runtime_root, &target_root);
        if !storage_available {
            fs::create_dir_all(&runtime_root).unwrap();
            fs::write(runtime_root.join("workflow-failures"), "blocked").unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let attempts = calls.clone();
        test_hooks::install(
            &runtime_root,
            Arc::new(move |_, _, stage, _| {
                if stage == "settlement" {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    panic!("settlement storage panic");
                }
                Ok(())
            }),
        );
        let outcome = workflow.settle_goal_failure(
            "ORIGIN",
            authority,
            "integration",
            &RefineError::Io("original write error".into()),
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "panics are not transient retry failures"
        );
        assert_eq!(items.show_goal_detail("ORIGIN").unwrap(), before);
        let expected = FailureSettlement::UnpersistedEvidence(
            "settlement panicked: settlement storage panic".into(),
        );
        let evidence = if storage_available {
            assert_eq!(outcome, expected);
            failure_evidence(&workflow, "ORIGIN", authority)
        } else {
            let reports = test_hooks::take_failures(&runtime_root);
            assert_eq!(reports.len(), 1);
            let evidence = reports.into_iter().next().unwrap();
            assert_eq!(
                evidence["final_outcome"],
                serde_json::to_value(outcome).unwrap()
            );
            assert!(!evidence["write_fault"].as_str().unwrap().is_empty());
            evidence
        };
        assert_eq!(evidence["goal_id"], "ORIGIN");
        assert_eq!(evidence["round_idx"], authority.round_idx);
        assert_eq!(evidence["workflow_revision"], authority.workflow_revision);
        assert_eq!(evidence["failure_stage"], "integration");
        assert_eq!(evidence["original_error"], "original write error");
        assert!(
            chrono::DateTime::parse_from_rfc3339(evidence["failure_at"].as_str().unwrap()).is_ok()
        );
        assert_eq!(
            evidence["settlement"],
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(evidence["runtime_evidence_persisted"], storage_available);
        test_hooks::remove(&runtime_root);
        fs::remove_dir_all(temp_root).unwrap();
    }
}

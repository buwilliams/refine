use chrono::{TimeDelta, TimeZone, Utc};

use super::super::claim_history::{
    EXECUTION_FAILURE_QUARANTINE_THRESHOLD, MAX_TERMINAL_CLAIM_HISTORY,
};
use super::*;

fn claim(
    index: usize,
    goal_id: &str,
    state: WorkflowClaimState,
    failure_stage: Option<&str>,
    failure_message: Option<String>,
) -> WorkflowClaim {
    let timestamp =
        Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap() + TimeDelta::seconds(index as i64);
    WorkflowClaim {
        claim_id: format!("claim-{index}"),
        goal_id: goal_id.to_string(),
        node_id: "default".to_string(),
        provider: "smoke-ai".to_string(),
        target_app_id: "default".to_string(),
        execution_id: Some(format!("exec-{index}")),
        round_idx: Some(0),
        goal_revision: Some(1),
        failure_stage: failure_stage.map(ToString::to_string),
        failure_message,
        decision_version: 3,
        occurrences: 1,
        state,
        created_at: timestamp.to_rfc3339(),
        updated_at: timestamp.to_rfc3339(),
    }
}

#[test]
fn claim_history_keeps_every_active_claim_and_hard_caps_terminal_records() {
    let mut state = WorkflowAutomationState::default();
    for index in 0..(MAX_TERMINAL_CLAIM_HISTORY + 80) {
        state.claims.push(claim(
            index,
            &format!("TERMINAL-{index}"),
            WorkflowClaimState::Failed,
            Some("execution"),
            Some(format!("distinct failure {index}")),
        ));
    }
    for index in 0..3 {
        state.claims.push(claim(
            10_000 + index,
            &format!("ACTIVE-{index}"),
            WorkflowClaimState::Running,
            None,
            None,
        ));
    }

    state.normalize_claim_history();

    assert_eq!(state.active_claim_count(), 3);
    assert_eq!(state.active_claims().count(), 3);
    assert_eq!(
        state
            .claims
            .iter()
            .filter(|claim| !claim.is_active())
            .count(),
        MAX_TERMINAL_CLAIM_HISTORY
    );
    assert_eq!(state.claims.len(), MAX_TERMINAL_CLAIM_HISTORY + 3);
}

#[test]
fn equivalent_terminal_attempts_deduplicate_without_losing_failure_count() {
    let mut state = WorkflowAutomationState::default();
    for index in 0..40 {
        state.claims.push(claim(
            index,
            "REPEATED",
            WorkflowClaimState::Failed,
            Some("execution"),
            Some("same provider failure".to_string()),
        ));
    }

    state.normalize_claim_history();

    assert_eq!(state.claims.len(), 1);
    assert_eq!(state.claims[0].occurrences, 40);
    let summary = &state.claim_summaries["REPEATED"];
    assert_eq!(
        summary.consecutive_execution_failures,
        EXECUTION_FAILURE_QUARANTINE_THRESHOLD
    );
    assert!(summary.execution_quarantined);
}

#[test]
fn preparation_failure_quarantine_survives_terminal_compaction() {
    let mut state = WorkflowAutomationState::default();
    state.claims.push(claim(
        0,
        "PREPARATION",
        WorkflowClaimState::Failed,
        Some("preparation"),
        Some("target state unavailable".to_string()),
    ));
    for index in 1..(MAX_TERMINAL_CLAIM_HISTORY + 40) {
        state.claims.push(claim(
            index,
            &format!("OTHER-{index}"),
            WorkflowClaimState::Completed,
            None,
            None,
        ));
    }

    state.normalize_claim_history();

    assert!(
        state
            .claims
            .iter()
            .all(|claim| claim.goal_id != "PREPARATION"),
        "the old full record should demonstrate that the summary, not accidental retention, preserves quarantine"
    );
    assert_eq!(
        state
            .latest_preparation_failure("PREPARATION")
            .map(|claim| claim.failure_message.as_deref()),
        Some(Some("target state unavailable"))
    );
    assert!(state.preparation_failure_goal_ids().contains("PREPARATION"));
}

#[test]
fn execution_failures_back_off_then_quarantine_at_the_hard_attempt_bound() {
    let started = Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap();
    let mut state = WorkflowAutomationState::default();
    for attempt in 0..EXECUTION_FAILURE_QUARANTINE_THRESHOLD {
        state.claims.push(claim(
            attempt as usize,
            "RETRY",
            WorkflowClaimState::Failed,
            Some("execution"),
            Some(format!("failure {attempt}")),
        ));
        state.normalize_claim_history();
        assert!(!state.claim_retry_allowed("RETRY", started + TimeDelta::seconds(attempt as i64)));
    }

    let summary = &state.claim_summaries["RETRY"];
    assert_eq!(
        summary.consecutive_execution_failures,
        EXECUTION_FAILURE_QUARANTINE_THRESHOLD
    );
    assert!(summary.execution_quarantined);
    assert!(!state.claim_retry_allowed("RETRY", started + TimeDelta::days(365)));
}

#[test]
fn long_distinct_failure_run_has_a_bounded_serialized_state_file() {
    let temp_root = unique_temp_dir("bounded-claim-state-file");
    let path = temp_root.join(WORKFLOW_AUTOMATION_STATE_FILE);
    let mut state = WorkflowAutomationState::default();
    for index in 0..5_000 {
        state.claims.push(claim(
            index,
            "LONG-RUN",
            WorkflowClaimState::Failed,
            Some("execution"),
            Some(format!("unique execution failure {index}")),
        ));
    }

    super::super::write_state(&path, &state).unwrap();
    let persisted = fs::read(&path).unwrap();
    let loaded = WorkflowEngine::new(&temp_root).load_state().unwrap();

    assert_eq!(loaded.claims.len(), MAX_TERMINAL_CLAIM_HISTORY);
    assert!(
        persisted.len() < 512 * 1_024,
        "claim state was {} bytes after compaction",
        persisted.len()
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn legacy_state_without_summary_or_occurrences_migrates_on_read() {
    let temp_root = unique_temp_dir("legacy-claim-state");
    let path = temp_root.join(WORKFLOW_AUTOMATION_STATE_FILE);
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "version": 7,
            "policy": WorkflowPolicy::default(),
            "claims": [{
                "claim_id": "legacy",
                "goal_id": "LEGACY",
                "node_id": "default",
                "provider": "smoke-ai",
                "target_app_id": "default",
                "execution_id": "legacy-exec",
                "decision_version": 2,
                "state": "failed",
                "failure_stage": "preparation",
                "created_at": "2026-08-06T12:00:00Z",
                "updated_at": "2026-08-06T12:00:01Z"
            }],
            "updated_at": "2026-08-06T12:00:01Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded = WorkflowEngine::new(&temp_root).load_state().unwrap();

    assert_eq!(loaded.claims[0].occurrences, 1);
    assert!(loaded.latest_preparation_failure("LEGACY").is_some());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn resuming_with_many_todo_goals_does_not_reclaim_quarantined_failures() {
    let temp_root = unique_temp_dir("quarantined-resume");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    let mut state = WorkflowAutomationState::default();

    for goal_index in 0..32 {
        let goal_id = format!("QUARANTINED-{goal_index:03}");
        work_items
            .create_goal_summary("Repeated execution failure", Some(&goal_id))
            .unwrap();
        work_items
            .transition_goal_status(&goal_id, GoalStatus::Todo)
            .unwrap();
        for attempt in 0..EXECUTION_FAILURE_QUARANTINE_THRESHOLD {
            let index = goal_index * 100 + attempt as usize;
            state.claims.push(claim(
                index,
                &goal_id,
                WorkflowClaimState::Failed,
                Some("execution"),
                Some("repeatable provider failure".to_string()),
            ));
        }
    }
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(
        runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 0);
    let resumed = automation.load_state().unwrap();
    assert_eq!(resumed.active_claim_count(), 0);
    assert_eq!(resumed.claim_summaries.len(), 32);
    assert!(
        resumed
            .claim_summaries
            .values()
            .all(|summary| summary.execution_quarantined)
    );
    let persisted: Value = serde_json::from_slice(
        &fs::read(runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted["claim_history_version"], 1);
    assert_eq!(persisted["claims"].as_array().unwrap().len(), 32);

    fs::remove_dir_all(temp_root).unwrap();
}

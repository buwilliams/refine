use crate::tools::product::work_items::workflow_revision;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::super::claim_history::MAX_TERMINAL_CLAIM_HISTORY;
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
fn active_indexes_do_not_hide_an_older_active_claim_behind_a_newer_terminal_claim() {
    let mut state = WorkflowAutomationState::default();
    state
        .claims
        .push(claim(0, "MIXED", WorkflowClaimState::Running, None, None));
    state.claims.push(claim(
        1,
        "MIXED",
        WorkflowClaimState::Failed,
        Some("execution"),
        Some("newer terminal attempt".to_string()),
    ));

    state.normalize_claim_history();

    assert_eq!(state.active_claim_count(), 1);
    assert_eq!(state.active_claim_goal_ids().collect::<Vec<_>>(), ["MIXED"]);
    assert_eq!(
        state
            .active_claim("MIXED")
            .map(|claim| claim.claim_id.as_str()),
        Some("claim-0")
    );
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
    assert_eq!(summary.consecutive_execution_failures, 40);
}

#[test]
fn preparation_failure_evidence_survives_terminal_compaction() {
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
        "the old full record should demonstrate that the summary, not accidental retention, preserves evidence"
    );
    assert_eq!(
        state.claim_summaries["PREPARATION"]
            .latest_preparation_failure
            .as_ref()
            .map(|claim| claim.failure_message.as_deref()),
        Some(Some("target state unavailable"))
    );
}

#[test]
fn execution_failures_keep_bounded_backoff_without_permanent_admission_latch() {
    let started = Utc.with_ymd_and_hms(2026, 8, 6, 12, 0, 0).unwrap();
    let mut state = WorkflowAutomationState::default();
    for attempt in 0..8 {
        state.claims.push(claim(
            attempt,
            "RETRY",
            WorkflowClaimState::Failed,
            Some("execution"),
            Some(format!("failure {attempt}")),
        ));
        state.normalize_claim_history();
    }

    let summary = &state.claim_summaries["RETRY"];
    assert_eq!(summary.consecutive_execution_failures, 8);
    let failed_at = started + TimeDelta::seconds(7);
    assert_eq!(
        state.claim_retry_not_before(
            "RETRY",
            Some(0),
            Some(1),
            failed_at + TimeDelta::seconds(299)
        ),
        summary.retry_not_before.as_deref()
    );
    assert_eq!(
        state.claim_retry_not_before(
            "RETRY",
            Some(0),
            Some(1),
            failed_at + TimeDelta::seconds(300)
        ),
        None,
        "even a long failure history is admitted after the five-minute cap"
    );
    assert_eq!(
        state.claim_retry_not_before("RETRY", Some(1), Some(2), failed_at),
        None,
        "a fresh Goal identity bypasses stale attempt backoff"
    );
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
    assert!(
        loaded.claim_summaries["LEGACY"]
            .latest_preparation_failure
            .is_some()
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn fresh_round_after_more_than_five_failures_gets_new_claim_and_execution_identity() {
    let temp_root = unique_temp_dir("fresh-round-after-long-failure-history");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Recover with a fresh Round", Some("RECOVER"))
        .unwrap();
    work_items
        .append_goal_round_summary("RECOVER", "Reporter", "Initial attempt")
        .unwrap();
    work_items
        .transition_goal_status("RECOVER", GoalStatus::Todo)
        .unwrap();
    work_items.fail_automated_goal_if_active("RECOVER").unwrap();
    let failed_detail = work_items.show_goal_detail("RECOVER").unwrap();
    let failed_revision = workflow_revision(&failed_detail);
    let failed_at = Utc::now();
    let mut state = WorkflowAutomationState::default();
    for attempt in 0..7 {
        let mut failure = claim(
            attempt,
            "RECOVER",
            WorkflowClaimState::Failed,
            Some("execution"),
            Some(format!("historic failure {attempt}")),
        );
        failure.goal_revision = Some(failed_revision);
        failure.updated_at = failed_at.to_rfc3339();
        state.claims.push(failure);
    }
    fs::create_dir_all(&runtime_root).unwrap();
    super::super::write_state(&runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE), &state).unwrap();

    let revised = work_items
        .append_goal_round_summary("RECOVER", "Reporter", "Fresh recovery")
        .unwrap();
    assert_eq!(revised.goal.status, GoalStatus::Todo);
    assert_eq!(revised.goal.round_count, 2);

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 1);
    let promoted = automation.load_state().unwrap();
    let fresh_claim = promoted.active_claim("RECOVER").unwrap();
    assert_eq!(
        promoted.claim_summaries["RECOVER"].consecutive_execution_failures,
        0
    );
    assert!(
        promoted.claim_summaries["RECOVER"]
            .retry_not_before
            .is_none()
    );
    assert!(!fresh_claim.claim_id.starts_with("claim-"));
    assert_eq!(fresh_claim.round_idx, Some(1));
    assert_ne!(fresh_claim.goal_revision, Some(failed_revision));
    let fresh_claim_id = fresh_claim.claim_id.clone();
    let execution_id = automation.start_claim(&fresh_claim_id).unwrap();
    assert!((0..7).all(|attempt| execution_id != format!("exec-{attempt}")));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn still_eligible_retry_delay_is_available_to_shared_status_surfaces() {
    let temp_root = unique_temp_dir("shared-retry-delay-status");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Temporarily delayed", Some("DELAYED"))
        .unwrap();
    work_items
        .append_goal_round_summary("DELAYED", "Reporter", "Retry safely")
        .unwrap();
    let goal = work_items
        .transition_goal_status("DELAYED", GoalStatus::Todo)
        .unwrap();
    let revision = workflow_revision(&work_items.show_goal_detail("DELAYED").unwrap());
    let mut failure = claim(
        0,
        "DELAYED",
        WorkflowClaimState::Failed,
        Some("execution"),
        Some("provider temporarily unavailable".to_string()),
    );
    failure.round_idx = goal.goal.round_count.checked_sub(1);
    failure.goal_revision = Some(revision);
    failure.updated_at = Utc::now().to_rfc3339();
    let expected_claim_id = failure.claim_id.clone();
    let mut state = WorkflowAutomationState::default();
    state.claims.push(failure);
    fs::create_dir_all(&runtime_root).unwrap();
    super::super::write_state(&runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE), &state).unwrap();

    let projection = crate::tools::product::project_state::FileProjectStateStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let delays = WorkflowEngine::with_target_root(&runtime_root, &target_root)
        .retry_delays_needing_attention(&projection)
        .unwrap();

    assert_eq!(delays.len(), 1);
    assert_eq!(delays[0].goal_id, "DELAYED");
    assert_eq!(delays[0].claim_id, expected_claim_id);
    assert_eq!(
        delays[0].failure_message.as_deref(),
        Some("provider temporarily unavailable")
    );
    assert!(DateTime::parse_from_rfc3339(&delays[0].retry_not_before).is_ok());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn legacy_quarantine_migrates_on_restart_without_losing_failure_evidence() {
    let temp_root = unique_temp_dir("legacy-quarantine-restart");
    let target_root = temp_root.join("target");
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Repeated execution failure", Some("RECOVER"))
        .unwrap();
    work_items
        .append_goal_round_summary("RECOVER", "Reporter", "Try again")
        .unwrap();
    work_items
        .transition_goal_status("RECOVER", GoalStatus::Todo)
        .unwrap();
    let detail = work_items.show_goal_detail("RECOVER").unwrap();
    let revision = workflow_revision(&detail);
    let mut failures = Vec::new();
    for attempt in 0..7 {
        let mut failure = claim(
            attempt,
            "RECOVER",
            WorkflowClaimState::Failed,
            Some("execution"),
            Some("repeatable provider failure".to_string()),
        );
        failure.goal_revision = Some(revision);
        failures.push(failure);
    }
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(
        runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE),
        serde_json::to_vec_pretty(&json!({
            "version": 3,
            "policy": WorkflowPolicy::default(),
            "claim_history_version": 1,
            "claim_summaries": {
                "RECOVER": {
                    "latest_claim": failures.last().unwrap(),
                    "consecutive_execution_failures": 5,
                    "retry_not_before": "2026-08-06T12:05:06Z",
                    "execution_quarantined": true
                }
            },
            "claims": failures,
            "updated_at": "2026-08-06T12:00:06Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    assert_eq!(automation.promote().unwrap(), 1);
    let resumed = automation.load_state().unwrap();
    assert_eq!(resumed.active_claim_count(), 1);
    assert_eq!(
        resumed.claim_summaries["RECOVER"].consecutive_execution_failures, 5,
        "migration preserves the legacy capped count without reconstructing lost detail"
    );
    assert_eq!(resumed.claims[0].occurrences, 7);
    let persisted: Value = serde_json::from_slice(
        &fs::read(runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted["claim_history_version"], 2);
    assert!(
        persisted
            .to_string()
            .find("execution_quarantined")
            .is_none()
    );
    assert_eq!(persisted["claims"].as_array().unwrap().len(), 2);

    fs::remove_dir_all(temp_root).unwrap();
}

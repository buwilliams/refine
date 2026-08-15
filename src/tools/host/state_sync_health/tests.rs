use super::*;

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("refine-{label}-{}", uuid::Uuid::new_v4()))
}

#[test]
fn failure_episode_is_redacted_and_recovery_is_transition_based() {
    let temp = temp_root("state-sync-failure");
    let target = temp.join("target");
    fs::create_dir_all(&target).expect("target");
    let service = FileStateSyncHealthService::new(temp.join("run"));
    let first = service
        .record_failure(
            &target,
            "node-a",
            "git fetch https://user:secret@example.com password=hunter2",
        )
        .expect("failure");
    assert_eq!(
        first,
        Some(StateSyncHealthActivity::FailureStarted {
            error: "git fetch https://[REDACTED]@example.com password=[REDACTED]".to_string()
        })
    );
    assert!(
        service
            .record_failure(&target, "node-a", "still failed")
            .expect("retry")
            .is_none()
    );
    let health = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("health");
    assert_eq!(health.status, "failed");
    assert!(!health.aggregate_counts_authoritative);
    assert_eq!(health.last_error.as_deref(), Some("still failed"));
    assert!(matches!(
        service.record_success(&target, "node-a").expect("success"),
        Some(StateSyncHealthActivity::Recovered { .. })
    ));
    assert_eq!(
        service
            .inspect(&target, "node-a", Duration::from_secs(900))
            .expect("recovered")
            .status,
        "healthy"
    );
    assert!(
        service
            .record_success(&target, "node-a")
            .expect("repeat")
            .is_none()
    );
    fs::remove_dir_all(temp).expect("cleanup");
}

#[test]
fn missing_baseline_recovery_kind_is_typed_and_cleared_by_success() {
    let temp = temp_root("state-sync-recovery-kind");
    let target = temp.join("target");
    fs::create_dir_all(&target).expect("target");
    let service = FileStateSyncHealthService::new(temp.join("run"));

    service
        .record_failure_with_recovery_kind(
            &target,
            "node-a",
            "baseline is missing",
            Some(StateSyncRecoveryKind::MissingBaseline),
        )
        .expect("typed failure");
    let failed = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("failed health");
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed.recovery_kind,
        Some(StateSyncRecoveryKind::MissingBaseline)
    );
    assert_eq!(
        serde_json::to_value(&failed).unwrap()["recovery_kind"],
        "missing_baseline"
    );

    let (settled, activity) = service
        .record_recovery_success(&target, "node-a", StateSyncRecoveryKind::MissingBaseline)
        .expect("recovered");
    assert!(settled);
    assert!(matches!(
        activity,
        Some(StateSyncHealthActivity::Recovered { .. })
    ));
    let recovered = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("recovered health");
    assert_eq!(recovered.status, "healthy");
    assert_eq!(recovered.recovery_kind, None);

    service
        .record_failure(&target, "node-a", "unrelated fetch failure")
        .expect("unrelated failure");
    let unrelated_revision = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("unrelated health")
        .revision;
    let (settled, activity) = service
        .record_recovery_success(&target, "node-a", StateSyncRecoveryKind::MissingBaseline)
        .expect("conditional settlement");
    assert!(!settled);
    assert_eq!(activity, None);
    let unchanged = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("unchanged health");
    assert_eq!(unchanged.status, "failed");
    assert_eq!(
        unchanged.last_error.as_deref(),
        Some("unrelated fetch failure")
    );
    assert_eq!(unchanged.revision, unrelated_revision);

    fs::remove_dir_all(temp).expect("cleanup");
}

#[test]
fn binding_is_scoped_to_target_and_node() {
    let temp = temp_root("state-sync-binding");
    let target = temp.join("target");
    let other = temp.join("other");
    fs::create_dir_all(&target).expect("target");
    fs::create_dir_all(&other).expect("other");
    let service = FileStateSyncHealthService::new(temp.join("run"));
    service.record_success(&target, "node-a").expect("success");
    let original_revision = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("original")
        .revision;
    service.bind(&other, "node-a").expect("rebind");
    assert_eq!(
        service
            .inspect(&other, "node-a", Duration::from_secs(900))
            .expect("other")
            .status,
        "unknown"
    );
    assert!(
        service
            .inspect(&other, "node-a", Duration::from_secs(900))
            .expect("rebound")
            .revision
            > original_revision
    );
    assert_eq!(
        service
            .inspect(&target, "node-b", Duration::from_secs(900))
            .expect("node")
            .status,
        "unknown"
    );
    fs::remove_dir_all(temp).expect("cleanup");
}

#[test]
fn deferrals_and_an_unconfigured_remote_are_neutral() {
    let temp = temp_root("state-sync-neutral");
    let target = temp.join("target");
    fs::create_dir_all(&target).expect("target");
    let service = FileStateSyncHealthService::new(temp.join("run"));

    service.record_attempt(&target, "node-a").expect("attempt");
    service
        .record_neutral(&target, "node-a", "deferred", None)
        .expect("deferral");
    let deferred = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("deferred health");
    assert_eq!(deferred.status, "unknown");
    assert_eq!(deferred.last_attempt_outcome.as_deref(), Some("deferred"));
    assert!(deferred.failure_since.is_none());

    service
        .record_neutral(&target, "node-a", "unconfigured", Some(false))
        .expect("unconfigured remote");
    let unconfigured = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("unconfigured health");
    assert_eq!(unconfigured.status, "unconfigured");
    assert!(unconfigured.failure_since.is_none());
    assert!(unconfigured.aggregate_counts_authoritative);

    service
        .record_failure_with_recovery_kind(
            &target,
            "node-a",
            "baseline missing",
            Some(StateSyncRecoveryKind::MissingBaseline),
        )
        .expect("recovery failure");
    service
        .record_neutral(&target, "node-a", "unconfigured", Some(false))
        .expect("remote removed");
    let no_longer_eligible = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .expect("ineligible health");
    assert_eq!(no_longer_eligible.status, "unconfigured");
    assert_eq!(no_longer_eligible.recovery_kind, None);

    fs::remove_dir_all(temp).expect("cleanup");
}

#[test]
fn freshness_derivation_crosses_the_wall_clock_threshold() {
    let monitoring_since = "2026-08-15T12:00:00Z".to_string();
    let record = StateSyncHealthRecord {
        target_root: "/target".to_string(),
        node_id: "node-a".to_string(),
        monitoring_since: monitoring_since.clone(),
        last_attempt_at: None,
        last_attempt_outcome: None,
        attempt_sequence: 0,
        last_attempt_id: None,
        last_attempt_source: None,
        last_success_at: Some(monitoring_since),
        last_success_attempt_id: None,
        failure_since: None,
        last_failure_at: None,
        last_error: None,
        last_failure_attempt_id: None,
        last_failure_source: None,
        last_conflict_report_id: None,
        last_conflict_report_location: None,
        recovery_kind: None,
        last_reminder_at: None,
        remote_configured: Some(true),
        revision: 7,
    };
    let before = derive_health(
        record.clone(),
        Duration::from_secs(900),
        parse_timestamp("2026-08-15T12:14:59Z").unwrap(),
    );
    assert_eq!(before.status, "healthy");
    let stale = derive_health(
        record,
        Duration::from_secs(900),
        parse_timestamp("2026-08-15T12:15:00Z").unwrap(),
    );
    assert_eq!(stale.status, "stale");
    assert_eq!(stale.stale_since.as_deref(), Some("2026-08-15T12:15:00Z"));
    assert!(!stale.aggregate_counts_authoritative);
}

#[test]
fn correlated_attempt_settlement_keeps_newer_metadata_and_active_failure_evidence() {
    let temp = temp_root("correlated-attempts");
    let runtime = temp.join("run");
    let target = temp.join("target");
    fs::create_dir_all(&target).unwrap();
    let service = FileStateSyncHealthService::new(&runtime);
    let first = service
        .begin_attempt(&target, "node-a", "background")
        .unwrap();
    let second = service.begin_attempt(&target, "node-a", "manual").unwrap();

    service
        .settle_failure(
            &target,
            "node-a",
            first,
            "background",
            "conflict",
            Some("report-1"),
            Some("/run/conflicts/latest.json"),
            None,
        )
        .unwrap();
    let third = service
        .begin_attempt(&target, "node-a", "background")
        .unwrap();
    service
        .settle_neutral(
            &target,
            "node-a",
            third,
            "background",
            "deferred",
            Some(true),
        )
        .unwrap();

    let failed = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.last_attempt_id, Some(third));
    assert_eq!(failed.last_attempt_source.as_deref(), Some("background"));
    assert_eq!(failed.last_attempt_outcome.as_deref(), Some("deferred"));
    assert_eq!(failed.last_failure_attempt_id, Some(first));
    assert_eq!(failed.last_conflict_report_id.as_deref(), Some("report-1"));

    service
        .settle_success(&target, "node-a", second, "manual")
        .unwrap();
    let late_success = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .unwrap();
    assert_eq!(late_success.status, "healthy");
    assert_eq!(late_success.last_attempt_id, Some(third));
    assert_eq!(
        late_success.last_attempt_source.as_deref(),
        Some("background")
    );
    assert_eq!(
        late_success.last_attempt_outcome.as_deref(),
        Some("deferred")
    );
    assert!(late_success.last_success_at.is_some());
    assert!(late_success.failure_since.is_none());
    assert!(late_success.last_conflict_report_id.is_none());

    let fourth = service
        .begin_attempt(&target, "node-a", "background")
        .unwrap();
    service
        .settle_success(&target, "node-a", fourth, "background")
        .unwrap();
    let recovered = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .unwrap();
    assert_eq!(recovered.status, "healthy");
    assert!(recovered.failure_since.is_none());
    assert!(recovered.last_conflict_report_id.is_none());

    let fifth = service
        .begin_attempt(&target, "node-a", "background")
        .unwrap();
    let sixth = service.begin_attempt(&target, "node-a", "manual").unwrap();
    service
        .settle_success(&target, "node-a", sixth, "manual")
        .unwrap();
    service
        .settle_failure(
            &target,
            "node-a",
            fifth,
            "background",
            "late conflict",
            Some("stale-report"),
            Some("/run/conflicts/stale.json"),
            None,
        )
        .unwrap();
    let still_healthy = service
        .inspect(&target, "node-a", Duration::from_secs(900))
        .unwrap();
    assert_eq!(still_healthy.status, "healthy");
    assert_eq!(still_healthy.last_attempt_id, Some(sixth));
    assert_eq!(
        still_healthy.last_attempt_outcome.as_deref(),
        Some("succeeded")
    );
    assert!(still_healthy.failure_since.is_none());
    assert!(still_healthy.last_conflict_report_id.is_none());
    fs::remove_dir_all(temp).unwrap();
}

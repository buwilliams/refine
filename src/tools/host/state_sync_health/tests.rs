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
        last_success_at: Some(monitoring_since),
        failure_since: None,
        last_failure_at: None,
        last_error: None,
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

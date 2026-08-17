use super::*;

const FAILURE_BACKOFF_BASE: Duration = Duration::from_secs(2);
const FAILURE_BACKOFF_CAP: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Eq, PartialEq)]
struct FailureScope {
    target_root: PathBuf,
    node_id: String,
    local_fingerprint: u64,
}

#[derive(Debug, Default)]
pub(super) struct GitSyncFailureBackoff {
    scope: Option<FailureScope>,
    failure_context: Option<String>,
    consecutive_failures: u32,
    retry_at: Option<Instant>,
}

impl GitSyncFailureBackoff {
    pub(super) fn observe(&mut self, target_root: &Path, node_id: &str, local_fingerprint: u64) {
        let scope = FailureScope {
            target_root: target_root.to_path_buf(),
            node_id: node_id.to_string(),
            local_fingerprint,
        };
        if self.scope.as_ref() != Some(&scope) {
            self.reset();
            self.scope = Some(scope);
        }
    }

    pub(super) fn suppressed(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|retry_at| now < retry_at)
    }

    pub(super) fn allows_attempt(&self, now: Instant, explicit: bool) -> bool {
        explicit || !self.suppressed(now)
    }

    pub(super) fn record_failure(&mut self, now: Instant, failure_context: &str) -> Duration {
        if self.failure_context.as_deref() == Some(failure_context) {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        } else {
            self.failure_context = Some(failure_context.to_string());
            self.consecutive_failures = 1;
        }
        let shift = self.consecutive_failures.saturating_sub(1).min(31);
        let multiplier = 1_u32.checked_shl(shift).unwrap_or(u32::MAX);
        let delay = FAILURE_BACKOFF_BASE
            .saturating_mul(multiplier)
            .min(FAILURE_BACKOFF_CAP);
        self.retry_at = Some(now + delay);
        delay
    }

    pub(super) fn reset(&mut self) {
        self.failure_context = None;
        self.consecutive_failures = 0;
        self.retry_at = None;
    }
}

/// Keep retry identity stable across conflict-report rewrites. Report ids,
/// attempt ids, timestamps, locations, and recovery presentation all change on
/// every attempt even when the synchronization inputs and rejection are
/// identical; including them would pin every retry to the base delay.
pub(super) fn git_sync_failure_context(
    error: &RefineError,
    conflict_report: Option<&StateSyncConflictReport>,
) -> String {
    let Some(report) = conflict_report else {
        return error.to_string();
    };
    serde_json::to_string(&(
        "state_sync_conflict",
        report.phase.as_str(),
        &report.target_identity,
        &report.repository_identity,
        &report.configured_remote,
        &report.baseline_snapshot,
        &report.local_snapshot,
        &report.remote_snapshot,
        &report.local_state_head,
        &report.remote_state_head,
        &report.unresolved_paths,
        &report.reconciliation_outcomes,
    ))
    .unwrap_or_else(|_| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict_report(report_id: &str, attempt_id: &str) -> StateSyncConflictReport {
        serde_json::from_value(serde_json::json!({
            "version": 2,
            "report_id": report_id,
            "phase": "first_pass",
            "attempt_id": attempt_id,
            "attempt_source": "background_publish",
            "created_at": format!("2026-08-17T14:{attempt_id}:00Z"),
            "target_identity": "/target",
            "repository_identity": "repository",
            "configured_remote": "origin",
            "baseline_snapshot": "baseline",
            "local_snapshot": "local",
            "remote_snapshot": "remote",
            "local_state_head": "local-head",
            "remote_state_head": "remote-head",
            "unresolved_paths": ["nodes.json"],
            "reconciliation_outcomes": [{
                "path": "nodes.json",
                "outcome": "baseline_bytes_unavailable",
                "detail": "recorded baseline bytes were unavailable"
            }],
            "recovery": {
                "available": true,
                "preview_command": "preview",
                "apply_command": "apply"
            },
            "report_location": format!("run/{report_id}.json")
        }))
        .unwrap()
    }

    #[test]
    fn unchanged_failures_back_off_exponentially_and_cap() {
        let now = Instant::now();
        let mut backoff = GitSyncFailureBackoff::default();
        backoff.observe(Path::new("/target"), "node-a", 7);
        assert_eq!(
            backoff.record_failure(now, "conflict"),
            Duration::from_secs(2)
        );
        assert!(backoff.suppressed(now + Duration::from_secs(1)));
        assert_eq!(
            backoff.record_failure(now, "conflict"),
            Duration::from_secs(4)
        );
        for _ in 0..20 {
            backoff.record_failure(now, "conflict");
        }
        assert_eq!(backoff.record_failure(now, "conflict"), FAILURE_BACKOFF_CAP);
    }

    #[test]
    fn meaningful_scope_failure_and_success_changes_reset_backoff() {
        let now = Instant::now();
        let mut backoff = GitSyncFailureBackoff::default();
        backoff.observe(Path::new("/target"), "node-a", 7);
        backoff.record_failure(now, "conflict-a");
        assert!(backoff.suppressed(now));

        backoff.observe(Path::new("/target"), "node-a", 8);
        assert!(!backoff.suppressed(now));
        assert_eq!(
            backoff.record_failure(now, "conflict-a"),
            FAILURE_BACKOFF_BASE
        );
        assert_eq!(
            backoff.record_failure(now, "conflict-b"),
            FAILURE_BACKOFF_BASE
        );
        backoff.observe(Path::new("/target"), "node-b", 8);
        assert!(!backoff.suppressed(now));
        backoff.record_failure(now, "conflict-b");
        assert!(backoff.allows_attempt(now, true));
        backoff.reset();
        assert!(!backoff.suppressed(now));
    }

    #[test]
    fn rotating_conflict_report_evidence_keeps_the_same_backoff_context() {
        let error_a = RefineError::Conflict("conflict report report-a".to_string());
        let error_b = RefineError::Conflict("conflict report report-b".to_string());
        let report_a = conflict_report("report-a", "01");
        let report_b = conflict_report("report-b", "02");
        let context_a = git_sync_failure_context(&error_a, Some(&report_a));
        let context_b = git_sync_failure_context(&error_b, Some(&report_b));
        assert_eq!(context_a, context_b);

        let now = Instant::now();
        let mut backoff = GitSyncFailureBackoff::default();
        backoff.observe(Path::new("/target"), "node-a", 7);
        assert_eq!(
            backoff.record_failure(now, &context_a),
            FAILURE_BACKOFF_BASE
        );
        assert_eq!(
            backoff.record_failure(now, &context_b),
            FAILURE_BACKOFF_BASE * 2
        );

        let mut changed_report = report_b;
        changed_report.unresolved_paths = vec!["goals/G/goal.json".to_string()];
        let changed_context = git_sync_failure_context(&error_b, Some(&changed_report));
        assert_eq!(
            backoff.record_failure(now, &changed_context),
            FAILURE_BACKOFF_BASE
        );
    }
}

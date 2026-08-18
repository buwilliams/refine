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

/// Keep retry identity stable across conflict-report rewrites. What makes two
/// failures the same failure is the CONTENTION — the contested records and
/// the remote head that must be reconciled with them — not the report id: a
/// node still writing records snapshots a new local head every pass, minting
/// a new report id for a divergence whose contested question has not moved.
/// Keyed on the report id, that churn reset the escalating retry delay to its
/// base on every pass, so a busy node never backed off a standing conflict at
/// all. Real movement on either side still mints a new context.
pub(super) fn git_sync_failure_context(
    error: &RefineError,
    conflict_report: Option<&StateSyncConflictReport>,
) -> String {
    let Some(report) = conflict_report else {
        return error.to_string();
    };
    let mut contested = report.unresolved_paths.clone();
    contested.sort();
    format!(
        "state_sync_contention:{}:{}",
        report.remote_state_head,
        contested.join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict_report(
        local_state_head: &str,
        remote_state_head: &str,
        contested: &str,
        attempt_id: &str,
    ) -> StateSyncConflictReport {
        serde_json::from_value(serde_json::json!({
            "version": 3,
            // The report's identity is the divergence: every operand of it,
            // this node's own snapshot commit included.
            "report_id": format!("{local_state_head}-{remote_state_head}-{contested}"),
            "phase": "first_pass",
            "attempt_id": attempt_id,
            "attempt_source": "background_publish",
            "created_at": format!("2026-08-17T14:{attempt_id}:00Z"),
            "target_identity": "/target",
            "repository_identity": "repository",
            "configured_remote": "origin",
            "merge_base": "base-head",
            "local_state_head": local_state_head,
            "remote_state_head": remote_state_head,
            "unresolved_paths": [contested],
            "conflicts": [{
                "path": contested,
                "summary": "both nodes changed node-a"
            }],
            "recovery": {
                "run_command": "run",
                "preview_command": "preview"
            },
            "report_location": "run/state-sync-conflicts/latest.json"
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
    fn a_standing_contention_keeps_one_backoff_context_while_the_node_keeps_working() {
        let error_a = RefineError::Conflict("conflict report attempt 1".to_string());
        let error_b = RefineError::Conflict("conflict report attempt 2".to_string());
        // One standing contention, two passes. This node kept writing
        // records, so its snapshot moved the local head — and the report id
        // with it — while the contested record and the remote head that must
        // be reconciled with it did not move at all.
        let report_a = conflict_report("local-1", "remote-head", "nodes.json", "01");
        let report_b = conflict_report("local-2", "remote-head", "nodes.json", "02");
        assert_ne!(report_a.report_id, report_b.report_id);
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

        // The remote side moving is what genuinely needs deciding again, and
        // so is a different record being contested; either resets the delay.
        let moved_remote = conflict_report("local-2", "remote-moved", "nodes.json", "03");
        assert_eq!(
            backoff.record_failure(
                now,
                &git_sync_failure_context(&error_b, Some(&moved_remote))
            ),
            FAILURE_BACKOFF_BASE
        );
        let other_record =
            conflict_report("local-2", "remote-moved", "goals/AA/BB/goal.json", "04");
        assert_eq!(
            backoff.record_failure(
                now,
                &git_sync_failure_context(&error_b, Some(&other_record))
            ),
            FAILURE_BACKOFF_BASE
        );
    }
}

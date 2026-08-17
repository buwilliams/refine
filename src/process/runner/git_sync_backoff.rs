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

#[cfg(test)]
mod tests {
    use super::*;

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
}

use super::*;

static RETRY_STATE: OnceLock<Mutex<BTreeMap<String, RetryState>>> = OnceLock::new();

#[derive(Clone, Copy)]
struct RetryState {
    failures: u32,
    not_before: Instant,
}

impl WorkflowEngine {
    pub(crate) fn retry_observations(&self) -> BTreeMap<String, i64> {
        let prefix = self.retry_key("");
        RETRY_STATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(|(key, state)| {
                Some((
                    key.strip_prefix(&prefix)?.to_string(),
                    chrono::Utc::now().timestamp_millis()
                        + state
                            .not_before
                            .checked_duration_since(Instant::now())?
                            .as_millis() as i64,
                ))
            })
            .collect()
    }

    pub(super) fn retry_key(&self, goal_id: &str) -> String {
        format!(
            "{}:{}:{goal_id}",
            self.runtime_root.display(),
            self.target_root
                .as_deref()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        )
    }

    pub(super) fn retry_delayed(&self, goal_id: &str) -> bool {
        RETRY_STATE
            .get_or_init(Default::default)
            .lock()
            .ok()
            .and_then(|state| state.get(&self.retry_key(goal_id)).copied())
            .is_some_and(|state| state.not_before > Instant::now())
    }

    pub(super) fn record_retry(&self, goal_id: &str) {
        if let Ok(mut retries) = RETRY_STATE.get_or_init(Default::default).lock() {
            let key = self.retry_key(goal_id);
            let failures = retries
                .get(&key)
                .map(|state| state.failures.saturating_add(1))
                .unwrap_or(1);
            let delay = 5_u64.saturating_mul(1_u64 << failures.saturating_sub(1).min(6));
            retries.insert(
                key,
                RetryState {
                    failures,
                    not_before: Instant::now() + Duration::from_secs(delay.min(300)),
                },
            );
        }
    }

    pub(super) fn clear_retry(&self, goal_id: &str) {
        if let Ok(mut retries) = RETRY_STATE.get_or_init(Default::default).lock() {
            retries.remove(&self.retry_key(goal_id));
        }
    }
}

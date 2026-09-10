//! Immutable attempt provenance and fallible evidence finalization, independent of Goal writes.
use super::*;
use serde::Serialize;
use std::io::Write;

#[derive(Serialize)]
pub(super) struct OriginatingFailure {
    pub goal_id: String,
    pub round_idx: usize,
    pub workflow_revision: u64,
    pub failure_stage: String,
    pub failure_at: String,
    pub original_error: String,
}

#[derive(Serialize)]
struct FailureEvidence<'a> {
    #[serde(flatten)]
    origin: &'a OriginatingFailure,
    settlement: &'a FailureSettlement,
    runtime_evidence_persisted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    write_fault: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_outcome: Option<&'a FailureSettlement>,
}

impl OriginatingFailure {
    pub(super) fn new(
        goal: &str,
        authority: WorkflowAttemptAuthority,
        stage: &str,
        error: &RefineError,
    ) -> Self {
        Self {
            goal_id: goal.into(),
            round_idx: authority.round_idx,
            workflow_revision: authority.workflow_revision,
            failure_stage: stage.into(),
            failure_at: now_timestamp(),
            original_error: error.to_string(),
        }
    }

    pub(super) fn finalize(
        &self,
        engine: &WorkflowEngine,
        outcome: FailureSettlement,
    ) -> FailureSettlement {
        let evidence = FailureEvidence {
            origin: self,
            settlement: &outcome,
            runtime_evidence_persisted: true,
            write_fault: None,
            final_outcome: None,
        };
        let write = contain("runtime evidence finalization", || -> Result<(), String> {
            #[cfg(test)]
            self.hook(engine, "evidence_write")?;
            let bytes = serde_json::to_vec(&evidence).map_err(|e| e.to_string())?;
            let dir = engine.runtime_root.join("workflow-failures");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
            crate::infrastructure::process::subprocess::write_json_atomically(
                &path,
                &bytes,
                "workflow failure evidence",
            )
            .map_err(|e| e.to_string())
        })
        .and_then(|result| result);
        if let Err(fault) = write {
            let final_outcome = FailureSettlement::UnpersistedEvidence(format!(
                "{outcome:?}; runtime evidence: {fault}"
            ));
            let fallback = FailureEvidence {
                origin: self,
                settlement: &outcome,
                runtime_evidence_persisted: false,
                write_fault: Some(&fault),
                final_outcome: Some(&final_outcome),
            };
            // Reporting is fallible too: a closed stderr or a reporting panic cannot unwind
            // completion. The complete typed evidence is retained until this last boundary.
            let _ = contain("evidence reporting", || report(engine, &fallback));
            return final_outcome;
        }
        if !matches!(
            outcome,
            FailureSettlement::AuthoritativeFailure | FailureSettlement::ExistingVerifiedOutcome
        ) {
            let _ = contain("settlement reporting", || report(engine, &evidence));
        }
        outcome
    }

    #[cfg(test)]
    fn hook(&self, engine: &WorkflowEngine, stage: &str) -> Result<(), String> {
        crate::application::workflow::engine::test_hooks::run(
            engine,
            &self.goal_id,
            stage,
            WorkflowAttemptAuthority {
                round_idx: self.round_idx,
                workflow_revision: self.workflow_revision,
            },
        )
        .map_err(|e| e.to_string())
    }
}

fn report(_engine: &WorkflowEngine, evidence: &FailureEvidence<'_>) -> Result<(), String> {
    let bytes = serde_json::to_vec(evidence).map_err(|e| e.to_string())?;
    #[cfg(test)]
    {
        crate::application::workflow::engine::test_hooks::capture_failure(
            &_engine.runtime_root,
            &bytes,
        );
        evidence.origin.hook(_engine, "evidence_report")?;
    }
    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(b"refine workflow evidence: ")
        .and_then(|()| stderr.write_all(&bytes))
        .and_then(|()| stderr.write_all(b"\n"))
        .map_err(|e| e.to_string())
}

pub(super) fn contain<T>(stage: &str, action: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).map_err(|payload| {
        let reason = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic payload");
        format!("{stage} panicked: {reason}")
    })
}

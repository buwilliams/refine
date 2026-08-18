use crate::error::RefineResult;

/// Maximum number of diagnostic follow-up invocations after an initial
/// structured response fails parsing or contract validation. Each follow-up is
/// a full provider invocation, so this is deliberately small; the diagnostic it
/// carries names the exact transport, schema-path, or validation fault.
pub const DIAGNOSTIC_REPAIR_ATTEMPTS: usize = 2;

/// Maximum number of completion-signal file replacements a live Goal Agent
/// session may be asked for after rejected payloads. Intentionally higher than
/// [`DIAGNOSTIC_REPAIR_ATTEMPTS`]: a replacement costs one in-session file
/// rewrite, not a fresh provider invocation.
pub const MAX_INVALID_SIGNAL_REPLACEMENTS: usize = 3;

/// How many diagnostic follow-ups a repair loop may spend.
#[derive(Clone, Copy, Debug)]
pub struct RepairPolicy {
    pub max_repairs: usize,
}

impl Default for RepairPolicy {
    fn default() -> Self {
        Self {
            max_repairs: DIAGNOSTIC_REPAIR_ATTEMPTS,
        }
    }
}

/// What a repair invocation must relay to the agent: the exact diagnostic and
/// the retained invalid response, alongside the attempt position.
#[derive(Clone, Debug)]
pub struct RepairDirective {
    /// 1-based repair number (the first repair after the initial attempt is 1).
    pub attempt: usize,
    pub max_repairs: usize,
    pub diagnostics: String,
    pub raw_output: String,
}

/// One settled invocation attempt, handed to the persistence hook.
#[derive(Clone, Copy, Debug)]
pub struct AttemptOutcome<'a> {
    /// 1-based invocation number (initial attempt is 1).
    pub attempt: usize,
    pub raw_output: &'a str,
    /// The decode diagnostic; `None` when the attempt was accepted.
    pub diagnostics: Option<&'a str>,
    /// True when this rejected attempt was the last one the policy allows.
    pub exhausted: bool,
}

/// Drive one structured-output interaction to acceptance or exhaustion.
///
/// `invoke(None)` runs the initial prompt; `invoke(Some(directive))` runs a
/// repair carrying the prior diagnostic. `output` projects the decodable text
/// out of the site's invocation record, `decode` applies the contract, and
/// `on_attempt` is the site's persistence hook for both accepted and rejected
/// attempts (rejected ones carry `diagnostics`). On exhaustion the final
/// decode error is returned unless the hook itself fails first.
pub fn run_with_repair<R, T>(
    policy: &RepairPolicy,
    mut invoke: impl FnMut(Option<&RepairDirective>) -> RefineResult<R>,
    output: impl Fn(&R) -> &str,
    decode: impl Fn(&str) -> RefineResult<T>,
    mut on_attempt: impl FnMut(&R, AttemptOutcome<'_>) -> RefineResult<()>,
) -> RefineResult<(R, T)> {
    let mut directive = None;
    for attempt in 1..=policy.max_repairs + 1 {
        let record = invoke(directive.as_ref())?;
        let raw_output = output(&record).to_string();
        match decode(&raw_output) {
            Ok(value) => {
                on_attempt(
                    &record,
                    AttemptOutcome {
                        attempt,
                        raw_output: &raw_output,
                        diagnostics: None,
                        exhausted: false,
                    },
                )?;
                return Ok((record, value));
            }
            Err(error) => {
                let diagnostics = error.to_string();
                let exhausted = attempt > policy.max_repairs;
                on_attempt(
                    &record,
                    AttemptOutcome {
                        attempt,
                        raw_output: &raw_output,
                        diagnostics: Some(&diagnostics),
                        exhausted,
                    },
                )?;
                if exhausted {
                    return Err(error);
                }
                directive = Some(RepairDirective {
                    attempt,
                    max_repairs: policy.max_repairs,
                    diagnostics,
                    raw_output,
                });
            }
        }
    }
    unreachable!("bounded structured-output repair loop always returns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RefineError;

    fn decode_ready(output: &str) -> RefineResult<usize> {
        match output {
            "ready" => Ok(7),
            other => Err(RefineError::Serialization(format!("not ready: {other}"))),
        }
    }

    #[test]
    fn accepts_the_first_valid_attempt_without_repairs() {
        let mut attempts = Vec::new();
        let (record, value) = run_with_repair(
            &RepairPolicy::default(),
            |directive| {
                assert!(directive.is_none());
                Ok("ready".to_string())
            },
            String::as_str,
            decode_ready,
            |_, outcome| {
                attempts.push((outcome.attempt, outcome.diagnostics.is_none()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(record, "ready");
        assert_eq!(value, 7);
        assert_eq!(attempts, vec![(1, true)]);
    }

    #[test]
    fn relays_diagnostics_through_the_directive_and_repairs() {
        let mut attempts = Vec::new();
        let (_, value) = run_with_repair(
            &RepairPolicy::default(),
            |directive| match directive {
                None => Ok("draft".to_string()),
                Some(directive) => {
                    assert_eq!(directive.attempt, 1);
                    assert_eq!(directive.max_repairs, DIAGNOSTIC_REPAIR_ATTEMPTS);
                    assert_eq!(directive.diagnostics, "not ready: draft");
                    assert_eq!(directive.raw_output, "draft");
                    Ok("ready".to_string())
                }
            },
            String::as_str,
            decode_ready,
            |_, outcome| {
                attempts.push((
                    outcome.attempt,
                    outcome.diagnostics.is_some(),
                    outcome.exhausted,
                ));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(value, 7);
        assert_eq!(attempts, vec![(1, true, false), (2, false, false)]);
    }

    #[test]
    fn exhaustion_returns_the_final_decode_error_after_the_hook_runs() {
        let mut attempts = Vec::new();
        let error = run_with_repair(
            &RepairPolicy::default(),
            |_| Ok("draft".to_string()),
            String::as_str,
            decode_ready,
            |_, outcome| {
                attempts.push((outcome.attempt, outcome.exhausted));
                Ok(())
            },
        )
        .map(|(_, value): (String, usize)| value)
        .unwrap_err();
        assert!(error.to_string().contains("not ready: draft"));
        assert_eq!(attempts, vec![(1, false), (2, false), (3, true)]);
    }
}

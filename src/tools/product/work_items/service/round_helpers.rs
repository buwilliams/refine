use super::*;

pub(super) fn attach_latest_log_fields(
    round: &mut Map<String, Value>,
    logs: &[crate::model::log::RoundLogEntry],
) -> RefineResult<()> {
    let latest_log = logs.last();
    let latest_error_log = logs
        .iter()
        .rev()
        .find(|log| log.entry.severity == "error" || log.entry.severity == "warn");
    let latest_state_log = logs.iter().rev().find(|log| log.entry.category == "state");
    let latest_workflow_log = logs
        .iter()
        .rev()
        .find(|log| log.entry.message.contains("Workflow status changed:"));
    for (key, value) in [
        ("latest_log", latest_log),
        ("latest_error_log", latest_error_log),
        ("latest_state_log", latest_state_log),
        ("latest_workflow_log", latest_workflow_log),
    ] {
        if let Some(log) = value {
            let value = serde_json::to_value(log).map_err(|error| {
                RefineError::Serialization(format!("failed to encode latest Goal log: {error}"))
            })?;
            round.insert(key.to_string(), value);
        }
    }
    Ok(())
}

/// A recorded failure describes the round that failed. Retrying a Goal reuses
/// that same round, so leaving the reason behind would show a live failure on
/// work that has since moved on.
pub(super) fn clear_latest_round_failure(object: &mut Map<String, Value>) {
    let Some(round) = object
        .get_mut("rounds")
        .and_then(Value::as_array_mut)
        .and_then(|rounds| rounds.last_mut())
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for key in ["failure_category", "failure_message", "failure_at"] {
        if round.contains_key(key) {
            round.insert(key.to_string(), Value::String(String::new()));
        }
    }
}

pub(super) fn clear_latest_round_workflow_attempt(object: &mut Map<String, Value>) {
    let Some(round) = object
        .get_mut("rounds")
        .and_then(Value::as_array_mut)
        .and_then(|rounds| rounds.last_mut())
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    round.insert("workflow_attempt_authority".to_string(), Value::Null);
}

/// True when the trailing Round was appended by automation and never started:
/// it carries recovery provenance (a non-null `automatic_retry` or
/// `workflow_recovery`), has no logs, and was never worked (no agent context
/// or implementation report). A claim on the Round disqualifies it unless it
/// is exactly `retiring_authority` — the caller's own live attempt, which the
/// queue operation is about to retire; production recoveries always queue
/// from inside a claimed attempt, so treating the caller's claim as work
/// would make reuse unreachable.
pub(super) fn last_round_is_unstarted_recovery(
    rounds: &[Value],
    retiring_authority: Option<WorkflowAttemptAuthority>,
) -> bool {
    let Some(round) = rounds.last().and_then(Value::as_object) else {
        return false;
    };
    let automation_appended = ["automatic_retry", "workflow_recovery"]
        .iter()
        .any(|key| round.get(*key).is_some_and(|value| !value.is_null()));
    let never_logged = round
        .get("logs")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty);
    let never_worked = ["agent_context", "implementation_report"]
        .iter()
        .all(|key| round.get(*key).is_none_or(Value::is_null));
    let unclaimed_or_retiring = match round.get("workflow_attempt_authority") {
        None | Some(Value::Null) => true,
        Some(claim) => retiring_authority.is_some_and(|authority| {
            super::workflow_attempts::claim_matches_authority(claim, authority)
        }),
    };
    automation_appended && never_logged && never_worked && unclaimed_or_retiring
}

/// Only the last Round of a Goal is claimable (`claim_workflow_attempt`
/// requires `rounds.len() == round_idx + 1`), so appending a recovery Round
/// past an unclaimed one strands the earlier Round forever: it can never be
/// started and never receives logs explaining why. When the trailing Round is
/// itself an unstarted automation-appended recovery Round, the new recovery
/// replaces it in place instead of appending.
///
/// The reuse merges every key of `successor` over the trailing Round and
/// restores only the original "created" value. The full merge deliberately
/// resets `failure_*` to empty — the same rationale as
/// `clear_latest_round_failure` above — while keys absent from `successor`,
/// like `*_recovery_analysis`, survive. Returns the effective successor index.
///
/// A reused Round is its own nominal source (every queue site computes
/// `source_round` from the trailing Round it is recovering from), so the
/// merged retry markers would point at the Round itself. Consumers such as
/// the Governance identical-signature early stop resolve `source_round` to
/// read the source Round's failure evidence, and a self-reference would make
/// them compare a fresh failure against itself. The reuse therefore keeps the
/// inert Round's original lineage pointer.
pub(super) fn append_or_reuse_recovery_round(
    rounds: &mut Vec<Value>,
    successor: Value,
    reuse_inert: bool,
) -> usize {
    if reuse_inert {
        let last_idx = rounds.len().saturating_sub(1);
        if let (Some(reused), Some(successor)) = (
            rounds.last_mut().and_then(Value::as_object_mut),
            successor.as_object(),
        ) {
            let created = reused.get("created").cloned();
            let original_source = ["automatic_retry", "workflow_recovery"]
                .iter()
                .find_map(|key| reused.get(*key)?.get("source_round")?.as_u64());
            for (key, value) in successor {
                reused.insert(key.clone(), value.clone());
            }
            if let Some(created) = created {
                reused.insert("created".to_string(), created);
            }
            if let Some(source_round) = original_source {
                for key in ["automatic_retry", "workflow_recovery"] {
                    if let Some(marker) = reused.get_mut(key).and_then(Value::as_object_mut)
                        && marker.contains_key("source_round")
                    {
                        marker.insert("source_round".to_string(), Value::from(source_round));
                    }
                }
            }
            return last_idx;
        }
    }
    rounds.push(successor);
    rounds.len() - 1
}

pub(super) fn new_round_value(reporter: &str, assignee: &str, prompt: &str) -> Value {
    let now = now_timestamp();
    let mut round = Map::new();
    round.insert("reporter".to_string(), Value::String(reporter.to_string()));
    round.insert("assignee".to_string(), Value::String(assignee.to_string()));
    round.insert("prompt".to_string(), Value::String(prompt.to_string()));
    round.insert("created".to_string(), Value::String(now.clone()));
    round.insert("updated".to_string(), Value::String(now));
    round.insert("logs".to_string(), Value::Array(Vec::new()));
    round.insert("implementation_report".to_string(), Value::Null);
    round.insert("implementation_reported_at".to_string(), Value::Null);
    round.insert("agent_context".to_string(), Value::Null);
    round.insert("implementation_plan".to_string(), Value::Null);
    round.insert("guidance_decision".to_string(), Value::Null);
    round.insert("workflow_reconciliation".to_string(), Value::Null);
    round.insert("workflow_recovery".to_string(), Value::Null);
    round.insert("workflow_attempt_authority".to_string(), Value::Null);
    round.insert(
        "rule_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "meta_rule_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "product_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "constitution_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert(
        "governance_message".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_details".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_checked_at".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_candidate_commit".to_string(),
        Value::String(String::new()),
    );
    round.insert(
        "governance_rule_actions".to_string(),
        Value::Array(Vec::new()),
    );
    round.insert(
        "quality_state".to_string(),
        Value::String("unclassified".to_string()),
    );
    round.insert("quality_message".to_string(), Value::String(String::new()));
    round.insert("quality_details".to_string(), Value::String(String::new()));
    round.insert(
        "quality_checked_at".to_string(),
        Value::String(String::new()),
    );
    round.insert("failure_category".to_string(), Value::String(String::new()));
    round.insert("failure_message".to_string(), Value::String(String::new()));
    round.insert("failure_at".to_string(), Value::String(String::new()));
    Value::Object(round)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recovery_round_reuse_keeps_the_successor_pointer_truthful() {
        let mut rounds = vec![
            json!({"created": "2026-01-01T00:00:00Z", "logs": [{"message": "worked"}]}),
            json!({
                "created": "2026-01-02T00:00:00Z",
                "logs": [],
                "automatic_retry": {"kind": "integration", "source_round": 1, "attempt": 1},
                "workflow_attempt_authority": null,
                "agent_context": null,
                "implementation_report": null
            }),
        ];
        let reuse_inert = last_round_is_unstarted_recovery(&rounds, None);
        assert!(reuse_inert);

        // Mirrors queue_integration_recovery_summary's successor arithmetic.
        let round_idx = 1usize;
        let successor_round = round_idx + if reuse_inert { 1 } else { 2 };
        let successor = json!({
            "created": "2026-01-03T00:00:00Z",
            "prompt": "recover integration",
            "workflow_recovery": {
                "state": "queued",
                "source_round": round_idx + 1,
                "successor_round": successor_round
            },
            "automatic_retry": {"kind": "integration", "source_round": round_idx + 1, "attempt": 2}
        });
        let effective = append_or_reuse_recovery_round(&mut rounds, successor, reuse_inert);

        assert_eq!(effective, 1);
        assert_eq!(rounds.len(), 2);
        assert_eq!(
            rounds[1]["workflow_recovery"]["successor_round"],
            json!(effective + 1)
        );
        // The reused Round keeps its original lineage: pointing the markers
        // at the Round itself would make source-evidence consumers read the
        // fresh failure as its own source.
        assert_eq!(rounds[1]["automatic_retry"]["source_round"], 1);
        assert_eq!(rounds[1]["workflow_recovery"]["source_round"], 1);
        assert_eq!(rounds[1]["created"], "2026-01-02T00:00:00Z");
        assert_eq!(rounds[1]["automatic_retry"]["attempt"], 2);
    }

    #[test]
    fn a_round_claimed_by_the_retiring_attempt_is_still_inert() {
        let rounds = vec![json!({
            "automatic_retry": {"kind": "governance", "source_round": 1, "attempt": 1},
            "logs": [],
            "workflow_attempt_authority": {"round_idx": 1, "workflow_revision": 7},
            "agent_context": null,
            "implementation_report": null
        })];
        let retiring = WorkflowAttemptAuthority {
            round_idx: 1,
            workflow_revision: 7,
        };
        assert!(last_round_is_unstarted_recovery(&rounds, Some(retiring)));
        // A claim from any other attempt still disqualifies the Round.
        assert!(!last_round_is_unstarted_recovery(&rounds, None));
        assert!(!last_round_is_unstarted_recovery(
            &rounds,
            Some(WorkflowAttemptAuthority {
                round_idx: 1,
                workflow_revision: 8,
            })
        ));
    }

    #[test]
    fn a_worked_or_authored_round_is_never_treated_as_inert() {
        for round in [
            json!({"automatic_retry": {"attempt": 1}, "logs": [{"message": "started"}]}),
            json!({
                "automatic_retry": {"attempt": 1},
                "logs": [],
                "workflow_attempt_authority": {"round_idx": 0}
            }),
            json!({"automatic_retry": {"attempt": 1}, "logs": [], "agent_context": {"version": 1}}),
            json!({"workflow_recovery": {"state": "queued"}, "implementation_report": "done"}),
            json!({"prompt": "authored by a person", "logs": []}),
        ] {
            assert!(
                !last_round_is_unstarted_recovery(&[round.clone()], None),
                "{round:#}"
            );
            let mut rounds = vec![round];
            let successor = new_round_value("Refine", "Refine", "recover");
            assert_eq!(
                append_or_reuse_recovery_round(&mut rounds, successor, false),
                1
            );
            assert_eq!(rounds.len(), 2);
        }
    }
}

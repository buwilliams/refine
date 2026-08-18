//! Goal ownership invariants derived from the actual three-way state operands.
//!
//! Ownership moves only through an explicit Goal transfer. Reconciliation may
//! compose every other member, but it may neither infer ownership from where
//! a merge runs nor restore the merge-base owner after one side transferred a
//! Goal. Missing or invalid evidence stays ambiguous until explicit recovery.

use std::collections::BTreeMap;
use std::path::Path;

use crate::model::fleet::valid_node_id;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GoalOwnership {
    Preserve { node_id: String, transferred: bool },
    Ambiguous { reason: String },
}

impl GoalOwnership {
    pub(crate) fn validate_result(&self, path: &str, bytes: Option<&[u8]>) -> Result<(), String> {
        match self {
            Self::Ambiguous { reason } => Err(format!(
                "{path} has ambiguous Goal ownership ({reason}) and cannot be resolved automatically"
            )),
            Self::Preserve { node_id, .. } => {
                let Some(bytes) = bytes else {
                    return Err(format!(
                        "{path} must preserve explicit Goal owner {node_id} and cannot delete the Goal"
                    ));
                };
                let actual = valid_goal_owner(Some(bytes)).map_err(|reason| {
                    format!("{path} must preserve explicit Goal owner {node_id}, but the result {reason}")
                })?;
                if actual.node_id != *node_id {
                    return Err(format!(
                        "{path} must preserve explicit Goal owner {node_id}, not {}",
                        actual.node_id
                    ));
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct GoalOwnershipPolicy {
    decisions: BTreeMap<String, GoalOwnership>,
}

impl GoalOwnershipPolicy {
    pub(crate) fn include(
        &mut self,
        path: &str,
        base: Option<&[u8]>,
        local: Option<&[u8]>,
        remote: Option<&[u8]>,
    ) {
        if let Some(decision) = classify_goal_ownership(path, base, local, remote) {
            self.decisions.insert(path.to_string(), decision);
        }
    }

    pub(crate) fn decision(&self, path: &str) -> Option<&GoalOwnership> {
        self.decisions.get(path)
    }

    pub(crate) fn ambiguities(&self) -> Vec<(&str, &str)> {
        self.decisions
            .iter()
            .filter_map(|(path, decision)| match decision {
                GoalOwnership::Ambiguous { reason } => Some((path.as_str(), reason.as_str())),
                GoalOwnership::Preserve { .. } => None,
            })
            .collect()
    }

    pub(crate) fn decision_question(&self) -> Option<String> {
        let ambiguities = self.ambiguities();
        (!ambiguities.is_empty()).then(|| {
            let detail = ambiguities
                .iter()
                .map(|(path, reason)| format!("{path}: {reason}"))
                .collect::<Vec<_>>()
                .join("; ");
            format!(
                "Goal ownership needs an explicit decision because a one-sided transfer cannot be proven ({detail}). Keep the current Goal unchanged, then choose the authoritative record through `refine sync --authority live|remote` (with `--path` exceptions as needed), or perform the intended move through a supported Goal-transfer surface. State sync will not infer an owner."
            )
        })
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &GoalOwnership)> {
        self.decisions
            .iter()
            .map(|(path, decision)| (path.as_str(), decision))
    }
}

pub(crate) fn is_goal_record(path: &str) -> bool {
    let relative = path.strip_prefix(".refine/").unwrap_or(path);
    Path::new(relative)
        .file_name()
        .and_then(|name| name.to_str())
        == Some("goal.json")
}

pub(crate) fn classify_goal_ownership(
    path: &str,
    base: Option<&[u8]>,
    local: Option<&[u8]>,
    remote: Option<&[u8]>,
) -> Option<GoalOwnership> {
    if !is_goal_record(path) {
        return None;
    }
    let base = match valid_goal_owner(base) {
        Ok(owner) => owner,
        Err(reason) => {
            return Some(GoalOwnership::Ambiguous {
                reason: format!("merge-base operand {reason}"),
            });
        }
    };
    let local = match valid_goal_owner(local) {
        Ok(owner) => owner,
        Err(reason) => {
            return Some(GoalOwnership::Ambiguous {
                reason: format!("local operand {reason}"),
            });
        }
    };
    let remote = match valid_goal_owner(remote) {
        Ok(owner) => owner,
        Err(reason) => {
            return Some(GoalOwnership::Ambiguous {
                reason: format!("remote operand {reason}"),
            });
        }
    };
    if base.id != local.id || base.id != remote.id {
        return Some(GoalOwnership::Ambiguous {
            reason: "does not carry one stable Goal id across all three operands".to_string(),
        });
    }

    let local_changed = local.node_id != base.node_id;
    let remote_changed = remote.node_id != base.node_id;
    match (local_changed, remote_changed) {
        (false, false) => Some(GoalOwnership::Preserve {
            node_id: base.node_id,
            transferred: false,
        }),
        (true, false) => Some(GoalOwnership::Preserve {
            node_id: local.node_id,
            transferred: true,
        }),
        (false, true) => Some(GoalOwnership::Preserve {
            node_id: remote.node_id,
            transferred: true,
        }),
        (true, true) if local.node_id == remote.node_id => Some(GoalOwnership::Preserve {
            node_id: local.node_id,
            transferred: true,
        }),
        (true, true) => Some(GoalOwnership::Ambiguous {
            reason: format!(
                "Goal {} was transferred from {} to {} locally and {} remotely",
                base.id, base.node_id, local.node_id, remote.node_id
            ),
        }),
    }
}

struct ValidGoalOwner {
    id: String,
    node_id: String,
}

fn valid_goal_owner(bytes: Option<&[u8]>) -> Result<ValidGoalOwner, String> {
    let Some(bytes) = bytes else {
        return Err("is missing or deleted".to_string());
    };
    let goal = serde_json::from_slice::<serde_json::Value>(bytes)
        .map_err(|error| format!("is not valid JSON: {error}"))?;
    let Some(goal) = goal.as_object() else {
        return Err("is not a JSON object".to_string());
    };
    let Some(id) = goal.get("id").and_then(serde_json::Value::as_str) else {
        return Err("has no string Goal id".to_string());
    };
    if id.trim().is_empty() {
        return Err("has an empty Goal id".to_string());
    }
    let Some(node_id) = goal.get("node_id").and_then(serde_json::Value::as_str) else {
        return Err("has no string node_id".to_string());
    };
    if node_id != node_id.trim() || !valid_node_id(node_id) {
        return Err(format!("has invalid node_id {node_id:?}"));
    }
    Ok(ValidGoalOwner {
        id: id.to_string(),
        node_id: node_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(id: &str, owner: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "id": id,
            "name": id,
            "status": "todo",
            "priority": "low",
            "reporter": null,
            "branch_name": null,
            "feature_id": null,
            "feature_order": null,
            "node_id": owner,
            "created": "2026-08-18T08:00:00Z",
            "updated": "2026-08-18T08:00:00Z",
            "notes": [],
            "rounds": []
        }))
        .unwrap()
    }

    #[test]
    fn classifies_both_one_sided_transfer_orientations() {
        let base = goal("GOALA", serde_json::json!("node-a"));
        let transferred = goal("GOALA", serde_json::json!("node-b"));
        assert_eq!(
            classify_goal_ownership(
                "goals/GO/ALA/goal.json",
                Some(&base),
                Some(&transferred),
                Some(&base)
            ),
            Some(GoalOwnership::Preserve {
                node_id: "node-b".to_string(),
                transferred: true
            })
        );
        assert_eq!(
            classify_goal_ownership(
                "goals/GO/ALA/goal.json",
                Some(&base),
                Some(&base),
                Some(&transferred)
            ),
            Some(GoalOwnership::Preserve {
                node_id: "node-b".to_string(),
                transferred: true
            })
        );
    }

    #[test]
    fn competing_missing_and_malformed_operands_are_ambiguous() {
        let base = goal("GOALA", serde_json::json!("node-a"));
        let local = goal("GOALA", serde_json::json!("node-b"));
        let remote = goal("GOALA", serde_json::json!("node-c"));
        for decision in [
            classify_goal_ownership(
                "goals/GO/ALA/goal.json",
                Some(&base),
                Some(&local),
                Some(&remote),
            ),
            classify_goal_ownership("goals/GO/ALA/goal.json", Some(&base), Some(&local), None),
            classify_goal_ownership(
                "goals/GO/ALA/goal.json",
                Some(&base),
                Some(&local),
                Some(br#"{}"#),
            ),
            classify_goal_ownership(
                "goals/GO/ALA/goal.json",
                Some(&base),
                Some(&local),
                Some(&goal("GOALA", serde_json::json!("Bad Node"))),
            ),
        ] {
            assert!(
                matches!(decision, Some(GoalOwnership::Ambiguous { .. })),
                "{decision:?}"
            );
        }
    }
}

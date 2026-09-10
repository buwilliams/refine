//! Agent input is a projection of authored work and relevant evidence, not its ledger.
use serde_json::{Value, json};

fn select(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| value.get(*key).map(|v| ((*key).into(), v.clone())))
            .collect(),
    )
}

pub fn goal_context(goal: &Value) -> Value {
    let mut context = select(
        goal,
        &[
            "id",
            "name",
            "description",
            "status",
            "node_id",
            "priority",
            "notes",
            "feature_id",
            "feature_order",
            "assignee",
            "reporter",
            "created",
            "updated",
            "branch_name",
            "target_branch",
            "base_commit",
            "candidate_commit",
            "event_generation",
            "workflow_revision",
        ],
    );
    context["rounds"] = json!(
        goal["rounds"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|round| {
                let mut projected = select(
                    round,
                    &[
                        "prompt",
                        "created",
                        "updated",
                        "implementation_report",
                        "implementation_reported_at",
                        "quality_agent_report",
                        "quality_state",
                        "quality_message",
                        "quality_candidate_commit",
                        "governance_message",
                        "governance_candidate_commit",
                        "failure_category",
                        "failure_message",
                        "automatic_retry",
                        "quality_recovery_analysis",
                        "quality_recovery_round_prompt",
                        "governance_recovery_analysis",
                        "governance_recovery_round_prompt",
                    ],
                );
                if let Some(plan) = round.get("implementation_plan") {
                    projected["implementation_plan"] = select(
                        plan,
                        &["state", "final_plan", "accepted_plans", "checklist"],
                    );
                }
                projected
            })
            .collect::<Vec<_>>()
    );
    context
}

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptTemplate, render};

mod markdown;

use markdown::*;

const GOAL_IDENTITY_FIELDS: &[(&str, &str)] = &[
    ("id", "Goal ID"),
    ("name", "Goal"),
    ("node_id", "Node"),
    ("reporter", "Reporter"),
    ("assignee", "Assignee"),
];
const GOAL_WHAT_FIELDS: &[(&str, &str)] = &[
    ("priority", "Priority"),
    ("feature_id", "Feature"),
    ("feature_order", "Feature Order"),
];
const ROUND_METADATA_FIELDS: &[(&str, &str)] =
    &[("reporter", "Reporter"), ("assignee", "Assignee")];
const ROUND_EVIDENCE_FIELDS: &[(&str, &str)] = &[
    ("implementation_plan", "Implementation Plan Evidence"),
    ("implementation_report", "Implementation Report"),
    ("guidance_decision", "Guidance Decision"),
    ("rule_state", "Rule State"),
    ("meta_rule_state", "Meta Rule State"),
    ("product_state", "Product State"),
    ("constitution_state", "Constitution State"),
    ("governance_message", "Governance Outcome"),
    ("governance_details", "Governance Details"),
    ("governance_rule_actions", "Governance Actions"),
    ("quality_state", "Quality State"),
    ("quality_message", "Quality Outcome"),
    ("quality_details", "Quality Evidence"),
];
const ROUND_OMITTED_FIELDS: &[&str] = &[
    "implementation_reported_at",
    "governance_checked_at",
    "quality_checked_at",
    "logs",
    "agent_context",
];

pub(super) fn goal_agent_prompt(goal_id: &str, agent_context: &Value) -> RefineResult<String> {
    let spec = render_goal_agent_spec(goal_id, agent_context)?;
    Ok(render(PromptTemplate::GoalAgent, &[("spec", &spec)]))
}

fn render_goal_agent_spec(goal_id: &str, agent_context: &Value) -> RefineResult<String> {
    let context = required_object(
        agent_context,
        format!("Goal {goal_id} agent context is not an object"),
    )?;
    let goal = required_object_field(
        context,
        "goal",
        format!("Goal {goal_id} has no pinned Goal context"),
    )?;
    let previous_rounds = context
        .get("previous_rounds")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RefineError::Serialization(format!("Goal {goal_id} has no pinned previous Rounds"))
        })?;
    let current_round = required_object_field(
        context,
        "current_round",
        format!("Goal {goal_id} has no pinned current Round"),
    )?;
    let latest_round_request = current_round
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty() && *prompt != "unclassified")
        .ok_or_else(|| {
            RefineError::Serialization(format!(
                "Goal {goal_id} pinned current Round has no request"
            ))
        })?;

    let refine_context = render_refine_context(context, goal);
    let what = render_what(context, goal);
    let why = render_why(context, goal);
    let rules = render_rules(context);
    let previous_rounds = render_previous_rounds(previous_rounds);
    let latest_round_context = render_round_context(current_round);

    Ok(render(
        PromptTemplate::GoalAgentSpec,
        &[
            ("refine_context", &refine_context),
            ("what", &what),
            ("why", &why),
            ("rules", &rules),
            ("previous_rounds", &previous_rounds),
            ("latest_round_context", &latest_round_context),
            ("latest_round_request", latest_round_request),
        ],
    ))
}

fn required_object(value: &Value, message: String) -> RefineResult<&Map<String, Value>> {
    value.as_object().ok_or(RefineError::Serialization(message))
}

fn required_object_field<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    message: String,
) -> RefineResult<&'a Map<String, Value>> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or(RefineError::Serialization(message))
}

fn render_refine_context(context: &Map<String, Value>, goal: &Map<String, Value>) -> String {
    let mut output = String::new();
    if let Some(summary) = meaningful_string(context.get("workflow_summary")) {
        push_prose(&mut output, "### Workflow and Review Boundary", summary);
    }

    let mut identity = String::new();
    if let Some(version) = context.get("version").filter(|value| meaningful(value)) {
        push_scalar_field(&mut identity, "Pinned Context Version", version);
    }
    for (key, label) in GOAL_IDENTITY_FIELDS {
        if let Some(value) = goal.get(*key).filter(|value| meaningful(value)) {
            push_named_value(&mut identity, label, value, 4);
        }
    }
    if !identity.is_empty() {
        push_heading(&mut output, 3, "Goal Identity");
        output.push_str(identity.trim_end());
        output.push('\n');
    }

    let known_context = [
        "version",
        "assembled_at",
        "workflow_summary",
        "governance",
        "guidance_candidates",
        "goal",
        "previous_rounds",
        "current_round",
    ];
    let additional = render_object_fields(context, &known_context, 3);
    if !additional.is_empty() {
        push_heading(&mut output, 3, "Additional Pinned Context");
        output.push_str(additional.trim_end());
        output.push('\n');
    }

    fallback(
        output,
        "No additional workflow or Goal identity context was pinned.",
    )
}

fn render_what(context: &Map<String, Value>, goal: &Map<String, Value>) -> String {
    let mut output = String::new();
    if let Some(product) = context
        .get("governance")
        .and_then(Value::as_object)
        .and_then(|governance| meaningful_string(governance.get("product")))
    {
        push_prose(&mut output, "### Product Intent", product);
    }

    let mut metadata = String::new();
    for (key, label) in GOAL_WHAT_FIELDS {
        if let Some(value) = goal.get(*key).filter(|value| meaningful(value)) {
            push_named_value(&mut metadata, label, value, 4);
        }
    }
    let known_goal = [
        "id",
        "name",
        "node_id",
        "reporter",
        "assignee",
        "priority",
        "feature_id",
        "feature_order",
        "notes",
    ];
    metadata.push_str(&render_object_fields(goal, &known_goal, 4));
    if !metadata.is_empty() {
        push_heading(&mut output, 3, "Goal Metadata");
        output.push_str(metadata.trim_end());
        output.push('\n');
    }

    fallback(
        output,
        "No additional product intent or Goal metadata was pinned.",
    )
}

fn render_why(context: &Map<String, Value>, goal: &Map<String, Value>) -> String {
    let mut output = String::new();
    let governance = context.get("governance").and_then(Value::as_object);
    if let Some(constitution) =
        governance.and_then(|value| meaningful_string(value.get("constitution")))
    {
        push_prose(&mut output, "### Constitution", constitution);
    }
    if let Some(configured) = governance
        .and_then(|value| value.get("configured"))
        .filter(|value| meaningful(value))
    {
        push_scalar_field(&mut output, "Governance Configured", configured);
    }
    if let Some(notes) = goal.get("notes").filter(|value| meaningful(value)) {
        push_named_value(&mut output, "Goal Notes", notes, 3);
    }
    if let Some(governance) = governance {
        let additional = render_object_fields(
            governance,
            &["product", "constitution", "configured", "rules"],
            3,
        );
        if !additional.is_empty() {
            push_heading(&mut output, 3, "Additional Rationale");
            output.push_str(additional.trim_end());
            output.push('\n');
        }
    }

    fallback(
        output,
        "No Constitution, notes, or additional rationale was pinned.",
    )
}

fn render_rules(context: &Map<String, Value>) -> String {
    let mut output = String::new();
    let governance_rules = context
        .get("governance")
        .and_then(Value::as_object)
        .and_then(|governance| governance.get("rules"))
        .and_then(Value::as_array);
    if let Some(rules) = governance_rules {
        let rendered = rules
            .iter()
            .filter(|rule| meaningful(rule))
            .enumerate()
            .map(|(index, rule)| render_rule(index + 1, rule))
            .collect::<Vec<_>>();
        if !rendered.is_empty() {
            push_heading(&mut output, 3, "Governance Rules");
            output.push_str(&rendered.join("\n"));
            output.push('\n');
        }
    }

    let guidance = context
        .get("guidance_candidates")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let rendered_guidance = guidance
        .iter()
        .enumerate()
        .filter(|(_, candidate)| {
            meaningful(candidate)
                && candidate.get("enabled").and_then(Value::as_bool) != Some(false)
        })
        .map(|(index, candidate)| render_guidance_candidate(index, candidate))
        .collect::<Vec<_>>();
    if !rendered_guidance.is_empty() {
        push_heading(&mut output, 3, "Enabled Guidance Candidates");
        output.push_str(&rendered_guidance.join("\n"));
        output.push('\n');
    }

    fallback(
        output,
        "No governance rules or enabled Guidance candidates were pinned.",
    )
}

fn render_rule(number: usize, rule: &Value) -> String {
    let mut output = String::new();
    let text = rule
        .as_object()
        .and_then(|object| meaningful_string(object.get("text")))
        .or_else(|| meaningful_string(Some(rule)))
        .unwrap_or("Rule");
    output.push_str(&format!("{number}. {}", indent_continuation(text, "   ")));
    output.push('\n');

    if let Some(object) = rule.as_object() {
        let details =
            render_object_fields(object, &["id", "text", "source", "created", "updated"], 4);
        if !details.is_empty() {
            output.push_str(details.trim_end());
            output.push('\n');
        }
    }
    output
}

fn render_guidance_candidate(index: usize, candidate: &Value) -> String {
    let Some(object) = candidate.as_object() else {
        return format!(
            "#### {index}. Guidance Candidate\n\n{}\n",
            scalar_text(candidate)
        );
    };
    let name = meaningful_string(object.get("name")).unwrap_or("Guidance Candidate");
    let mut output = String::new();
    push_heading(&mut output, 4, &format!("{index}. {name}"));
    if let Some(rule) = object.get("rule").filter(|value| meaningful(value)) {
        push_named_value(&mut output, "Applies When", rule, 5);
    }
    if let Some(instructions) = object.get("instructions").filter(|value| meaningful(value)) {
        push_named_value(&mut output, "Instructions", instructions, 5);
    }
    output.push_str(&render_object_fields(
        object,
        &["name", "enabled", "rule", "instructions"],
        5,
    ));
    output
}

fn render_previous_rounds(rounds: &[Value]) -> String {
    let rendered = rounds
        .iter()
        .enumerate()
        .filter_map(|(index, round)| {
            round
                .as_object()
                .map(|round| render_previous_round(round, index))
        })
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        "No previous Rounds were pinned for this implementation attempt.".to_string()
    } else {
        rendered.join("\n")
    }
}

fn render_previous_round(round: &Map<String, Value>, index: usize) -> String {
    let number = round
        .get("round")
        .and_then(Value::as_u64)
        .unwrap_or(index as u64 + 1);
    let mut output = String::new();
    push_heading(&mut output, 3, &format!("Round {number}"));
    for (key, label) in ROUND_METADATA_FIELDS {
        if let Some(value) = round.get(*key).filter(|value| meaningful(value)) {
            push_named_value(&mut output, label, value, 4);
        }
    }
    if let Some(request) = round.get("prompt").filter(|value| meaningful(value)) {
        push_named_value(&mut output, "Request", request, 4);
    }
    render_round_evidence(&mut output, round);
    output.push_str(&render_additional_round_fields(round));
    output
}

fn render_round_context(round: &Map<String, Value>) -> String {
    let mut output = String::new();
    if let Some(number) = round.get("round").filter(|value| meaningful(value)) {
        push_scalar_field(&mut output, "Round", number);
    }
    for (key, label) in ROUND_METADATA_FIELDS {
        if let Some(value) = round.get(*key).filter(|value| meaningful(value)) {
            push_named_value(&mut output, label, value, 4);
        }
    }
    render_round_evidence(&mut output, round);
    output.push_str(&render_additional_round_fields(round));
    output.trim().to_string()
}

fn render_round_evidence(output: &mut String, round: &Map<String, Value>) {
    for (key, label) in ROUND_EVIDENCE_FIELDS {
        if let Some(value) = round.get(*key).filter(|value| meaningful(value)) {
            push_named_value(output, label, value, 4);
        }
    }
}

fn render_additional_round_fields(round: &Map<String, Value>) -> String {
    let mut excluded = BTreeSet::from(["round", "prompt"]);
    excluded.extend(ROUND_METADATA_FIELDS.iter().map(|(key, _)| *key));
    excluded.extend(ROUND_EVIDENCE_FIELDS.iter().map(|(key, _)| *key));
    excluded.extend(ROUND_OMITTED_FIELDS.iter().copied());
    render_object_fields(round, &excluded.into_iter().collect::<Vec<_>>(), 4)
}

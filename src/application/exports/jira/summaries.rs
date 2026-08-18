use super::*;

pub(super) fn required_string<'a>(value: &'a Value, key: &str) -> RefineResult<&'a str> {
    nonempty_string(value, key).ok_or_else(|| {
        RefineError::Serialization(format!("Goal export requires a non-empty {key}"))
    })
}

pub(super) fn nonempty_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(super) fn string_or<'a>(value: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    nonempty_string(value, key).unwrap_or(fallback)
}

pub(super) fn push_bounded_optional_line(
    lines: &mut Vec<String>,
    label: &str,
    value: &Value,
    key: &str,
    value_limit: usize,
) {
    if let Some(value) = nonempty_string(value, key) {
        push_bounded_line(lines, label, value, value_limit);
    }
}

pub(super) fn push_bounded_line(
    lines: &mut Vec<String>,
    label: &str,
    value: &str,
    value_limit: usize,
) {
    lines.push(format!(
        "{label}: {}",
        truncate_with_marker(value, value_limit, label)
    ));
}

pub(super) fn push_quality_summary(lines: &mut Vec<String>, round: &Value) {
    push_bounded_optional_line(lines, "Quality state", round, "quality_state", 128);
    push_bounded_optional_line(lines, "Quality result", round, "quality_message", 768);
    push_bounded_optional_line(
        lines,
        "Quality checked at",
        round,
        "quality_checked_at",
        128,
    );

    let Some(details) = round
        .get("quality_details")
        .filter(|value| !value.is_null())
    else {
        return;
    };
    if let Some(detail) = compact_scalar(details) {
        push_unique_line(
            lines,
            format!(
                "Quality detail: {}",
                truncate_with_marker(&detail, 768, "Quality detail")
            ),
        );
        return;
    }
    for (key, label) in [
        ("evaluation_scope", "scope"),
        ("command", "command"),
        ("exit_code", "exit code"),
        ("cwd", "working directory"),
        ("source_candidate_commit", "source candidate"),
        ("candidate_commit", "evaluated commit"),
        ("operation_id", "operation"),
    ] {
        if let Some(value) = details.get(key).and_then(compact_scalar) {
            push_unique_line(
                lines,
                format!(
                    "Quality {label}: {}",
                    truncate_with_marker(&value, 768, &format!("Quality {label}"))
                ),
            );
        }
    }
    if let Some(results) = details.get("results").and_then(Value::as_array)
        && !results.is_empty()
    {
        let mut states = BTreeMap::<String, usize>::new();
        for result in results {
            let state = ["status", "state", "result"]
                .iter()
                .find_map(|key| result.get(key).and_then(compact_scalar))
                .unwrap_or_else(|| "recorded".to_string());
            *states.entry(state).or_default() += 1;
        }
        let counts = states
            .into_iter()
            .map(|(state, count)| format!("{state}={count}"))
            .collect::<Vec<_>>()
            .join(", ");
        push_unique_line(
            lines,
            format!("Quality checks: {} ({counts})", results.len()),
        );
    }
}

pub(super) fn push_implementation_plan_summary(lines: &mut Vec<String>, round: &Value) {
    let Some(plan) = round.get("implementation_plan").and_then(Value::as_object) else {
        return;
    };
    if let Some(phase) = plan.get("phase").and_then(compact_scalar) {
        push_unique_line(lines, format!("Implementation planning phase: {phase}"));
    }
    if let Some(state) = plan.get("state").and_then(compact_scalar) {
        push_unique_line(lines, format!("Implementation planning state: {state}"));
    }
    if let Some(summary) = plan
        .get("final_plan")
        .and_then(|value| value.get("result"))
        .and_then(|value| value.get("summary"))
        .and_then(compact_scalar)
    {
        push_unique_line(
            lines,
            format!(
                "Final implementation plan: {}",
                truncate_with_marker(&summary, 768, "Final implementation plan")
            ),
        );
    }
    if let Some(checklist) = plan
        .get("final_plan")
        .and_then(|value| value.get("result"))
        .and_then(|value| value.get("checklist"))
        .and_then(Value::as_array)
    {
        push_unique_line(
            lines,
            format!("Implementation plan checklist items: {}", checklist.len()),
        );
    }
    if let Some(failure) = plan
        .get("failure")
        .and_then(|value| value.get("message"))
        .and_then(compact_scalar)
    {
        push_unique_line(
            lines,
            format!(
                "Implementation planning failure: {}",
                truncate_with_marker(&failure, 768, "Implementation planning failure")
            ),
        );
    }
}

pub(super) fn push_governance_summary(lines: &mut Vec<String>, round: &Value) {
    let states = [
        ("rule_state", "rule"),
        ("product_state", "product"),
        ("constitution_state", "constitution"),
        ("meta_rule_state", "meta-rule"),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        nonempty_string(round, key).map(|value| {
            format!(
                "{label}={}",
                truncate_with_marker(value, 128, &format!("{label} governance state"))
            )
        })
    })
    .collect::<Vec<_>>();
    if !states.is_empty() {
        lines.push(format!("Governance states: {}", states.join(", ")));
    }
    push_bounded_optional_line(lines, "Governance result", round, "governance_message", 768);

    let details = round
        .get("governance_details")
        .filter(|value| !value.is_null());
    if let Some(detail) = details.and_then(compact_scalar) {
        push_unique_line(
            lines,
            format!(
                "Governance detail: {}",
                truncate_with_marker(&detail, 768, "Governance detail")
            ),
        );
    } else if let Some(details) = details {
        for (key, label) in [
            ("phase", "phase"),
            ("configured", "configured"),
            ("rules_checked", "rules checked"),
        ] {
            if let Some(value) = details.get(key).and_then(compact_scalar) {
                push_unique_line(
                    lines,
                    format!(
                        "Governance {label}: {}",
                        truncate_with_marker(&value, 256, &format!("Governance {label}"))
                    ),
                );
            }
        }
    }

    let explicit_actions = round
        .get("governance_rule_actions")
        .and_then(Value::as_array)
        .filter(|actions| !actions.is_empty());
    let actions = explicit_actions.or_else(|| {
        details.and_then(|details| {
            ["failed_actions", "violations", "rule_violations"]
                .iter()
                .find_map(|key| {
                    details
                        .get(key)
                        .and_then(Value::as_array)
                        .filter(|actions| !actions.is_empty())
                })
                .or_else(|| {
                    details.get("verdict").and_then(|verdict| {
                        ["failed_actions", "violations", "rule_violations"]
                            .iter()
                            .find_map(|key| {
                                verdict
                                    .get(key)
                                    .and_then(Value::as_array)
                                    .filter(|actions| !actions.is_empty())
                            })
                    })
                })
        })
    });
    let Some(actions) = actions else {
        return;
    };
    let mut seen = BTreeSet::new();
    for action in actions {
        let rendered = compact_governance_action(action);
        if !rendered.is_empty() && seen.insert(rendered.clone()) {
            lines.push(format!(
                "Governance action: {}",
                truncate_with_marker(&rendered, 1_200, "governance action")
            ));
        }
    }
}

fn compact_governance_action(action: &Value) -> String {
    if let Some(value) = compact_scalar(action) {
        return value;
    }
    [
        ("rule_id", "rule"),
        ("action", "action"),
        ("status", "status"),
        ("message", "message"),
        ("reason", "reason"),
        ("summary", "summary"),
        ("text", "text"),
        ("rule", "requirement"),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        action
            .get(key)
            .and_then(compact_scalar)
            .map(|value| format!("{label}={value}"))
    })
    .collect::<Vec<_>>()
    .join("; ")
}

fn compact_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        Value::Bool(_) | Value::Number(_) => Some(value.to_string()),
        _ => None,
    }
}

pub(super) fn push_unique_line(lines: &mut Vec<String>, line: String) {
    if !lines.iter().any(|existing| existing == &line) {
        lines.push(line);
    }
}

pub(super) fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

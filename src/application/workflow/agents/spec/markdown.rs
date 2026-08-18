use std::collections::BTreeSet;

use serde_json::{Map, Value};

pub(crate) fn render_object_fields(
    object: &Map<String, Value>,
    excluded: &[&str],
    heading_level: usize,
) -> String {
    let excluded = excluded.iter().copied().collect::<BTreeSet<_>>();
    let mut fields = object
        .iter()
        .filter(|(key, value)| !excluded.contains(key.as_str()) && meaningful(value))
        .collect::<Vec<_>>();
    fields.sort_by_key(|(key, _)| *key);

    let mut output = String::new();
    for (key, value) in fields {
        push_named_value(&mut output, &humanize(key), value, heading_level);
    }
    output
}

pub(crate) fn push_named_value(
    output: &mut String,
    label: &str,
    value: &Value,
    heading_level: usize,
) {
    match value {
        Value::String(text) if text.contains('\n') => {
            push_prose(
                output,
                &format!("{} {label}", "#".repeat(heading_level)),
                text,
            );
        }
        Value::String(_) | Value::Bool(_) | Value::Number(_) => {
            push_scalar_field(output, label, value);
        }
        Value::Array(values) => {
            let values = values
                .iter()
                .filter(|value| meaningful(value))
                .collect::<Vec<_>>();
            if values.is_empty() {
                return;
            }
            push_heading(output, heading_level, label);
            for (index, item) in values.into_iter().enumerate() {
                match item {
                    Value::Object(object) => {
                        push_heading(
                            output,
                            (heading_level + 1).min(6),
                            &format!("Item {}", index + 1),
                        );
                        output.push_str(&render_object_fields(
                            object,
                            &["created", "updated"],
                            (heading_level + 2).min(6),
                        ));
                    }
                    _ => {
                        output.push_str(&format!(
                            "{}. {}\n",
                            index + 1,
                            indent_continuation(&scalar_text(item), "   ")
                        ));
                    }
                }
            }
            output.push('\n');
        }
        Value::Object(object) => {
            let rendered = render_object_fields(object, &[], (heading_level + 1).min(6));
            if rendered.is_empty() {
                return;
            }
            push_heading(output, heading_level, label);
            output.push_str(rendered.trim_end());
            output.push('\n');
        }
        Value::Null => {}
    }
}

pub(crate) fn push_scalar_field(output: &mut String, label: &str, value: &Value) {
    output.push_str(&format!("- **{label}:** {}\n", scalar_text(value)));
}

pub(crate) fn push_prose(output: &mut String, heading: &str, prose: &str) {
    if !output.is_empty() && !output.ends_with("\n\n") {
        output.push('\n');
    }
    output.push_str(heading);
    output.push_str("\n\n");
    output.push_str(prose.trim());
    output.push_str("\n\n");
}

pub(crate) fn push_heading(output: &mut String, level: usize, label: &str) {
    if !output.is_empty() && !output.ends_with("\n\n") {
        output.push('\n');
    }
    output.push_str(&"#".repeat(level.clamp(1, 6)));
    output.push(' ');
    output.push_str(label);
    output.push_str("\n\n");
}

pub(crate) fn fallback(output: String, text: &str) -> String {
    let output = output.trim();
    if output.is_empty() {
        text.to_string()
    } else {
        output.to_string()
    }
}

pub(crate) fn meaningful_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty() && *text != "unclassified")
}

pub(crate) fn meaningful(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.trim().is_empty() && text.trim() != "unclassified",
        Value::Array(values) => values.iter().any(meaningful),
        Value::Object(values) => values.values().any(meaningful),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

pub(crate) fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Bool(true) => "Yes".to_string(),
        Value::Bool(false) => "No".to_string(),
        Value::Number(number) => number.to_string(),
        Value::Null | Value::Array(_) | Value::Object(_) => String::new(),
    }
}

fn humanize(key: &str) -> String {
    key.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            match characters.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), characters.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn indent_continuation(value: &str, indent: &str) -> String {
    value
        .trim()
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_string()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

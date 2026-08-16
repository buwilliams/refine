use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::process::supervisor::errors::{RefineError, RefineResult};

/// Maximum number of diagnostic follow-up invocations after an initial
/// structured response fails parsing or contract validation.
pub const DIAGNOSTIC_REPAIR_ATTEMPTS: usize = 2;

const MAX_STRUCTURED_OUTPUT_BYTES: usize = 1_048_576;
const MAX_JSON_DEPTH: usize = 32;
const MAX_TRANSPORT_LAYERS: usize = 4;

/// Decode one structured agent response through the shared transport boundary.
///
/// The boundary accepts a direct JSON value, one JSON value in a code fence or
/// mixed text, and completion envelopes or stringified values named by
/// `envelope_fields`. It deliberately rejects multiple distinct candidates and
/// bounds input size, JSON depth, and recursive envelope/string layers.
pub fn decode_bounded<T, F>(
    output: &str,
    label: &str,
    envelope_fields: &[&str],
    mut normalize: F,
) -> RefineResult<T>
where
    T: DeserializeOwned,
    F: FnMut(&mut Value) -> RefineResult<()>,
{
    let mut value = select_json_candidate(output, label)?;
    for layer in 0..=MAX_TRANSPORT_LAYERS {
        ensure_json_depth(&value, label)?;
        match value {
            Value::String(encoded) => {
                if layer == MAX_TRANSPORT_LAYERS {
                    return Err(contract_error(
                        label,
                        format!(
                            "exceeds the maximum of {MAX_TRANSPORT_LAYERS} completion-envelope or stringification layers"
                        ),
                    ));
                }
                value = serde_json::from_str(&encoded).map_err(|error| {
                    contract_error(
                        label,
                        format!("contains invalid recursively stringified JSON: {error}"),
                    )
                })?;
            }
            Value::Object(mut object) => {
                let present = envelope_fields
                    .iter()
                    .filter(|field| object.contains_key(**field))
                    .copied()
                    .collect::<Vec<_>>();
                if present.len() > 1 {
                    return Err(contract_error(
                        label,
                        format!(
                            "has ambiguous completion envelope fields: {}",
                            present.join(", ")
                        ),
                    ));
                }
                if let Some(field) = present.first() {
                    if layer == MAX_TRANSPORT_LAYERS {
                        return Err(contract_error(
                            label,
                            format!(
                                "exceeds the maximum of {MAX_TRANSPORT_LAYERS} completion-envelope or stringification layers"
                            ),
                        ));
                    }
                    value = object.remove(*field).expect("present envelope field");
                    continue;
                }
                value = Value::Object(object);
                break;
            }
            _ => break,
        }
    }

    ensure_json_depth(&value, label)?;
    normalize(&mut value)?;
    serde_path_to_error::deserialize(value).map_err(|error| {
        let path = error.path().to_string();
        let location = if path.is_empty() {
            "the root value".to_string()
        } else {
            format!("field `{path}`")
        };
        contract_error(
            label,
            format!(
                "does not match the required schema at {location}: {}",
                error.inner()
            ),
        )
    })
}

fn select_json_candidate(output: &str, label: &str) -> RefineResult<Value> {
    if output.len() > MAX_STRUCTURED_OUTPUT_BYTES {
        return Err(contract_error(
            label,
            format!(
                "exceeds the maximum payload size of {MAX_STRUCTURED_OUTPUT_BYTES} bytes (observed {})",
                output.len()
            ),
        ));
    }
    let output = output.trim();
    if output.is_empty() {
        return Err(contract_error(label, "is empty"));
    }
    let full_output_error = match serde_json::from_str(output) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };

    let fenced = fenced_contents(output);
    let spans = balanced_json_spans(output);
    let likely_suffix = likely_json_suffix(output);
    let mut candidates = Vec::new();
    for candidate in fenced.iter().chain(spans.iter()) {
        if let Ok(value) = serde_json::from_str::<Value>(candidate.trim())
            && !candidates.contains(&value)
        {
            candidates.push(value);
        }
    }
    if candidates.is_empty() {
        for candidate in stringified_json_spans(output) {
            if let Ok(value) = serde_json::from_str::<Value>(candidate)
                && !candidates.contains(&value)
            {
                candidates.push(value);
            }
        }
    }
    match candidates.len() {
        1 => return Ok(candidates.remove(0)),
        count if count > 1 => {
            return Err(contract_error(
                label,
                format!("contains {count} distinct JSON candidates; exactly one is required"),
            ));
        }
        _ => {}
    }

    let likely = fenced
        .first()
        .copied()
        .or(likely_suffix)
        .unwrap_or(output)
        .trim();
    let error = match serde_json::from_str::<Value>(likely) {
        Err(error) => error,
        Ok(_) => full_output_error,
    };
    Err(contract_error(
        label,
        format!("contains invalid JSON: {error}"),
    ))
}

fn fenced_contents(output: &str) -> Vec<&str> {
    let mut contents = Vec::new();
    let mut offset = 0;
    while let Some(relative_open) = output[offset..].find("```") {
        let open = offset + relative_open + 3;
        let Some(header_end_relative) = output[open..].find('\n') else {
            break;
        };
        let content_start = open + header_end_relative + 1;
        let Some(close_relative) = output[content_start..].find("```") else {
            break;
        };
        let close = content_start + close_relative;
        contents.push(&output[content_start..close]);
        offset = close + 3;
    }
    contents
}

fn balanced_json_spans(output: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut start = None;
    let mut closers = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in output.char_indices() {
        if start.is_none() {
            match ch {
                '{' => {
                    start = Some(index);
                    closers.push('}');
                }
                '[' => {
                    start = Some(index);
                    closers.push(']');
                }
                _ => {}
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => closers.push('}'),
            '[' => closers.push(']'),
            '}' | ']' if closers.last() == Some(&ch) => {
                closers.pop();
                if closers.is_empty() {
                    let span_start = start.take().expect("started JSON span");
                    spans.push(&output[span_start..index + ch.len_utf8()]);
                }
            }
            '}' | ']' => {
                start = None;
                closers.clear();
                in_string = false;
                escaped = false;
            }
            _ => {}
        }
    }
    spans
}

fn stringified_json_spans(output: &str) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(relative_start) = output[offset..].find('"') {
        let start = offset + relative_start;
        let mut escaped = false;
        let mut end = None;
        for (relative_index, ch) in output[start + 1..].char_indices() {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                end = Some(start + 1 + relative_index + ch.len_utf8());
                break;
            }
        }
        let Some(end) = end else {
            break;
        };
        let candidate = &output[start..end];
        if let Ok(Value::String(encoded)) = serde_json::from_str(candidate)
            && serde_json::from_str::<Value>(&encoded).is_ok()
        {
            spans.push(candidate);
        }
        offset = end;
    }
    spans
}

fn likely_json_suffix(output: &str) -> Option<&str> {
    output
        .char_indices()
        .find(|(_, ch)| matches!(ch, '{' | '[' | '"'))
        .map(|(index, _)| &output[index..])
}

fn ensure_json_depth(value: &Value, label: &str) -> RefineResult<()> {
    fn depth(value: &Value, current: usize) -> usize {
        match value {
            Value::Array(values) => values
                .iter()
                .map(|value| depth(value, current + 1))
                .max()
                .unwrap_or(current + 1),
            Value::Object(values) => values
                .values()
                .map(|value| depth(value, current + 1))
                .max()
                .unwrap_or(current + 1),
            _ => current,
        }
    }

    let observed = depth(value, 0);
    if observed > MAX_JSON_DEPTH {
        return Err(contract_error(
            label,
            format!(
                "exceeds the maximum JSON nesting depth of {MAX_JSON_DEPTH} (observed {observed})"
            ),
        ));
    }
    Ok(())
}

fn contract_error(label: &str, detail: impl Into<String>) -> RefineError {
    RefineError::Serialization(format!(
        "agent returned invalid structured {label}: {}",
        detail.into()
    ))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct Fixture {
        name: String,
        count: usize,
    }

    fn decode(output: &str) -> RefineResult<Fixture> {
        decode_bounded(
            output,
            "fixture JSON",
            &["planning_result", "result"],
            |_| Ok(()),
        )
    }

    #[test]
    fn decodes_direct_fenced_mixed_wrapped_and_stringified_values() {
        let expected = Fixture {
            name: "ready".to_string(),
            count: 2,
        };
        for output in [
            r#"{"name":"ready","count":2}"#,
            "```json\n{\"name\":\"ready\",\"count\":2}\n```",
            "The result follows: {\"name\":\"ready\",\"count\":2} done.",
            r#"{"state":"completed","planning_result":{"name":"ready","count":2}}"#,
            r#"{"planning_result":"{\"name\":\"ready\",\"count\":2}"}"#,
            r#""{\"planning_result\":\"{\\\"name\\\":\\\"ready\\\",\\\"count\\\":2}\"}""#,
            r#"The stringified result follows: "{\"name\":\"ready\",\"count\":2}" done."#,
        ] {
            assert_eq!(decode(output).unwrap(), expected, "output: {output}");
        }
    }

    #[test]
    fn rejects_ambiguous_candidates_and_envelopes() {
        let candidates = "first {\"name\":\"one\",\"count\":1} then {\"name\":\"two\",\"count\":2}";
        assert!(
            decode(candidates)
                .unwrap_err()
                .to_string()
                .contains("2 distinct JSON candidates")
        );

        let envelopes =
            r#"{"planning_result":{"name":"one","count":1},"result":{"name":"two","count":2}}"#;
        assert!(
            decode(envelopes)
                .unwrap_err()
                .to_string()
                .contains("ambiguous completion envelope fields")
        );

        let stringified =
            r#"first "{\"name\":\"one\",\"count\":1}" then "{\"name\":\"two\",\"count\":2}""#;
        assert!(
            decode(stringified)
                .unwrap_err()
                .to_string()
                .contains("2 distinct JSON candidates")
        );
    }

    #[test]
    fn bounds_payload_depth_and_recursive_transport_layers() {
        let oversized = "x".repeat(MAX_STRUCTURED_OUTPUT_BYTES + 1);
        assert!(
            decode(&oversized)
                .unwrap_err()
                .to_string()
                .contains("maximum payload size")
        );

        let over_nested = format!(
            "{}0{}",
            "[".repeat(MAX_JSON_DEPTH + 1),
            "]".repeat(MAX_JSON_DEPTH + 1)
        );
        assert!(
            decode(&over_nested)
                .unwrap_err()
                .to_string()
                .contains("maximum JSON nesting depth")
        );

        let mut stringified = r#"{"name":"ready","count":2}"#.to_string();
        for _ in 0..=MAX_TRANSPORT_LAYERS {
            stringified = serde_json::to_string(&stringified).unwrap();
        }
        assert!(
            decode(&stringified)
                .unwrap_err()
                .to_string()
                .contains("completion-envelope or stringification layers")
        );
    }

    #[test]
    fn preserves_json_and_path_aware_schema_diagnostics() {
        let malformed = decode("```json\n{\"name\":\"ready\",\"count\":}\n```")
            .unwrap_err()
            .to_string();
        assert!(malformed.contains("invalid JSON"));
        assert!(malformed.contains("line 1 column"));

        let wrong_type = decode(r#"{"name":"ready","count":"two"}"#)
            .unwrap_err()
            .to_string();
        assert!(wrong_type.contains("field `count`"));
        assert!(wrong_type.contains("invalid type"));
    }
}

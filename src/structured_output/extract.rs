use serde::de::DeserializeOwned;
use serde_json::Value;

use super::error::StructuredOutputError;

const MAX_STRUCTURED_OUTPUT_BYTES: usize = 1_048_576;
const MAX_JSON_DEPTH: usize = 32;
const MAX_TRANSPORT_LAYERS: usize = 4;

/// How to choose one JSON value when an output contains several.
#[derive(Clone, Copy)]
pub enum Selection<'a> {
    /// Exactly one distinct JSON candidate; ambiguity is a transport error.
    /// This is the default and the right posture for repair-guarded contracts.
    Single,
    /// The last balanced candidate satisfying the predicate. For lenient
    /// no-retry sites (Governance verdicts, Quality recovery) whose agents
    /// legitimately quote JSON while reasoning before the final value.
    LastMatching(&'a dyn Fn(&Value) -> bool),
}

/// Bounds and behavior for one structured decode through the shared transport
/// boundary. Every agent-output parse site uses these options; the bounds are
/// defensive transport limits, not product rules.
#[derive(Clone, Copy)]
pub struct DecodeOptions<'a> {
    pub label: &'a str,
    pub envelope_fields: &'a [&'a str],
    pub selection: Selection<'a>,
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_layers: usize,
}

impl<'a> DecodeOptions<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            envelope_fields: &[],
            selection: Selection::Single,
            max_bytes: MAX_STRUCTURED_OUTPUT_BYTES,
            max_depth: MAX_JSON_DEPTH,
            max_layers: MAX_TRANSPORT_LAYERS,
        }
    }

    pub fn with_envelopes(label: &'a str, envelope_fields: &'a [&'a str]) -> Self {
        Self {
            envelope_fields,
            ..Self::new(label)
        }
    }

    pub fn selecting(mut self, selection: Selection<'a>) -> Self {
        self.selection = selection;
        self
    }
}

/// Decode one structured agent response through the shared transport boundary.
///
/// The boundary accepts a direct JSON value, one JSON value in a code fence or
/// mixed text, and completion envelopes or stringified values named by
/// `options.envelope_fields`. It bounds input size, JSON depth, and recursive
/// envelope/string layers, applies `normalize` to the selected value, and
/// deserializes with path-aware schema diagnostics.
pub fn decode_structured<T, F>(
    output: &str,
    options: &DecodeOptions<'_>,
    normalize: F,
) -> Result<T, StructuredOutputError>
where
    T: DeserializeOwned,
    F: FnOnce(&mut Value) -> Result<(), StructuredOutputError>,
{
    let mut value = select_value(output, options)?;
    normalize(&mut value)?;
    serde_path_to_error::deserialize(value).map_err(|error| {
        let path = error.path().to_string();
        StructuredOutputError::schema(
            options.label,
            (!path.is_empty()).then_some(path),
            error.inner().to_string(),
        )
    })
}

/// Select one bounded JSON value from an agent output without deserializing
/// it, for sites that keep lenient `Value`-level derivation.
pub fn select_value(
    output: &str,
    options: &DecodeOptions<'_>,
) -> Result<Value, StructuredOutputError> {
    let label = options.label;
    let mut value = select_json_candidate(output, options)?;
    for layer in 0..=options.max_layers {
        ensure_json_depth(&value, options)?;
        match value {
            Value::String(encoded) => {
                if layer == options.max_layers {
                    return Err(StructuredOutputError::transport(
                        label,
                        format!(
                            "exceeds the maximum of {} completion-envelope or stringification layers",
                            options.max_layers
                        ),
                    ));
                }
                value = serde_json::from_str(&encoded).map_err(|error| {
                    StructuredOutputError::transport(
                        label,
                        format!("contains invalid recursively stringified JSON: {error}"),
                    )
                })?;
            }
            Value::Object(mut object) => {
                let present = options
                    .envelope_fields
                    .iter()
                    .filter(|field| object.contains_key(**field))
                    .copied()
                    .collect::<Vec<_>>();
                if present.len() > 1 {
                    return Err(StructuredOutputError::transport(
                        label,
                        format!(
                            "has ambiguous completion envelope fields: {}",
                            present.join(", ")
                        ),
                    ));
                }
                if let Some(field) = present.first() {
                    if layer == options.max_layers {
                        return Err(StructuredOutputError::transport(
                            label,
                            format!(
                                "exceeds the maximum of {} completion-envelope or stringification layers",
                                options.max_layers
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
    ensure_json_depth(&value, options)?;
    Ok(value)
}

fn select_json_candidate(
    output: &str,
    options: &DecodeOptions<'_>,
) -> Result<Value, StructuredOutputError> {
    let label = options.label;
    if output.len() > options.max_bytes {
        return Err(StructuredOutputError::transport(
            label,
            format!(
                "exceeds the maximum payload size of {} bytes (observed {})",
                options.max_bytes,
                output.len()
            ),
        ));
    }
    let output = output.trim();
    if output.is_empty() {
        return Err(StructuredOutputError::transport(label, "is empty"));
    }
    let full_output_error = match serde_json::from_str::<Value>(output) {
        Ok(value) => match options.selection {
            Selection::Single => return Ok(value),
            Selection::LastMatching(matches) if matches(&value) => return Ok(value),
            Selection::LastMatching(_) => {
                return Err(no_matching_candidate(label));
            }
        },
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
    if let Selection::LastMatching(matches) = options.selection {
        return match candidates.into_iter().filter(|value| matches(value)).next_back() {
            Some(value) => Ok(value),
            None => Err(no_matching_candidate(label)),
        };
    }
    match candidates.len() {
        1 => return Ok(candidates.remove(0)),
        count if count > 1 => {
            return Err(StructuredOutputError::transport(
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
    Err(StructuredOutputError::transport(
        label,
        format!("contains invalid JSON: {error}"),
    ))
}

fn no_matching_candidate(label: &str) -> StructuredOutputError {
    StructuredOutputError::transport(label, "contains no JSON value matching the required shape")
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

fn ensure_json_depth(
    value: &Value,
    options: &DecodeOptions<'_>,
) -> Result<(), StructuredOutputError> {
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
    if observed > options.max_depth {
        return Err(StructuredOutputError::transport(
            options.label,
            format!(
                "exceeds the maximum JSON nesting depth of {} (observed {observed})",
                options.max_depth
            ),
        ));
    }
    Ok(())
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

    fn decode(output: &str) -> Result<Fixture, StructuredOutputError> {
        decode_structured(
            output,
            &DecodeOptions::with_envelopes("fixture JSON", &["planning_result", "result"]),
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
        let options = DecodeOptions::with_envelopes("fixture JSON", &["planning_result", "result"]);
        let oversized = "x".repeat(options.max_bytes + 1);
        assert!(
            decode(&oversized)
                .unwrap_err()
                .to_string()
                .contains("maximum payload size")
        );

        let over_nested = format!(
            "{}0{}",
            "[".repeat(options.max_depth + 1),
            "]".repeat(options.max_depth + 1)
        );
        assert!(
            decode(&over_nested)
                .unwrap_err()
                .to_string()
                .contains("maximum JSON nesting depth")
        );

        let mut stringified = r#"{"name":"ready","count":2}"#.to_string();
        for _ in 0..=options.max_layers {
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

        let wrong_type = decode(r#"{"name":"ready","count":"two"}"#).unwrap_err();
        assert_eq!(
            wrong_type.kind(),
            super::super::error::StructuredOutputErrorKind::Schema
        );
        let wrong_type = wrong_type.to_string();
        assert!(wrong_type.contains("field `count`"));
        assert!(wrong_type.contains("invalid type"));
    }

    #[test]
    fn last_matching_selection_picks_the_final_shaped_value_from_prose() {
        let matches: &dyn Fn(&Value) -> bool =
            &|value| value.get("name").is_some() && value.get("count").is_some();
        let options =
            DecodeOptions::new("fixture JSON").selecting(Selection::LastMatching(matches));

        let reasoning = "Considering {\"scratch\":true} first, {\"name\":\"draft\",\"count\":1} \
                         then the verdict {\"name\":\"final\",\"count\":2} concludes.";
        let selected = select_value(reasoning, &options).unwrap();
        assert_eq!(selected["name"], "final");
        assert_eq!(selected["count"], 2);

        let direct = r#"{"name":"only","count":3}"#;
        assert_eq!(select_value(direct, &options).unwrap()["name"], "only");

        let unmatched = "prose with {\"scratch\":true} only";
        assert!(
            select_value(unmatched, &options)
                .unwrap_err()
                .to_string()
                .contains("no JSON value matching the required shape")
        );
    }
}

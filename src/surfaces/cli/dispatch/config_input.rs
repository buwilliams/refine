use super::*;

pub(super) fn decode_config_input(
    payload: ConfigPayload,
    flags: serde_json::Map<String, Value>,
    label: &str,
) -> RefineResult<Value> {
    decode_config_input_with_reader(payload, flags, label, &mut std::io::stdin().lock())
}

fn decode_config_input_with_reader(
    payload: ConfigPayload,
    flags: serde_json::Map<String, Value>,
    label: &str,
    stdin: &mut impl Read,
) -> RefineResult<Value> {
    let source_count = usize::from(payload.json.is_some())
        + usize::from(payload.file.is_some())
        + usize::from(payload.stdin);
    if source_count > 1 {
        return Err(RefineError::InvalidInput(
            "use exactly one of --json, --file, or --stdin".to_string(),
        ));
    }
    if source_count > 0 && !flags.is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "{label} flags cannot be combined with --json, --file, or --stdin"
        )));
    }
    let value = if let Some(text) = payload.json {
        parse_object(&text, "inline JSON")?
    } else if let Some(path) = payload.file {
        let text = fs::read_to_string(&path).map_err(|error| {
            RefineError::InvalidInput(format!(
                "failed to read configuration file {}: {error}",
                path.display()
            ))
        })?;
        parse_object(&text, &format!("configuration file {}", path.display()))?
    } else if payload.stdin {
        let mut text = String::new();
        stdin.read_to_string(&mut text).map_err(|error| {
            RefineError::InvalidInput(format!("failed to read configuration stdin: {error}"))
        })?;
        parse_object(&text, "configuration stdin")?
    } else {
        Value::Object(flags)
    };
    if value.as_object().is_none_or(serde_json::Map::is_empty) {
        return Err(RefineError::InvalidInput(format!(
            "{label} requires at least one value"
        )));
    }
    Ok(value)
}

fn parse_object(text: &str, source: &str) -> RefineResult<Value> {
    let value = serde_json::from_str::<Value>(text)
        .map_err(|error| RefineError::InvalidInput(format!("malformed {source}: {error}")))?;
    if !value.is_object() {
        return Err(RefineError::InvalidInput(format!(
            "{source} must contain a JSON object"
        )));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn input_decoder_handles_file_and_stdin_and_reports_malformed_sources() {
        let path =
            std::env::temp_dir().join(format!("refine-config-input-{}.json", std::process::id()));
        fs::write(&path, "{not-json").unwrap();
        let malformed_file = decode_config_input_with_reader(
            ConfigPayload {
                json: None,
                file: Some(path.clone()),
                stdin: false,
            },
            serde_json::Map::new(),
            "Quality patch",
            &mut Cursor::new(Vec::<u8>::new()),
        );
        assert!(matches!(malformed_file, Err(RefineError::InvalidInput(_))));
        fs::remove_file(&path).unwrap();

        let malformed_stdin = decode_config_input_with_reader(
            ConfigPayload {
                json: None,
                file: None,
                stdin: true,
            },
            serde_json::Map::new(),
            "Quality patch",
            &mut Cursor::new(b"[not-an-object]"),
        );
        assert!(matches!(malformed_stdin, Err(RefineError::InvalidInput(_))));

        let decoded = decode_config_input_with_reader(
            ConfigPayload {
                json: None,
                file: None,
                stdin: true,
            },
            serde_json::Map::new(),
            "Quality patch",
            &mut Cursor::new(br#"{"instructions":"line one\nline two"}"#),
        )
        .unwrap();
        assert_eq!(decoded["instructions"], "line one\nline two");

        let missing_file = decode_config_input_with_reader(
            ConfigPayload {
                json: None,
                file: Some(path),
                stdin: false,
            },
            serde_json::Map::new(),
            "Quality patch",
            &mut Cursor::new(Vec::<u8>::new()),
        );
        assert!(matches!(missing_file, Err(RefineError::InvalidInput(_))));
    }
}

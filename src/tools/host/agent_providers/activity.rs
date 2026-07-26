use super::*;

pub(super) struct ProviderActivityFormatter {
    output_format: String,
    stdout_buffer: String,
    stderr_buffer: String,
}

impl ProviderActivityFormatter {
    pub(super) fn new(output_format: &str) -> Self {
        Self {
            output_format: output_format.to_string(),
            stdout_buffer: String::new(),
            stderr_buffer: String::new(),
        }
    }

    pub(super) fn push(&mut self, stream: ManagedProcessOutputStream, bytes: &[u8]) -> Vec<String> {
        let chunk = String::from_utf8_lossy(bytes);
        let buffer = match stream {
            ManagedProcessOutputStream::Stdout => &mut self.stdout_buffer,
            ManagedProcessOutputStream::Stderr => &mut self.stderr_buffer,
        };
        buffer.push_str(&chunk);
        let mut lines = Vec::new();
        while let Some(index) = buffer.find('\n') {
            let mut line = buffer.drain(..=index).collect::<String>();
            line.truncate(line.trim_end_matches(['\r', '\n']).len());
            if let Some(activity) = provider_activity_line(stream, &line, &self.output_format) {
                lines.push(activity);
            }
        }
        lines
    }

    pub(super) fn finish(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        let stdout = std::mem::take(&mut self.stdout_buffer);
        if let Some(activity) = provider_activity_line(
            ManagedProcessOutputStream::Stdout,
            &stdout,
            &self.output_format,
        ) {
            lines.push(activity);
        }
        let stderr = std::mem::take(&mut self.stderr_buffer);
        if let Some(activity) = provider_activity_line(
            ManagedProcessOutputStream::Stderr,
            &stderr,
            &self.output_format,
        ) {
            lines.push(activity);
        }
        lines
    }
}

fn provider_activity_line(
    stream: ManagedProcessOutputStream,
    line: &str,
    output_format: &str,
) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if stream == ManagedProcessOutputStream::Stderr {
        return Some(format!(
            "stderr: {}",
            line.chars().take(1000).collect::<String>()
        ));
    }
    if output_format == "plain" {
        return Some(line.chars().take(1000).collect());
    }
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        return None;
    };
    provider_activity_text_from_json(&event).map(|text| text.chars().take(1000).collect())
}

fn provider_activity_text_from_json(event: &serde_json::Value) -> Option<String> {
    let object = event.as_object()?;
    if let Some(item) = object.get("item").and_then(|value| value.as_object()) {
        let item_type = item.get("type").and_then(|value| value.as_str());
        let text = item
            .get("text")
            .or_else(|| item.get("content"))
            .or_else(|| object.get("text"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if matches!(item_type, Some("agent_message" | "assistant_message")) {
            return text.map(str::to_string);
        }
        if let (Some(item_type), Some(text)) = (item_type, text) {
            return Some(format!("{item_type}: {text}"));
        }
    }
    if object.get("type").and_then(|value| value.as_str()) == Some("assistant.message") {
        return object
            .get("data")
            .and_then(|value| value.get("content"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    if object.get("type").and_then(|value| value.as_str()) == Some("assistant.message_delta") {
        return object
            .get("data")
            .and_then(|value| value.get("deltaContent"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    if object.get("type").and_then(|value| value.as_str()) == Some("assistant") {
        return object
            .get("message")
            .and_then(|value| value.get("content"))
            .and_then(text_from_content)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    for key in ["delta", "text", "message", "result"] {
        if let Some(text) = object
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(text.to_string());
        }
    }
    None
}

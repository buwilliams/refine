use crate::model::JsonObject;

/// A positive stored value, or the fallback when the key is absent, empty, or
/// non-positive. Absent and empty are how an operator hands a limit back to the
/// host-capacity governor.
pub(super) fn setting_usize(settings: &JsonObject, key: &str, fallback: usize) -> usize {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(super) fn setting_cap_with_default_values(
    settings: &JsonObject,
    key: &str,
    fallback: usize,
    default_values: &[usize],
) -> usize {
    let Some(value) = settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
    else {
        return fallback;
    };
    if fallback > value && default_values.contains(&value) {
        fallback
    } else {
        value
    }
}

/// The no-output stall budget for unattended non-interactive agent runs.
///
/// Backed by `agent_idle_timeout_seconds` so operators tune one idle knob for
/// every verdict-producing invocation (governance, quality evaluation, quality
/// recovery). PTY Goal Agent sessions keep the separate wall-clock hard cap.
pub(super) fn agent_stall_timeout_seconds(settings: &JsonObject) -> u64 {
    setting_usize(settings, "agent_idle_timeout_seconds", 900) as u64
}

pub(super) fn setting_string(settings: &JsonObject, key: &str, fallback: &str) -> String {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

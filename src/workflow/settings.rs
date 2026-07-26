use crate::model::JsonObject;

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

pub(super) fn setting_string(settings: &JsonObject, key: &str, fallback: &str) -> String {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

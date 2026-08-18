use crate::model::JsonObject;

/// A positive stored value, or the fallback when the key is absent, empty, or
/// non-positive. Absent and empty are how an operator hands a limit back to the
/// host-capacity governor.
pub(crate) fn setting_usize(settings: &JsonObject, key: &str, fallback: usize) -> usize {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn automatic_resource_budget_percent(settings: &JsonObject) -> usize {
    setting_usize(
        settings,
        "automatic_agent_resource_budget_percent",
        crate::infrastructure::process::supervisor::config::AUTOMATIC_AGENT_RESOURCE_BUDGET_PERCENT_DEFAULT,
    )
}

/// The Goal Agent stall budget from `agent_idle_timeout_seconds`. Distinct
/// from `agent_hard_cap_seconds`: the idle budget resets on every sign of
/// agent activity, so a hung session fails in minutes while a slow-but-working
/// one runs up to the hard cap. Non-interactive verdict invocations
/// (governance, quality evaluation, quality recovery) derive their supervised
/// no-output stall budget from the same knob.
pub(crate) fn agent_idle_timeout(settings: &JsonObject) -> Option<std::time::Duration> {
    Some(std::time::Duration::from_secs(
        setting_usize(settings, "agent_idle_timeout_seconds", 900) as u64,
    ))
}

pub(crate) fn setting_cap_with_default_values(
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

pub(crate) fn setting_string(settings: &JsonObject, key: &str, fallback: &str) -> String {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_parallel_run_cap_remains_authoritative() {
        let settings = serde_json::json!({
            "parallel_run_cap": "2",
            "automatic_agent_resource_budget_percent": "100"
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(setting_usize(&settings, "parallel_run_cap", 1), 2);
    }

    #[test]
    fn automatic_resource_budget_defaults_to_seventy_for_missing_legacy_state() {
        let settings = JsonObject::new();
        assert_eq!(automatic_resource_budget_percent(&settings), 70);
        let explicit = serde_json::json!({"automatic_agent_resource_budget_percent": "45"})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(automatic_resource_budget_percent(&explicit), 45);
    }

    #[test]
    fn unset_subordinate_caps_inherit_the_governed_global_limit() {
        let settings = JsonObject::new();
        let global_limit = setting_usize(&settings, "parallel_run_cap", 3);
        assert_eq!(global_limit, 3);
        assert_eq!(
            setting_cap_with_default_values(
                &settings,
                "parallel_per_node_cap",
                global_limit,
                &[1, 2]
            ),
            3
        );
        assert_eq!(
            setting_cap_with_default_values(
                &settings,
                "parallel_per_provider_cap",
                global_limit,
                &[2]
            ),
            3
        );
        assert_eq!(
            setting_cap_with_default_values(
                &settings,
                "parallel_per_target_app_cap",
                global_limit,
                &[2]
            ),
            3
        );
    }
}

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::JsonObject;

pub const CURRENT_FLEET_SCHEMA_VERSION: u64 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FleetConfig {
    #[serde(default = "default_fleet_schema_version")]
    pub schema_version: u64,
    #[serde(default = "default_fleet_provider_name")]
    pub default_provider: String,
    #[serde(default)]
    pub providers: BTreeMap<String, FleetProviderConfig>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct FleetProviderConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    pub binary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_env: Vec<String>,
    #[serde(default)]
    pub require_credentials: bool,
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub defaults: JsonObject,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provision: Vec<FleetCommandStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deprovision: Vec<FleetCommandStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<FleetCommandStep>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FleetCommandStep {
    pub argv: Vec<String>,
    #[serde(default)]
    pub allow_failure: bool,
}

impl<'de> Deserialize<'de> for FleetCommandStep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StepRepr {
            Argv(Vec<String>),
            Detailed {
                argv: Vec<String>,
                #[serde(default)]
                allow_failure: bool,
            },
        }
        Ok(match StepRepr::deserialize(deserializer)? {
            StepRepr::Argv(argv) => FleetCommandStep {
                argv,
                allow_failure: false,
            },
            StepRepr::Detailed {
                argv,
                allow_failure,
            } => FleetCommandStep {
                argv,
                allow_failure,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FleetOperation {
    Provision,
    Deprovision,
    Status,
}

impl FleetOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            FleetOperation::Provision => "provision",
            FleetOperation::Deprovision => "deprovision",
            FleetOperation::Status => "status",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FleetStepResult {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
    pub allow_failure: bool,
    pub executed: bool,
}

/// Renders `{placeholder}` tokens inside each argv element. Every token must
/// resolve; unknown tokens are an error so a config typo fails loudly instead
/// of provisioning with a literal `{region}` argument.
pub fn render_argv(
    argv: &[String],
    values: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    argv.iter()
        .map(|part| render_template(part, values))
        .collect()
}

pub fn render_template(
    template: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, String> {
    let mut rendered = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        rendered.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            return Err(format!("unterminated placeholder in `{template}`"));
        };
        let name = &rest[open + 1..open + close];
        if name.is_empty() || !valid_placeholder_name(name) {
            return Err(format!("invalid placeholder `{{{name}}}` in `{template}`"));
        }
        let Some(value) = values.get(name) else {
            return Err(format!(
                "unresolved placeholder `{{{name}}}` in `{template}`"
            ));
        };
        rendered.push_str(value);
        rest = &rest[open + close + 1..];
    }
    rendered.push_str(rest);
    Ok(rendered)
}

fn valid_placeholder_name(name: &str) -> bool {
    name.chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

pub fn valid_provider_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

fn default_fleet_schema_version() -> u64 {
    1
}

fn default_fleet_provider_name() -> String {
    "fly".to_string()
}

impl Default for FleetConfig {
    fn default() -> Self {
        Self {
            schema_version: default_fleet_schema_version(),
            default_provider: default_fleet_provider_name(),
            providers: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn render_template_substitutes_placeholders() {
        let rendered = render_template(
            "refine-{node_id}-{region}",
            &values(&[("node_id", "worker-1"), ("region", "iad")]),
        )
        .unwrap();
        assert_eq!(rendered, "refine-worker-1-iad");
    }

    #[test]
    fn render_template_rejects_unresolved_placeholder() {
        let error = render_template("{missing}", &values(&[])).unwrap_err();
        assert!(error.contains("unresolved placeholder `{missing}`"));
    }

    #[test]
    fn render_template_rejects_unterminated_placeholder() {
        let error = render_template("{node_id", &values(&[("node_id", "a")])).unwrap_err();
        assert!(error.contains("unterminated"));
    }

    #[test]
    fn fleet_command_step_accepts_bare_argv_and_detailed_form() {
        let steps: Vec<FleetCommandStep> = serde_json::from_value(serde_json::json!([
            ["fly", "status"],
            {"argv": ["fly", "apps", "create"], "allow_failure": true}
        ]))
        .unwrap();
        assert_eq!(steps[0].argv, vec!["fly", "status"]);
        assert!(!steps[0].allow_failure);
        assert!(steps[1].allow_failure);
    }

    #[test]
    fn fleet_config_ignores_unknown_fields_for_forward_compatibility() {
        let config: FleetConfig = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "future_field": {"anything": true},
            "providers": {
                "fly": {"binary": "fly", "future_provider_field": 1}
            }
        }))
        .unwrap();
        assert_eq!(config.schema_version, 1);
        assert!(config.providers.contains_key("fly"));
    }

    #[test]
    fn provider_name_validation() {
        assert!(valid_provider_name("fly"));
        assert!(valid_provider_name("aws-ec2"));
        assert!(!valid_provider_name(""));
        assert!(!valid_provider_name("Fly"));
        assert!(!valid_provider_name("1fly"));
    }
}

//! Events select reusable Skill instructions; execution and storage live elsewhere.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::workflow::GoalStatus;

pub const SCHEMA_VERSION: u32 = 2;
pub const MAX_CONFIG_BYTES: usize = 16 * 1024 * 1024;
pub const CUSTOM_EVENT_ID: &str = "custom";
pub const WORKFLOW_STEPS: [&str; 10] = [
    "backlog",
    "todo",
    "plan",
    "implement",
    "quality",
    "governance",
    "review",
    "done",
    "failed",
    "cancelled",
];

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    /// None means project-wide. Node scope is synchronized configuration, not local state.
    #[serde(default)]
    pub node_id: Option<String>,
}

impl Scope {
    pub fn applies(&self, node: &str) -> bool {
        self.node_id
            .as_ref()
            .is_none_or(|id| id.eq_ignore_ascii_case(node.trim()))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    #[default]
    Text,
    Number,
    Boolean,
    Choice,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kind: ParameterType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub choices: Vec<String>,
}

impl Parameter {
    pub fn accepts(&self, value: &Value) -> bool {
        match self.kind {
            ParameterType::Text => value.is_string(),
            ParameterType::Number => value.is_number(),
            ParameterType::Boolean => value.is_boolean(),
            ParameterType::Choice => value
                .as_str()
                .is_some_and(|v| self.choices.iter().any(|c| c == v)),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub prompt: String,
    #[serde(default = "task_role")]
    pub role: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub provenance: Option<String>,
}

fn task_role() -> String {
    "task".into()
}
fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingMode {
    #[default]
    Blocking,
    Background,
    Context,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub id: String,
    pub skill_id: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: BindingMode,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub scope: Scope,
    /// Explicit project binding ID replaced (or disabled) on this node.
    #[serde(default)]
    pub overrides: Option<String>,
    /// Parameter name -> goal.*, system.*, or event.* field path. No expressions.
    #[serde(default)]
    pub inputs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    #[default]
    Custom,
    System,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventDefinition {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: EventKind,
    /// Immutable catalog key for system events; absent for custom events.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    /// A supported Goal action, performed only after every blocking result passes.
    #[serde(default)]
    pub on_success: Option<String>,
}

impl EventDefinition {
    /// The workflow owns its output contract; Skills supply reusable instructions.
    pub fn result_role(&self) -> &str {
        match self.source.as_deref() {
            Some("workflow.plan.enter") => "plan",
            Some("workflow.implement.enter") => "implement",
            Some("workflow.quality.enter") => "quality",
            Some("workflow.governance.enter") => "governance",
            _ => "task",
        }
    }
}

/// A shared manual trigger. Selecting it never creates a user-named Event.
pub fn custom_event() -> EventDefinition {
    EventDefinition {
        id: CUSTOM_EVENT_ID.into(),
        name: "Custom".into(),
        kind: EventKind::Custom,
        source: None,
        enabled: true,
        scope: Scope::default(),
        parameters: Vec::new(),
        bindings: Vec::new(),
        on_success: None,
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AutomationConfig {
    pub schema_version: u32,
    pub revision: u64,
    pub skills: BTreeMap<String, Skill>,
    pub events: BTreeMap<String, EventDefinition>,
}

pub fn system_catalog() -> Vec<String> {
    WORKFLOW_STEPS
        .iter()
        .flat_map(|step| {
            [
                format!("workflow.{step}.enter"),
                format!("workflow.{step}.exit"),
            ]
        })
        .chain(std::iter::once("node.startup.ready".into()))
        .collect()
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && !matches!(id, "." | "..")
        && id.len() <= 120
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

pub fn validate_parameters(parameters: &[Parameter]) -> Result<(), String> {
    let mut names = BTreeSet::new();
    if parameters.len() > 64 {
        return Err("at most 64 parameters are supported".into());
    }
    for p in parameters {
        if !valid_id(&p.name) || p.name.contains('.') || !names.insert(&p.name) {
            return Err(format!("invalid or repeated parameter name: {}", p.name));
        }
        if p.kind == ParameterType::Choice && p.choices.is_empty() {
            return Err(format!("{} requires choices", p.name));
        }
        if p.default.as_ref().is_some_and(|v| !p.accepts(v)) {
            return Err(format!("invalid default for {}", p.name));
        }
    }
    Ok(())
}

impl AutomationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if serde_json::to_vec_pretty(self)
            .map_err(|e| e.to_string())?
            .len()
            > MAX_CONFIG_BYTES
        {
            return Err("Events/Skills configuration exceeds 16 MiB".into());
        }
        if ![1, SCHEMA_VERSION].contains(&self.schema_version) {
            return Err("unsupported Events/Skills schema".into());
        }
        if self.events.len() > 1024 || self.skills.len() > 1024 {
            return Err("at most 1024 Events and Skills are supported".into());
        }
        for (id, skill) in &self.skills {
            if id != &skill.id
                || !valid_id(id)
                || skill.name.trim().is_empty()
                || skill.prompt.trim().is_empty()
                || skill.prompt.len() > 131_072
            {
                return Err(format!(
                    "invalid Skill {id}: supply an ID, name and prompt (at most 128 KiB)"
                ));
            }
            if skill.role != "task" && !WORKFLOW_STEPS.contains(&skill.role.as_str()) {
                return Err(format!("invalid Skill role {}", skill.role));
            }
            validate_scope(&skill.scope)?;
            validate_parameters(&skill.parameters)?;
        }
        let catalog = system_catalog();
        let mut assigned_skills = BTreeSet::new();
        for (id, event) in &self.events {
            if id != &event.id || !valid_id(id) || event.name.trim().is_empty() {
                return Err(format!("invalid Event {id}"));
            }
            validate_scope(&event.scope)?;
            validate_parameters(&event.parameters)?;
            match event.kind {
                EventKind::System if event.source.as_ref().is_none_or(|s| !catalog.contains(s)) => {
                    return Err(format!("unknown system source for {id}"));
                }
                EventKind::Custom if event.source.is_some() => {
                    return Err("custom Events cannot specify a system source".into());
                }
                _ => {}
            }
            if event
                .on_success
                .as_deref()
                .is_some_and(|a| !["start", "accept", "retry", "reopen"].contains(&a))
            {
                return Err("unsupported Event success action".into());
            }
            let max_bindings = if id == CUSTOM_EVENT_ID { 1024 * 64 } else { 64 };
            if event.bindings.len() > max_bindings {
                return Err(format!(
                    "at most {max_bindings} bindings per Event are supported"
                ));
            }
            let mut ids = BTreeSet::new();
            for binding in &event.bindings {
                if self.schema_version >= 2 && !assigned_skills.insert(&binding.skill_id) {
                    return Err("A Skill has one trigger. Clone the Skill to use it at another trigger point.".into());
                }
                if !valid_id(&binding.id) || !ids.insert(&binding.id) {
                    return Err("invalid or repeated binding ID".into());
                }
                validate_scope(&binding.scope)?;
                let skill = self.skills.get(&binding.skill_id).ok_or_else(|| {
                    format!(
                        "binding {} references missing Skill {}",
                        binding.id, binding.skill_id
                    )
                })?;
                if let Some(node) = &skill.scope.node_id {
                    let binding_node = binding
                        .scope
                        .node_id
                        .as_ref()
                        .or(event.scope.node_id.as_ref());
                    if binding_node.is_none_or(|n| !n.eq_ignore_ascii_case(node)) {
                        return Err("node Skills require a binding on the same node".into());
                    }
                }
                if let Some(overridden) = &binding.overrides {
                    if binding.scope.node_id.is_none()
                        || !event.bindings.iter().any(|b| {
                            &b.id == overridden
                                && b.scope.node_id.is_none()
                                && b.overrides.is_none()
                        })
                    {
                        return Err(
                            "overrides must identify a project binding and have node scope".into(),
                        );
                    }
                    if event
                        .bindings
                        .iter()
                        .filter(|b| {
                            b.overrides.as_ref() == Some(overridden) && b.scope == binding.scope
                        })
                        .count()
                        > 1
                    {
                        return Err("only one override per binding and node is allowed".into());
                    }
                }
                for (name, path) in &binding.inputs {
                    if !skill.parameters.iter().any(|p| &p.name == name)
                        || !["goal.", "system.", "event."]
                            .iter()
                            .any(|prefix| path.starts_with(prefix))
                        || path.split('.').any(|p| !valid_id(p))
                    {
                        return Err(format!("invalid parameter mapping {name}: {path}"));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn bindings<'a>(
        &'a self,
        event: &'a EventDefinition,
        node: &str,
    ) -> Vec<(&'a Binding, &'a Skill)> {
        if !event.enabled || !event.scope.applies(node) {
            return Vec::new();
        }
        let overridden: BTreeSet<_> = event
            .bindings
            .iter()
            .filter(|b| b.scope.applies(node))
            .filter_map(|b| b.overrides.as_deref())
            .collect();
        let mut bindings: Vec<_> = event
            .bindings
            .iter()
            .filter(|b| b.enabled && b.scope.applies(node) && !overridden.contains(b.id.as_str()))
            .filter_map(|b| {
                self.skills
                    .get(&b.skill_id)
                    .filter(|s| s.enabled && s.scope.applies(node))
                    .map(|s| (b, s))
            })
            .collect();
        bindings.sort_by(|(a, _), (b, _)| (a.order, &a.id).cmp(&(b.order, &b.id)));
        bindings
    }
}

fn validate_scope(scope: &Scope) -> Result<(), String> {
    if scope
        .node_id
        .as_ref()
        .is_some_and(|id| !valid_id(id) || id != &id.to_ascii_lowercase())
    {
        return Err("node IDs must be canonical lowercase IDs".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SkillResult {
    pub invocation_id: String,
    pub binding_id: String,
    pub role: String,
    /// success, failure (a finding), or error (execution infrastructure/contract).
    pub outcome: String,
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub artifacts: Value,
}

impl SkillResult {
    pub fn validate(
        &self,
        invocation_id: &str,
        binding_id: &str,
        role: &str,
    ) -> Result<(), String> {
        if self.invocation_id != invocation_id || self.binding_id != binding_id || self.role != role
        {
            return Err("Skill result identity does not match this invocation".into());
        }
        if !["success", "failure", "error"].contains(&self.outcome.as_str())
            || self.summary.trim().is_empty()
        {
            return Err("Skill result requires an outcome and summary".into());
        }
        if self.outcome == "success"
            && role == "plan"
            && !self.artifacts.get("plan").is_some_and(Value::is_object)
        {
            return Err("Plan Skills must return a plan artifact".into());
        }
        if self.outcome == "success"
            && ["quality", "governance"].contains(&role)
            && self.evidence.is_empty()
        {
            return Err("gate success requires supporting evidence".into());
        }
        if role == "governance" && self.outcome != "error" {
            let violations = self
                .artifacts
                .get("violations")
                .and_then(Value::as_array)
                .ok_or("Governance requires a violations array")?;
            if violations.iter().any(|v| {
                v.get("rule_id")
                    .and_then(Value::as_str)
                    .is_none_or(|s| s.trim().is_empty())
                    || v.get("message")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s.trim().is_empty())
            }) {
                return Err(
                    "Governance violations require stable rule_id and message fields".into(),
                );
            }
            if self.outcome == "success" && !violations.is_empty() {
                return Err("a Governance success cannot contain violations".into());
            }
            if self.outcome == "failure"
                && (violations.is_empty()
                    || self
                        .artifacts
                        .get("recovery_round_prompt")
                        .and_then(Value::as_str)
                        .is_none_or(|s| s.trim().is_empty()))
            {
                return Err(
                    "Governance findings require violations and an actionable recovery request"
                        .into(),
                );
            }
        }
        if role == "quality"
            && self.outcome == "success"
            && self
                .artifacts
                .get("tests")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
        {
            return Err("Quality requires supervised test commands".into());
        }
        Ok(())
    }
}

pub fn role_status(role: &str) -> Option<GoalStatus> {
    WORKFLOW_STEPS
        .contains(&role)
        .then(|| GoalStatus::parse_wire(role))
        .flatten()
}

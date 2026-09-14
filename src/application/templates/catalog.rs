use super::TemplateDefinition;
use crate::application::agent_io::prompts::{PromptEngine, PromptTemplate};
use std::collections::BTreeSet;

pub fn definitions() -> Vec<TemplateDefinition> {
    PromptTemplate::ALL
        .iter()
        .map(|template| {
            let id = template.id();
            let name = id
                .split('-')
                .map(|word| {
                    let mut letters = word.chars();
                    letters
                        .next()
                        .map(|first| first.to_uppercase().to_string() + letters.as_str())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ");
            TemplateDefinition {
                id,
                name,
                default_prompt: PromptEngine::load(*template).to_string(),
            }
        })
        .collect()
}

pub fn definition(id: &str) -> Option<TemplateDefinition> {
    definitions().into_iter().find(|item| item.id == id)
}

pub(crate) fn references(source: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("{{") {
        let escaped = start > 0 && rest.as_bytes()[start - 1] == b'\\';
        let tail = &rest[start + 2..];
        let end = tail.find("}}").ok_or("Unclosed template variable")?;
        if !escaped {
            let name = tail[..end].trim();
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            {
                return Err(format!("Invalid template variable: {name}"));
            }
            names.push(name.to_string());
        }
        rest = &tail[end + 2..];
    }
    Ok(names)
}

pub fn variables() -> serde_json::Value {
    let mut names = BTreeSet::new();
    for item in definitions() {
        names.extend(references(&item.default_prompt).unwrap_or_default());
    }
    names.extend(COMMON.iter().map(|(name, _)| name.to_string()));
    serde_json::json!(names.into_iter().filter(|name| !name.starts_with("templates.")).map(|name| {
        let description = COMMON.iter().find(|(key, _)| *key == name).map(|(_, description)| *description)
            .unwrap_or("Task data supplied by the launch that uses this template.");
        serde_json::json!({"name":name,"description":description,"kind":if matches!(name.as_str(), "skill" | "attached_skills") {"template"} else {"literal"},"optional": COMMON.iter().any(|(key,_)| *key == name)})
    }).collect::<Vec<_>>())
}

pub(crate) const COMMON: &[(&str, &str)] = &[
    (
        "refine_executable",
        "Absolute path to Refine on the executing node.",
    ),
    (
        "current_round_goal",
        "The current Round's authored Goal request; empty without a Goal.",
    ),
    (
        "skill",
        "The selected Skill, with its own variables expanded.",
    ),
    (
        "attached_skills",
        "Attached context Skills, rendered with their parameters.",
    ),
    (
        "workflow_step",
        "Current workflow step; empty outside a workflow.",
    ),
    ("project_root", "Target project's absolute root path."),
    ("workspace", "Agent's working directory."),
    ("node_id", "Executing Refine node."),
    (
        "goal_context",
        "Goal details and retained evidence supplied by this launch.",
    ),
    ("accepted_plan", "The current Round's accepted plan."),
    ("message", "The user's message, treated as literal data."),
    ("parameters", "Resolved Skill or task parameters."),
    (
        "completion_contract",
        "The response contract supplied by the operation.",
    ),
    (
        "resume_context",
        "Retained execution facts available when continuing work.",
    ),
];

pub(crate) fn validate(source: &str) -> Result<(), String> {
    let catalog = variables();
    for name in references(source)? {
        if let Some(id) = name.strip_prefix("templates.") {
            if definition(id).is_none() {
                return Err(format!("Unknown template: {id}"));
            }
        } else if !catalog
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == name)
        {
            return Err(format!("Unknown template variable: {name}"));
        }
    }
    Ok(())
}

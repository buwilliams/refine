use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptTemplate {
    Chat,
    ChatPlan,
    ChatGoal,
    ChatFeature,
    ChatSupervisor,
    ChatStandalone,
    ImportFeature,
    ImportRound,
    ImportPlanGoal,
    ImportStandaloneGoal,
    ImportNotes,
    Supervisor,
    ReleaseGoal,
    PostImplementationGovernance,
    GoalAgent,
    GovernanceGeneration,
    TargetAppGeneration,
    TargetAppLifecycle,
    TargetAppCommandStart,
    TargetAppCommandStop,
    TargetAppCommandBuild,
}

impl PromptTemplate {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Chat => "chat.md",
            Self::ChatPlan => "chat-plan.md",
            Self::ChatGoal => "chat-goal.md",
            Self::ChatFeature => "chat-feature.md",
            Self::ChatSupervisor => "chat-supervisor.md",
            Self::ChatStandalone => "chat-standalone.md",
            Self::ImportFeature => "import-feature.md",
            Self::ImportRound => "import-round.md",
            Self::ImportPlanGoal => "import-plan-goal.md",
            Self::ImportStandaloneGoal => "import-standalone-goal.md",
            Self::ImportNotes => "import-notes.md",
            Self::Supervisor => "supervisor.md",
            Self::ReleaseGoal => "release-goal.md",
            Self::PostImplementationGovernance => "post-implementation-governance.md",
            Self::GoalAgent => "goal-agent.md",
            Self::GovernanceGeneration => "governance-generation.md",
            Self::TargetAppGeneration => "target-app-generation.md",
            Self::TargetAppLifecycle => "target-app-lifecycle.md",
            Self::TargetAppCommandStart => "target-app-command-start.md",
            Self::TargetAppCommandStop => "target-app-command-stop.md",
            Self::TargetAppCommandBuild => "target-app-command-build.md",
        }
    }

    const fn source(self) -> &'static str {
        match self {
            Self::Chat => include_str!("chat.md"),
            Self::ChatPlan => include_str!("chat-plan.md"),
            Self::ChatGoal => include_str!("chat-goal.md"),
            Self::ChatFeature => include_str!("chat-feature.md"),
            Self::ChatSupervisor => include_str!("chat-supervisor.md"),
            Self::ChatStandalone => include_str!("chat-standalone.md"),
            Self::ImportFeature => include_str!("import-feature.md"),
            Self::ImportRound => include_str!("import-round.md"),
            Self::ImportPlanGoal => include_str!("import-plan-goal.md"),
            Self::ImportStandaloneGoal => include_str!("import-standalone-goal.md"),
            Self::ImportNotes => include_str!("import-notes.md"),
            Self::Supervisor => include_str!("supervisor.md"),
            Self::ReleaseGoal => include_str!("release-goal.md"),
            Self::PostImplementationGovernance => {
                include_str!("post-implementation-governance.md")
            }
            Self::GoalAgent => include_str!("goal-agent.md"),
            Self::GovernanceGeneration => include_str!("governance-generation.md"),
            Self::TargetAppGeneration => include_str!("target-app-generation.md"),
            Self::TargetAppLifecycle => include_str!("target-app-lifecycle.md"),
            Self::TargetAppCommandStart => include_str!("target-app-command-start.md"),
            Self::TargetAppCommandStop => include_str!("target-app-command-stop.md"),
            Self::TargetAppCommandBuild => include_str!("target-app-command-build.md"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromptTemplateError {
    DuplicateVariable(String),
    InvalidPlaceholder(String),
    MissingVariable(String),
    UnclosedPlaceholder,
    UnusedVariable(String),
}

impl fmt::Display for PromptTemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateVariable(name) => write!(formatter, "duplicate prompt variable: {name}"),
            Self::InvalidPlaceholder(name) => {
                write!(formatter, "invalid prompt placeholder: {name}")
            }
            Self::MissingVariable(name) => write!(formatter, "missing prompt variable: {name}"),
            Self::UnclosedPlaceholder => formatter.write_str("unclosed prompt placeholder"),
            Self::UnusedVariable(name) => write!(formatter, "unused prompt variable: {name}"),
        }
    }
}

pub struct PromptEngine;

impl PromptEngine {
    pub fn load(template: PromptTemplate) -> &'static str {
        template.source().trim()
    }

    pub fn render(
        template: PromptTemplate,
        variables: &[(&str, &str)],
    ) -> Result<String, PromptTemplateError> {
        let mut values = BTreeMap::new();
        for (name, value) in variables {
            if values.insert(*name, *value).is_some() {
                return Err(PromptTemplateError::DuplicateVariable((*name).to_string()));
            }
        }

        let source = Self::load(template);
        let mut output = String::with_capacity(source.len());
        let mut remaining = source;
        let mut used = BTreeSet::new();
        while let Some(start) = remaining.find("{{") {
            output.push_str(&remaining[..start]);
            let placeholder = &remaining[start + 2..];
            let Some(end) = placeholder.find("}}") else {
                return Err(PromptTemplateError::UnclosedPlaceholder);
            };
            let name = placeholder[..end].trim();
            if name.is_empty()
                || !name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
            {
                return Err(PromptTemplateError::InvalidPlaceholder(name.to_string()));
            }
            let value = values
                .get(name)
                .ok_or_else(|| PromptTemplateError::MissingVariable(name.to_string()))?;
            output.push_str(value);
            used.insert(name);
            remaining = &placeholder[end + 2..];
        }
        output.push_str(remaining);

        if let Some(name) = values.keys().find(|name| !used.contains(**name)) {
            return Err(PromptTemplateError::UnusedVariable((*name).to_string()));
        }
        Ok(output)
    }
}

pub fn render(template: PromptTemplate, variables: &[(&str, &str)]) -> String {
    PromptEngine::render(template, variables).unwrap_or_else(|error| {
        panic!(
            "invalid embedded prompt template {}: {error}",
            template.name()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_embedded_markdown_template_variables() {
        let rendered = PromptEngine::render(
            PromptTemplate::GoalAgent,
            &[
                ("goal_id", "goal-123"),
                ("goal_context", "{}"),
                ("previous_rounds", "[]"),
                ("latest_round", "{\"round\":1}"),
            ],
        )
        .unwrap();

        assert!(rendered.contains("ready Goal goal-123"));
        assert!(!rendered.contains("{{goal_id}}"));
    }

    #[test]
    fn rejects_missing_and_unused_variables() {
        assert_eq!(
            PromptEngine::render(PromptTemplate::GoalAgent, &[]),
            Err(PromptTemplateError::MissingVariable("goal_id".to_string()))
        );
        assert_eq!(
            PromptEngine::render(
                PromptTemplate::GoalAgent,
                &[
                    ("goal_id", "goal-123"),
                    ("goal_context", "{}"),
                    ("previous_rounds", "[]"),
                    ("latest_round", "{\"round\":1}"),
                    ("extra", "value"),
                ]
            ),
            Err(PromptTemplateError::UnusedVariable("extra".to_string()))
        );
    }

    #[test]
    fn loads_templates_without_trailing_file_whitespace() {
        let loaded = PromptEngine::load(PromptTemplate::ChatGoal);

        assert!(loaded.starts_with("Advance the attached Goal"));
        assert!(!loaded.ends_with('\n'));
    }

    #[test]
    fn agent_prompts_are_concise_and_unhobble_discovery() {
        let templates = [
            PromptTemplate::Chat,
            PromptTemplate::ChatPlan,
            PromptTemplate::ChatGoal,
            PromptTemplate::ChatFeature,
            PromptTemplate::ChatSupervisor,
            PromptTemplate::ChatStandalone,
            PromptTemplate::ImportFeature,
            PromptTemplate::ImportRound,
            PromptTemplate::ImportPlanGoal,
            PromptTemplate::ImportStandaloneGoal,
            PromptTemplate::ImportNotes,
            PromptTemplate::Supervisor,
            PromptTemplate::ReleaseGoal,
            PromptTemplate::PostImplementationGovernance,
            PromptTemplate::GoalAgent,
            PromptTemplate::GovernanceGeneration,
            PromptTemplate::TargetAppGeneration,
            PromptTemplate::TargetAppLifecycle,
            PromptTemplate::TargetAppCommandStart,
            PromptTemplate::TargetAppCommandStop,
            PromptTemplate::TargetAppCommandBuild,
        ];
        for template in templates {
            let word_count = PromptEngine::load(template).split_whitespace().count();
            assert!(
                word_count <= 120,
                "{} is too prescriptive at {word_count} words",
                template.name()
            );
        }

        let plan = PromptEngine::load(PromptTemplate::ChatPlan);
        for strategy in [
            "map",
            "territory",
            "blind-spot paths",
            "prototype",
            "interview the user",
            "good, fast, and cheap",
        ] {
            assert!(plan.contains(strategy), "plan prompt omitted {strategy}");
        }

        let goal = PromptEngine::load(PromptTemplate::GoalAgent);
        assert!(goal.contains("strongest solution"));
        assert!(goal.contains("good, fast, and cheap"));

        let supervisor = PromptEngine::load(PromptTemplate::Supervisor);
        assert!(supervisor.contains("interview the user"));
        assert!(supervisor.contains("Never hide provider failures"));
        assert!(supervisor.contains("force merges"));
        assert!(supervisor.contains("discard worktrees"));
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub mod implementation_planning;
pub mod structured_output;

macro_rules! prompt_templates {
    ($($variant:ident => $path:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub enum PromptTemplate {
            $($variant),+
        }

        impl PromptTemplate {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $path),+
                }
            }

            const fn source(self) -> &'static str {
                match self {
                    $(Self::$variant => include_str!($path)),+
                }
            }
        }
    };
}

prompt_templates! {
    AgentProviderFileBootstrap => "agent_providers/file-bootstrap.md",
    Chat => "chat/session.md",
    ChatPlan => "chat/plan.md",
    ChatGoal => "chat/goal.md",
    ChatFeature => "chat/feature.md",
    ChatAgent => "chat/agent.md",
    ChatStandalone => "chat/standalone.md",
    GoalAgentSpec => "goal_agents/spec.md",
    GoalAgent => "goal_agents/prompt.md",
    GoalAgentSession => "goal_agents/session.md",
    GoalAgentWorkflowSummary => "goal_agents/workflow-summary.md",
    GoalWorkflowRecoverReconciliation => "goal_workflow/recover-reconciliation.md",
    GoalWorkflowRecoverIntegration => "goal_workflow/recover-integration.md",
    GovernanceGeneration => "governance/generation.md",
    PlanGovernancePrecheck => "governance/plan-precheck.md",
    PostImplementationGovernance => "governance/post-implementation.md",
    ImplementationPlanningPlan => "implementation_planning/plan.md",
    ImplementationPlanningCriticize => "implementation_planning/criticize.md",
    ImplementationPlanningRevise => "implementation_planning/revise.md",
    ImplementationPlanningImplement => "implementation_planning/implement.md",
    ImportFeature => "imports/feature.md",
    ImportRound => "imports/round.md",
    ImportPlanGoal => "imports/plan-goal.md",
    ImportStandaloneGoal => "imports/standalone-goal.md",
    ImportNotes => "imports/notes.md",
    PostImplementationQuality => "quality/post-implementation.md",
    StructuredOutputRepair => "structured_output/repair.md",
    GoalWorkflowQualityAgent => "quality/agent.md",
    GoalWorkflowQualityRecovery => "quality/recovery.md",
    QualityDefaultInstructions => "quality/default-instructions.md",
    ReleaseGoal => "release/goal.md",
    TargetAppGeneration => "target_apps/generation.md",
    TargetAppLifecycle => "target_apps/lifecycle.md",
    TargetAppCommandStart => "target_apps/command-start.md",
    TargetAppCommandStop => "target_apps/command-stop.md",
    TargetAppCommandBuild => "target_apps/command-build.md",
    TerminalProfileGeneralAgentWorkflow => "terminal_profiles/general-agent-workflow.md",
    TerminalProfileActiveRefine => "terminal_profiles/active-refine.md",
    TerminalProfileAttachedGoal => "terminal_profiles/attached-goal.md",
    TerminalProfileAttachedFeature => "terminal_profiles/attached-feature.md",
    TerminalProfilePlan => "terminal_profiles/plan.md",
    TerminalProfileGoalDiagnostic => "terminal_profiles/goal-diagnostic.md",
    TerminalProfileSupplementalContext => "terminal_profiles/supplemental-context.md",
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
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn renders_embedded_markdown_template_variables() {
        let rendered = PromptEngine::render(
            PromptTemplate::GoalAgent,
            &[("spec", "# Goal Agent Specification")],
        )
        .unwrap();

        assert_eq!(rendered, "# Goal Agent Specification");
        assert!(!rendered.contains("{{spec}}"));
    }

    #[test]
    fn rejects_missing_and_unused_variables() {
        assert_eq!(
            PromptEngine::render(PromptTemplate::GoalAgent, &[]),
            Err(PromptTemplateError::MissingVariable("spec".to_string()))
        );
        assert_eq!(
            PromptEngine::render(
                PromptTemplate::GoalAgent,
                &[("spec", "specification"), ("extra", "value")]
            ),
            Err(PromptTemplateError::UnusedVariable("extra".to_string()))
        );
    }

    #[test]
    fn loads_templates_without_trailing_file_whitespace() {
        let loaded = PromptEngine::load(PromptTemplate::ChatGoal);

        assert!(loaded.starts_with("Help advance the attached Goal"));
        assert!(!loaded.ends_with('\n'));
    }

    #[test]
    fn standalone_chat_waits_for_user_in_an_unassigned_worktree() {
        let loaded = PromptEngine::load(PromptTemplate::ChatStandalone);

        assert!(loaded.contains("empty of assigned work"));
        assert!(loaded.contains("until the user tells you what they want"));
    }

    #[test]
    fn quality_commands_must_encode_pass_semantics_in_their_exit_status() {
        let loaded = PromptEngine::load(PromptTemplate::PostImplementationQuality);

        assert!(loaded.contains("exit 0 iff the test passes"));
        assert!(loaded.contains("never return grep's no-match exit 1 for a pass"));
    }

    #[test]
    fn quality_defaults_are_template_owned() {
        let loaded = PromptEngine::load(PromptTemplate::QualityDefaultInstructions);

        assert!(loaded.contains("Evaluate every Quality test"));
        assert!(loaded.contains("Do not change product code"));
    }

    #[test]
    fn every_feature_template_is_registered_exactly_once() {
        fn markdown_files(root: &Path, directory: &Path, files: &mut Vec<String>) {
            for entry in fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    markdown_files(root, &path, files);
                } else if path.extension().and_then(|extension| extension.to_str()) == Some("md") {
                    files.push(
                        path.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/prompts");
        let mut actual = Vec::new();
        markdown_files(&root, &root, &mut actual);
        actual.sort();

        let mut registered = PromptTemplate::ALL
            .iter()
            .map(|template| template.name().to_string())
            .collect::<Vec<_>>();
        registered.sort();
        registered.dedup();

        assert_eq!(registered.len(), PromptTemplate::ALL.len());
        assert_eq!(registered, actual);
        assert!(
            registered
                .iter()
                .all(|name| name.contains('/') && name.ends_with(".md")),
            "every prompt template must live under src/prompts/<feature>/"
        );
    }

    #[test]
    fn agent_prompts_keep_only_task_specific_contracts() {
        for &template in PromptTemplate::ALL {
            let word_count = PromptEngine::load(template).split_whitespace().count();
            let word_limit = match template {
                PromptTemplate::AgentProviderFileBootstrap => 140,
                PromptTemplate::TerminalProfileGeneralAgentWorkflow => 180,
                _ => 90,
            };
            assert!(
                word_count <= word_limit,
                "{} is too prescriptive at {word_count} words",
                template.name()
            );
            let prompt = PromptEngine::load(template).to_ascii_lowercase();
            for boilerplate in [
                "map and the",
                "map and available",
                "blind-spot paths",
                "prototype uncertain",
                "good, fast, and cheap",
            ] {
                assert!(
                    !prompt.contains(boilerplate),
                    "{} repeats general intent: {boilerplate}",
                    template.name()
                );
            }
        }
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

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

            pub fn id(self) -> String {
                match self {
                    Self::ChatPlan => "planning-agent".into(),
                    Self::ChatAgent => "agent".into(),
                    Self::ChatGoal => "goal-agent".into(),
                    Self::Workflow => "workflow".into(),
                    _ => self.name().trim_start_matches("templates/").trim_end_matches(".md").replace(['/', '_'], "-"),
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
    ChatContextUnavailable => "templates/chat-context-unavailable.md",
    FleetManage => "templates/fleet-manage.md",
    FleetDistribute => "templates/fleet-distribute.md",
    SyncOwnershipDoctrine => "templates/sync-ownership-doctrine.md",
    ConflictResolution => "templates/conflict-resolution.md",
    ConflictAncestry => "templates/conflict-ancestry.md",
    ConflictFeedback => "templates/conflict-feedback.md",
    TerminalSession => "templates/terminal-session.md",
    Workflow => "templates/workflow.md",
    SupervisedSkill => "templates/supervised-skill.md",
    WorkflowContext => "templates/workflow-context.md",
    WorkflowContinuation => "templates/workflow-continuation.md",
    WorkflowObservational => "templates/workflow-observational.md",
    ContextSkill => "templates/context-skill.md",
    ManualSkill => "templates/manual-skill.md",
    SkillRepair => "templates/skill-repair.md",
    DirectAgent => "templates/direct-agent.md",
    GoalCompletion => "templates/goal-completion.md",
    SignalRepair => "templates/signal-repair.md",
    SourceUpgrade => "templates/source-upgrade.md",
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
    ImportFeature => "imports/feature.md",
    ImportRound => "imports/round.md",
    ImportPlanGoal => "imports/plan-goal.md",
    ImportStandaloneGoal => "imports/standalone-goal.md",
    ImportNotes => "imports/notes.md",
    StructuredOutputRepair => "structured_output/repair.md",
    GoalWorkflowQualityRecovery => "quality/recovery.md",
    QualityDefaultInstructions => "quality/default-instructions.md",
    ReleaseGoal => "release/goal.md",
    ResolveStateConflict => "sync/resolve-state-conflict.md",
    ResolveCandidateConflict => "sync/resolve-candidate-conflict.md",
    TargetAppGeneration => "target_apps/generation.md",
    TargetAppLifecycle => "target_apps/lifecycle.md",
    TargetAppCommandStart => "target_apps/command-start.md",
    TargetAppCommandStop => "target_apps/command-stop.md",
    TargetAppCommandBuild => "target_apps/command-build.md",
    TerminalProfileGeneralAgentWorkflow => "terminal_profiles/general-agent-workflow.md",
    TerminalProfileToolbarAgentWorkflow => "terminal_profiles/toolbar-agent-workflow.md",
    TerminalProfileActiveRefine => "terminal_profiles/active-refine.md",
    TerminalProfileAttachedGoal => "terminal_profiles/attached-goal.md",
    TerminalProfileAttachedFeature => "terminal_profiles/attached-feature.md",
    TerminalProfilePlan => "terminal_profiles/plan.md",
    TerminalProfileGoalDiagnostic => "terminal_profiles/goal-diagnostic.md",
    TerminalProfileSupplementalContext => "terminal_profiles/supplemental-context.md",
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromptTemplateError {
    Resolution(String),
    DuplicateVariable(String),
    InvalidPlaceholder(String),
    MissingVariable(String),
    UnclosedPlaceholder,
    UnusedVariable(String),
}

impl fmt::Display for PromptTemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolution(message) => formatter.write_str(message),
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
        Self::render_source(Self::load(template), variables, true)
    }

    /// Render supported variables in authored text, preserving other template syntax.
    pub fn render_available(
        source: &str,
        variables: &[(&str, &str)],
    ) -> Result<String, PromptTemplateError> {
        Self::render_source(source, variables, false)
    }

    fn render_source(
        source: &str,
        variables: &[(&str, &str)],
        strict: bool,
    ) -> Result<String, PromptTemplateError> {
        let mut values = BTreeMap::new();
        for (name, value) in variables {
            if values.insert(*name, *value).is_some() {
                return Err(PromptTemplateError::DuplicateVariable((*name).to_string()));
            }
        }

        let mut used = BTreeSet::new();
        let output = Self::render_resolved(source, strict, usize::MAX, |name| {
            Ok(values.get(name).map(|value| {
                used.insert(name.to_string());
                (*value).to_string()
            }))
        })?;

        if strict && let Some(name) = values.keys().find(|name| !used.contains(**name)) {
            return Err(PromptTemplateError::UnusedVariable((*name).to_string()));
        }
        Ok(output)
    }
    /// Shared single-pass tokenizer. Resolvers may expand template-valued inputs;
    /// their returned bytes are appended literally and never tokenized again.
    pub fn render_resolved(
        source: &str,
        strict: bool,
        max_bytes: usize,
        mut resolve: impl FnMut(&str) -> Result<Option<String>, PromptTemplateError>,
    ) -> Result<String, PromptTemplateError> {
        let mut output = String::new();
        let mut remaining = source;
        while let Some(start) = remaining.find("{{") {
            let escaped = start > 0 && remaining.as_bytes()[start - 1] == b'\\';
            let placeholder = &remaining[start + 2..];
            let Some(end) = placeholder.find("}}") else {
                if strict {
                    return Err(PromptTemplateError::UnclosedPlaceholder);
                }
                break;
            };
            output.push_str(&remaining[..if escaped { start - 1 } else { start }]);
            let name = placeholder[..end].trim();
            let token = &remaining[start..start + 2 + end + 2];
            if escaped {
                output.push_str(token);
            } else {
                if strict
                    && (name.is_empty()
                        || !name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
                {
                    return Err(PromptTemplateError::InvalidPlaceholder(name.into()));
                }
                match resolve(name)? {
                    Some(value) => {
                        if value.len() > max_bytes.saturating_sub(output.len()) {
                            return Err(PromptTemplateError::Resolution(
                                "Expanded template exceeds the size limit".into(),
                            ));
                        }
                        output.push_str(&value);
                    }
                    None if strict => {
                        return Err(PromptTemplateError::MissingVariable(name.into()));
                    }
                    None => output.push_str(token),
                }
            }
            if output.len() > max_bytes {
                return Err(PromptTemplateError::Resolution(
                    "Expanded template exceeds the size limit".into(),
                ));
            }
            remaining = &placeholder[end + 2..];
        }
        if remaining.len() > max_bytes.saturating_sub(output.len()) {
            return Err(PromptTemplateError::Resolution(
                "Expanded template exceeds the size limit".into(),
            ));
        }
        output.push_str(remaining);
        Ok(output)
    }
}

pub fn render(
    template: PromptTemplate,
    variables: &[(&str, &str)],
) -> crate::error::RefineResult<String> {
    crate::application::templates::TemplateScope::render(
        &template.id(),
        crate::application::templates::TemplateScope::literals(variables),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn authored_templates_render_once_and_preserve_unrelated_text() {
        let source = "  {{refine_executable}} / {{ refine_executable }} {{user.name}} {{ other }} {{ unfinished\n";
        let path = "/opt/Refine {{other}}/refine";
        assert_eq!(
            PromptEngine::render_available(source, &[("refine_executable", path)]).unwrap(),
            format!("  {path} / {path} {{{{user.name}}}} {{{{ other }}}} {{{{ unfinished\n")
        );
        assert_eq!(
            PromptEngine::render_available("ordinary skill\n", &[("refine_executable", path)])
                .unwrap(),
            "ordinary skill\n"
        );
    }

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
    fn quality_defaults_are_template_owned() {
        let loaded = PromptEngine::load(PromptTemplate::QualityDefaultInstructions);

        assert!(loaded.contains("project instructions and your judgment"));
        assert!(loaded.contains("run checks when useful"));
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

        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/application/agent_io/prompts");
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
            "every prompt template must live under src/application/agent_io/prompts/<feature>/"
        );
    }

    #[test]
    fn agent_prompts_keep_only_task_specific_contracts() {
        for &template in PromptTemplate::ALL {
            let word_count = PromptEngine::load(template).split_whitespace().count();
            let word_limit = match template {
                PromptTemplate::AgentProviderFileBootstrap => 140,
                // Existing orchestration contracts moved here unchanged.
                PromptTemplate::GoalCompletion => 100,
                PromptTemplate::SourceUpgrade => 150,
                PromptTemplate::SupervisedSkill | PromptTemplate::SyncOwnershipDoctrine => 120,
                PromptTemplate::TerminalProfileGeneralAgentWorkflow
                | PromptTemplate::TerminalProfileToolbarAgentWorkflow => 180,
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

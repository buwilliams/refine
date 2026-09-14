//! Human-readable launch context for the editable catalog.
use crate::application::agent_io::prompts::PromptTemplate;
use serde::Serialize;

#[derive(Serialize)]
pub struct TemplateUsage {
    pub kind: &'static str,
    pub group: &'static str,
    pub description: &'static str,
}

pub fn usage(template: PromptTemplate) -> TemplateUsage {
    use PromptTemplate::*;
    let (kind, group, description) = match template {
        Workflow => (
            "template",
            "workflow",
            "Builds the Skill prompt for Plan, Implement, Quality, and Governance.",
        ),
        GoalAgentSession => (
            "template",
            "workflow",
            "Wraps supervised agent prompts with session completion instructions.",
        ),
        WorkflowContext => (
            "partial",
            "workflow",
            "Explains the shared Goal workflow to the agent.",
        ),
        SupervisedSkill => (
            "template",
            "workflow",
            "Combines a supervised Skill with task context and the response contract. Also included by Workflow.",
        ),
        ContextSkill => (
            "partial",
            "workflow",
            "Formats each attached context Skill.",
        ),
        WorkflowContinuation => (
            "partial",
            "workflow",
            "Included when a supervised Skill resumes interrupted work.",
        ),
        WorkflowObservational => (
            "partial",
            "workflow",
            "Included when a supervised Skill runs in verification-only mode.",
        ),
        GoalCompletion => (
            "partial",
            "workflow",
            "Explains how a workflow agent reports session completion.",
        ),
        SkillRepair => (
            "template",
            "workflow",
            "Sent when a supervised Skill returns an invalid response.",
        ),
        SignalRepair => (
            "template",
            "workflow",
            "Sent when a workflow session returns an invalid completion signal.",
        ),
        GoalAgentWorkflowSummary => (
            "partial",
            "workflow",
            "Workflow summary retained in a Goal's agent context.",
        ),
        TerminalSession => (
            "template",
            "interactive",
            "Builds the initial prompt for agent terminals opened from the toolbar or CLI.",
        ),
        Chat => (
            "template",
            "interactive",
            "Builds each managed chat turn from its mode, attachment, and user message.",
        ),
        ChatPlan => (
            "partial",
            "interactive",
            "Planning Agent instructions used by planning terminals and Plan-mode chats.",
        ),
        ChatAgent => (
            "partial",
            "interactive",
            "Agent instructions used by general agent terminals.",
        ),
        ChatGoal => (
            "partial",
            "interactive",
            "Goal Agent instructions used by Goal diagnostic terminals and Goal chats.",
        ),
        ChatFeature => (
            "partial",
            "interactive",
            "Instructions for chats attached to a Feature.",
        ),
        ChatStandalone => (
            "partial",
            "interactive",
            "Instructions for standalone chats and agent terminals.",
        ),
        ChatContextUnavailable => (
            "partial",
            "interactive",
            "Explains missing attachment context in a managed chat.",
        ),
        TerminalProfileGeneralAgentWorkflow => (
            "partial",
            "interactive",
            "Workflow guidance for agents opened through the CLI.",
        ),
        TerminalProfileToolbarAgentWorkflow => (
            "partial",
            "interactive",
            "Workflow guidance for agents opened from the toolbar.",
        ),
        TerminalProfileActiveRefine => (
            "partial",
            "interactive",
            "Identifies the active Refine installation in an agent terminal.",
        ),
        TerminalProfileAttachedGoal => (
            "partial",
            "interactive",
            "Adds attached Goal details to an agent terminal.",
        ),
        TerminalProfileAttachedFeature => (
            "partial",
            "interactive",
            "Adds attached Feature details to an agent terminal.",
        ),
        TerminalProfilePlan => (
            "partial",
            "interactive",
            "Adds planning-specific terminal context.",
        ),
        TerminalProfileGoalDiagnostic => (
            "partial",
            "interactive",
            "Adds diagnostic context when opening a Goal terminal.",
        ),
        TerminalProfileSupplementalContext => (
            "partial",
            "interactive",
            "Adds extra context supplied when opening an agent terminal.",
        ),
        ManualSkill => (
            "template",
            "interactive",
            "Builds the prompt when a user runs a Skill in an interactive terminal.",
        ),
        DirectAgent => (
            "template",
            "tasks",
            "Used by direct agent invocation through the CLI or API.",
        ),
        FleetManage => (
            "template",
            "tasks",
            "Starts an interactive agent for fleet management.",
        ),
        FleetDistribute => (
            "partial",
            "tasks",
            "Adds work-distribution instructions to a fleet-management request.",
        ),
        SourceUpgrade => (
            "template",
            "tasks",
            "Starts the agent that prepares a Refine source upgrade.",
        ),
        ImportFeature => (
            "template",
            "tasks",
            "Extracts a Feature and its Goals from supplied text.",
        ),
        ImportRound => (
            "template",
            "tasks",
            "Extracts a new Goal Round from supplied text.",
        ),
        ImportPlanGoal => (
            "template",
            "tasks",
            "Extracts a single planning Goal from supplied text.",
        ),
        ImportStandaloneGoal => (
            "template",
            "tasks",
            "Extracts a standalone Goal from supplied text.",
        ),
        ImportNotes => (
            "template",
            "tasks",
            "Extracts Goal notes from supplied text.",
        ),
        ReleaseGoal => (
            "template",
            "tasks",
            "Creates the authored request for a release-preparation Goal.",
        ),
        TargetAppGeneration => (
            "template",
            "tasks",
            "Asks an agent to generate target-app configuration.",
        ),
        TargetAppLifecycle => (
            "template",
            "tasks",
            "Runs a configured target-app lifecycle task through an agent.",
        ),
        TargetAppCommandStart | TargetAppCommandStop | TargetAppCommandBuild => (
            "partial",
            "tasks",
            "Seeds editable lifecycle instructions when converting command-based target-app configuration.",
        ),
        ConflictResolution => (
            "template",
            "tasks",
            "Builds a state-conflict resolution agent request.",
        ),
        ResolveStateConflict => (
            "partial",
            "tasks",
            "Supplies conflicting Refine state and resolution instructions.",
        ),
        SyncOwnershipDoctrine => (
            "partial",
            "tasks",
            "Explains state ownership to a conflict-resolution agent.",
        ),
        ConflictAncestry => (
            "partial",
            "tasks",
            "Adds the relationship between the two conflicting histories.",
        ),
        ConflictFeedback => (
            "partial",
            "tasks",
            "Adds rejection feedback when conflict resolution is retried.",
        ),
        AgentProviderFileBootstrap => (
            "template",
            "delivery",
            "Sent instead of a large inline prompt when the provider needs a file handoff.",
        ),
        StructuredOutputRepair => (
            "template",
            "library",
            "Available to structured-output repair callers; no current built-in launch uses it.",
        ),
        QualityDefaultInstructions => (
            "partial",
            "library",
            "Reference text for initial Quality Skill migration. Edit the active Quality Skill in Settings → Skills.",
        ),
        GoalAgentSpec
        | GoalAgent
        | GoalWorkflowRecoverReconciliation
        | GoalWorkflowRecoverIntegration
        | GoalWorkflowQualityRecovery
        | ResolveCandidateConflict => (
            "partial",
            "library",
            "Available for inclusion by custom Templates; no current built-in launch uses this entry.",
        ),
    };
    TemplateUsage {
        kind,
        group,
        description,
    }
}

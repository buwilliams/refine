use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

pub mod behavior;
pub mod behaviors;
pub mod capacity;
pub mod context;
pub mod promotion;

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::model::JsonObject;
use crate::model::feature::{compare_feature_goal_order, is_ordered_feature_goal};
use crate::model::goal::{GoalPriority, RoundIntegration};
use crate::model::log::LogEntry;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::{FileProcessSupervisor, ProcessPauseState};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::coordination::acquire_workflow_coordination;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::FileOperationRegistry;
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::git_sync::with_repository_git_lock;
use crate::tools::host::git_worktrees::{FileGitWorktreeService, MergeResult};
use crate::tools::host::project_layout::prepare_refine_dir;
use crate::tools::observability::logs::FileLogService;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::process_control::FileProcessControlService;
use crate::tools::product::project_state::{
    FileProjectStateStore, GoalSummaryProjection, ProjectionSnapshot,
};
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::behavior::{WorkflowAdvanceOutcome, WorkflowBehavior};
use crate::workflow::behaviors::{
    WorkflowBuild, WorkflowImplementation, WorkflowQa, WorkflowReadyMerge, WorkflowReview,
    WorkflowTodo,
};
use crate::workflow::capacity::{AgentCapacityRequest, AgentCapacityService};
use crate::workflow::context::WorkflowContext;
use crate::workflow::promotion::BacklogPromotionService;

pub const WORKFLOW_AUTOMATION_STATE_FILE: &str = "workflow-automation-state.json";
const WORKFLOW_AUTOMATION_STATE_LOCK_FILE: &str = ".workflow-automation-state.lock";
const AUTOMATION_CONCURRENCY_LIMIT_REACHED: &str = "automation concurrency limit reached";
const ACTIVE_WORK_REPLENISH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowClaimState {
    Claimed,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowClaim {
    pub claim_id: String,
    #[serde(alias = "gap_id")]
    pub goal_id: String,
    #[serde(default = "default_node_id")]
    pub node_id: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_target_app_id")]
    pub target_app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round_idx: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_revision: Option<u64>,
    #[serde(default)]
    pub decision_version: u64,
    pub state: WorkflowClaimState,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowPolicy {
    pub global_limit: usize,
    pub per_node_limit: usize,
    pub per_provider_limit: usize,
    pub per_target_app_limit: usize,
    pub active_node_id: String,
    pub provider: String,
    pub target_app_id: String,
}

impl Default for WorkflowPolicy {
    fn default() -> Self {
        Self {
            global_limit: 2,
            per_node_limit: 1,
            per_provider_limit: 2,
            per_target_app_limit: 2,
            active_node_id: default_node_id(),
            provider: default_provider(),
            target_app_id: default_target_app_id(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowAutomationState {
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub policy: WorkflowPolicy,
    pub claims: Vec<WorkflowClaim>,
    pub updated_at: Option<String>,
}

impl WorkflowAutomationState {
    pub(crate) fn active_claim(&self, goal_id: &str) -> Option<&WorkflowClaim> {
        self.claims.iter().find(|claim| {
            claim.goal_id == goal_id
                && matches!(
                    claim.state,
                    WorkflowClaimState::Claimed | WorkflowClaimState::Running
                )
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowExecutionFence {
    pub claim_id: String,
    pub execution_id: String,
    pub goal_id: String,
    pub node_id: String,
    pub round_idx: usize,
    pub goal_revision: u64,
    pub decision_version: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowPassResult {
    pub promoted: usize,
    pub claims: Vec<WorkflowClaim>,
    pub steps: Vec<WorkflowStepResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowStepResult {
    pub claim_id: String,
    pub goal_id: String,
    pub execution_id: String,
    pub provider: String,
    pub branch: String,
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeResult>,
    pub final_status: String,
    pub provider_output: String,
}

pub trait WorkflowAutomation {
    fn promote(&self) -> RefineResult<usize>;
    fn claim(&self, goal_id: &str) -> RefineResult<String>;
    fn start_claim(&self, claim_id: &str) -> RefineResult<String>;
    fn cancel(&self, execution_id: &str) -> RefineResult<()>;
    fn retry(&self, execution_id: &str) -> RefineResult<String>;
}

#[derive(Clone)]
pub struct WorkflowEngine {
    pub runtime_root: PathBuf,
    pub target_root: Option<PathBuf>,
    #[cfg(test)]
    before_worker_prepare_hook: Option<std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>>,
}

impl std::fmt::Debug for WorkflowEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkflowEngine")
            .field("runtime_root", &self.runtime_root)
            .field("target_root", &self.target_root)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct WorkflowStateMutationLock {
    file: File,
}

impl Drop for WorkflowStateMutationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

impl WorkflowEngine {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ClaimLoad {
    global: usize,
    by_node: BTreeMap<String, usize>,
    by_provider: BTreeMap<String, usize>,
    by_target_app: BTreeMap<String, usize>,
}

impl ClaimLoad {
    fn ensure_policy_keys(&mut self, policy: &WorkflowPolicy) {
        self.by_node
            .entry(policy.active_node_id.clone())
            .or_default();
        self.by_provider.entry(policy.provider.clone()).or_default();
        self.by_target_app
            .entry(policy.target_app_id.clone())
            .or_default();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClaimMetadata {
    node_id: String,
    provider: String,
    target_app_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct GovernanceEvaluation {
    failed: bool,
    message: Option<String>,
    details: JsonObject,
}

fn read_state(path: &Path) -> RefineResult<WorkflowAutomationState> {
    if !path.exists() {
        return Ok(WorkflowAutomationState::default());
    }
    let bytes = fs::read(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read automation state {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice::<WorkflowAutomationState>(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse automation state {}: {error}",
            path.display()
        ))
    })
}

fn write_state(path: &Path, state: &WorkflowAutomationState) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create automation state directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
        RefineError::Serialization(format!("failed to encode automation state: {error}"))
    })?;
    let temp_path = path.with_extension(format!("json.{}.tmp", Uuid::new_v4()));
    fs::write(&temp_path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write automation state {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        RefineError::Io(format!(
            "failed to publish automation state {}: {error}",
            path.display()
        ))
    })
}

fn setting_usize(settings: &JsonObject, key: &str, fallback: usize) -> usize {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

fn setting_cap_with_default_values(
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

fn setting_string(settings: &JsonObject, key: &str, fallback: &str) -> String {
    settings
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn ensure_workflow_round(work_items: &FileWorkItemService, goal_id: &str) -> RefineResult<usize> {
    let goal = work_items.show_goal_summary(goal_id)?;
    if let Some(idx) = goal.goal.round_count.checked_sub(1) {
        return Ok(idx);
    }
    let goal = work_items.append_goal_round_summary(
        goal_id,
        "Refine",
        "Implement and verify this Goal.",
    )?;
    goal.goal
        .round_count
        .checked_sub(1)
        .ok_or_else(|| RefineError::InvalidInput(format!("Goal {goal_id} has no rounds")))
}

fn hydrate_retry_context(ctx: &mut WorkflowContext<'_>, current: GoalStatus) -> RefineResult<()> {
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let branch = required_workflow_string(&detail, "branch_name", &ctx.goal_id)?;
    let candidate = required_workflow_string(&detail, "candidate_commit", &ctx.goal_id)?;
    let base = required_workflow_string(&detail, "base_commit", &ctx.goal_id)?;
    let round = detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {} has no round {} to resume",
                ctx.goal_id,
                ctx.round_idx + 1
            ))
        })?;
    let worktree = FileGitWorktreeService::with_runtime_root(ctx.target_root, ctx.runtime_root)
        .existing_worktree_for_branch(&branch)?;
    ctx.branch = Some(branch.clone());
    ctx.worktree_path = worktree.as_ref().map(|path| path.display().to_string());
    ctx.agent_cwd = worktree;
    ctx.provider_output = Some(
        round
            .get("implementation_report")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "Resumed existing workflow candidate".to_string()),
    );
    ctx.commit = Some(candidate.clone());
    ctx.implementation_changed = candidate != base;
    ctx.merge = round
        .get("workflow_integration")
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value::<RoundIntegration>(value.clone())
                .map(|integration| integration.merge)
                .map_err(|error| {
                    RefineError::Serialization(format!(
                        "Goal {} has invalid Ready Merge evidence: {error}",
                        ctx.goal_id
                    ))
                })
        })
        .transpose()?;
    ctx.start_status = current.clone();
    ctx.log(
        "workflow",
        &format!("Resumed workflow from {}", current.as_str()),
        Some(json_object(json!({
            "status": current.as_str(),
            "branch": branch,
            "candidate_commit": candidate,
            "round": ctx.round_idx + 1
        }))),
    )
}

fn required_workflow_string(value: &Value, key: &str, goal_id: &str) -> RefineResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            RefineError::Conflict(format!(
                "Goal {goal_id} has no recorded {key} to resume workflow"
            ))
        })
}

fn implementation_branch_name(pattern: &str, goal_id: &str, round_idx: usize) -> String {
    let pattern = pattern.trim();
    let base = if pattern.is_empty() {
        "refine/{goal_id}"
    } else {
        pattern
    };
    let round = (round_idx + 1).to_string();
    let branch = base
        .replace("{goal_id}", goal_id)
        .replace("{goal}", goal_id)
        .replace("{round}", &round);
    if branch.contains(&format!("round-{round}")) || branch.contains(&format!("round/{round}")) {
        branch
    } else {
        format!("{branch}/round-{round}")
    }
}

fn agent_worktree_cwd(worktree_path: &str, agent_subpath: &str) -> RefineResult<PathBuf> {
    let root = PathBuf::from(worktree_path);
    let subpath = agent_subpath.trim();
    if subpath.is_empty() {
        return Ok(root);
    }
    let relative = Path::new(subpath);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(RefineError::InvalidInput(
            "agent_subpath must be a relative path inside the worktree".to_string(),
        ));
    }
    Ok(root.join(relative))
}

fn post_implementation_governance_prompt(
    governance: &Value,
    rules: &[Value],
    worktree_path: &str,
    provider_cwd: &Path,
    goal_id: &str,
    round_idx: usize,
) -> String {
    let product = governance
        .get("product")
        .and_then(Value::as_str)
        .unwrap_or("");
    let constitution = governance
        .get("constitution")
        .and_then(Value::as_str)
        .unwrap_or("");
    let rules_json = serde_json::to_string_pretty(rules).unwrap_or_else(|_| "[]".to_string());
    let round_number = (round_idx + 1).to_string();
    let provider_cwd = provider_cwd.display().to_string();
    render(
        PromptTemplate::PostImplementationGovernance,
        &[
            ("goal_id", goal_id),
            ("round_number", &round_number),
            ("worktree_path", worktree_path),
            ("provider_cwd", &provider_cwd),
            ("product", product),
            ("constitution", constitution),
            ("rules_json", &rules_json),
        ],
    )
}

/// Keys that only a governance verdict object carries. A JSON object quoted in
/// the review prose is very unlikely to hold one.
const GOVERNANCE_VERDICT_KEYS: [&str; 5] = [
    "status",
    "verdict",
    "violations",
    "rule_violations",
    "failed_actions",
];

/// Weaker verdict signals, consulted only when no object carries a strong key.
const GOVERNANCE_VERDICT_FALLBACK_KEYS: [&str; 5] =
    ["ok", "result", "failed", "violates", "violation"];

const GOVERNANCE_VERDICT_UNPARSABLE: &str = "Governance verdict could not be parsed: the review did not return the required JSON verdict object.";

fn parse_governance_provider_output(output: &str, rules_checked: usize) -> GovernanceEvaluation {
    let trimmed = output.trim();
    match parse_governance_verdict(trimmed) {
        Some(value) => governance_evaluation_from_json(value, trimmed, rules_checked),
        // No verdict means the review could not be read, which is a parsing
        // failure and not a rule violation. Guessing pass/fail from the prose
        // scores explanatory text ("no rule violation here") as a violation, so
        // fail closed and say plainly that the verdict was unreadable.
        None => unparsable_governance_evaluation(trimmed, rules_checked),
    }
}

fn unparsable_governance_evaluation(
    raw_output: &str,
    rules_checked: usize,
) -> GovernanceEvaluation {
    GovernanceEvaluation {
        failed: true,
        message: Some(GOVERNANCE_VERDICT_UNPARSABLE.to_string()),
        details: json_object(json!({
            "phase": "post_implementation",
            "configured": true,
            "rules_checked": rules_checked,
            "verdict_parse_error": true,
            "failed_actions": [{
                "action": "verdict_parse_error",
                "message": GOVERNANCE_VERDICT_UNPARSABLE
            }],
            "raw_output": raw_output
        })),
    }
}

/// Find the review's verdict, not merely the first brace in its prose. Reviews
/// routinely quote code and JSON while reasoning about rules, so scan every
/// balanced object and take the last one that actually looks like a verdict.
fn parse_governance_verdict(raw: &str) -> Option<Value> {
    let candidates = json_object_candidates(raw);
    candidates
        .iter()
        .rev()
        .find(|value| has_any_key(value, &GOVERNANCE_VERDICT_KEYS))
        .or_else(|| {
            candidates
                .iter()
                .rev()
                .find(|value| has_any_key(value, &GOVERNANCE_VERDICT_FALLBACK_KEYS))
        })
        .cloned()
}

fn has_any_key(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|object| keys.iter().any(|key| object.contains_key(*key)))
}

fn governance_evaluation_from_json(
    value: Value,
    raw_output: &str,
    rules_checked: usize,
) -> GovernanceEvaluation {
    let violations = value
        .get("violations")
        .or_else(|| value.get("rule_violations"))
        .or_else(|| value.get("failed_actions"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let status = value
        .get("status")
        .or_else(|| value.get("verdict"))
        .or_else(|| value.get("result"))
        .and_then(Value::as_str)
        .map(|status| status.trim().to_ascii_lowercase());
    let ok = value.get("ok").and_then(Value::as_bool);
    let explicit_failed = value
        .get("failed")
        .or_else(|| value.get("violates"))
        .or_else(|| value.get("violation"))
        .and_then(Value::as_bool);
    let failed = explicit_failed
        .or_else(|| ok.map(|ok| !ok))
        .or_else(|| {
            status.as_ref().map(|status| {
                matches!(
                    status.as_str(),
                    "failed" | "fail" | "blocked" | "violated" | "violation"
                )
            })
        })
        .unwrap_or(!violations.is_empty());
    let provider_message = value
        .get("message")
        .or_else(|| value.get("reason"))
        .or_else(|| value.get("summary"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(ToString::to_string);
    let message = if failed {
        Some(provider_message.unwrap_or_else(|| violation_message_from_actions(&violations)))
    } else {
        provider_message
    };
    GovernanceEvaluation {
        failed,
        message,
        details: json_object(json!({
            "phase": "post_implementation",
            "configured": true,
            "rules_checked": rules_checked,
            "failed_actions": violations,
            "raw_output": raw_output,
            "verdict": value
        })),
    }
}

/// Every top-level balanced object in `raw` that parses as JSON, in the order
/// they appear. An opening brace that never balances (unclosed code in prose)
/// only costs the scan that brace: it resumes right after it instead of
/// swallowing the rest of the text.
fn json_object_candidates(raw: &str) -> Vec<Value> {
    let mut candidates = Vec::new();
    let mut search_from = 0usize;
    while let Some(relative_start) = raw[search_from..].find('{') {
        let start = search_from + relative_start;
        if let Some(end) = balanced_object_end(raw, start)
            && let Ok(value) = serde_json::from_str::<Value>(&raw[start..=end])
        {
            candidates.push(value);
            search_from = end + 1;
            continue;
        }
        search_from = start + 1;
    }
    candidates
}

/// Byte index of the `}` closing the object that opens at `start`.
fn balanced_object_end(raw: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in raw[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn violation_message_from_actions(actions: &[Value]) -> String {
    actions
        .iter()
        .find_map(|action| {
            action
                .get("message")
                .or_else(|| action.get("reason"))
                .or_else(|| action.get("text"))
                .or_else(|| action.get("rule"))
                .and_then(Value::as_str)
                .map(governance_violation_message)
        })
        .unwrap_or_else(|| "Governance rule violation detected.".to_string())
}

fn governance_violation_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        "Governance rule violation detected.".to_string()
    } else if message
        .to_ascii_lowercase()
        .contains("governance rule violation")
    {
        message.to_string()
    } else {
        format!("Governance rule violation: {message}")
    }
}

fn goal_agent_prompt(goal_id: &str, agent_context: &Value) -> RefineResult<String> {
    let goal_context = agent_context.get("goal").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned Goal context"))
    })?;
    let previous_rounds = agent_context.get("previous_rounds").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned previous Rounds"))
    })?;
    let current_round = agent_context.get("current_round").ok_or_else(|| {
        RefineError::Serialization(format!("Goal {goal_id} has no pinned current Round"))
    })?;
    let goal_context = serde_json::to_string_pretty(&goal_context).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Goal {goal_id} context: {error}"))
    })?;
    let previous_rounds = serde_json::to_string_pretty(&previous_rounds).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode Goal {goal_id} previous-round context: {error}"
        ))
    })?;
    let current_round = serde_json::to_string_pretty(&current_round).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode Goal {goal_id} current-round context: {error}"
        ))
    })?;
    let agent_context = serde_json::to_string_pretty(agent_context).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to encode Goal {goal_id} agent context: {error}"
        ))
    })?;

    Ok(render(
        PromptTemplate::GoalAgent,
        &[
            ("goal_id", goal_id),
            ("agent_context", &agent_context),
            ("goal_context", &goal_context),
            ("previous_rounds", &previous_rounds),
            ("latest_round", &current_round),
        ],
    ))
}

fn round_agent_context(round: &Value, round_idx: usize) -> Value {
    let mut context = selected_agent_context(
        round,
        &[
            "reporter",
            "assignee",
            "prompt",
            "guidance_decision",
            "implementation_report",
            "implementation_reported_at",
            "rule_state",
            "meta_rule_state",
            "product_state",
            "constitution_state",
            "governance_message",
            "governance_details",
            "governance_checked_at",
            "governance_rule_actions",
            "quality_state",
            "quality_message",
            "quality_details",
            "quality_checked_at",
        ],
    );
    if let Some(context) = context.as_object_mut() {
        context.insert("round".to_string(), Value::from(round_idx + 1));
    }
    context
}

fn selected_agent_context(value: &Value, keys: &[&str]) -> Value {
    let mut context = JsonObject::new();
    let Some(source) = value.as_object() else {
        return Value::Object(context);
    };
    for key in keys {
        let Some(value) = source.get(*key) else {
            continue;
        };
        if agent_context_value_is_meaningful(value) {
            context.insert((*key).to_string(), value.clone());
        }
    }
    Value::Object(context)
}

fn agent_context_value_is_meaningful(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty() && value != "unclassified",
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

fn json_object(value: serde_json::Value) -> JsonObject {
    value.as_object().cloned().unwrap_or_default()
}

fn default_node_id() -> String {
    "default".to_string()
}

fn default_provider() -> String {
    "claude".to_string()
}

fn default_target_app_id() -> String {
    "default".to_string()
}

fn priority_rank(priority: &GoalPriority) -> u8 {
    match priority {
        GoalPriority::Low => 0,
        GoalPriority::Medium => 1,
        GoalPriority::High => 2,
    }
}

fn new_claim_id() -> String {
    format!("res-{}", Uuid::new_v4())
}

fn new_execution_id() -> String {
    format!("exec-{}", Uuid::new_v4())
}

fn missing_workflow_artifact(name: &str, goal_id: &str) -> RefineError {
    RefineError::Conflict(format!(
        "workflow artifact {name} is missing for Goal {goal_id}"
    ))
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

mod automation;
mod execution;
mod policy;
mod ready_merge;
mod state;
#[cfg(test)]
mod tests;

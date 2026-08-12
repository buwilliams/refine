mod dashboard;
mod nodes_fleet;
mod projects;
mod settings_governance;
mod target_app;
mod todos;
use crate::process::supervisor::config::{
    ConfigService, FileGovernanceService, FileGuidanceService, FileReporterService,
    FileSettingsService,
};
use std::collections::BTreeMap;
use std::thread;

use chrono::Utc;
use serde_json::{Value, json};

use crate::model::workflow::GoalStatus;
use crate::process::runner::FileRunnerWorkerService;
use crate::process::subprocess::{FileProcessSupervisor, ProcessOwner, ProcessSupervisor};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{current_launch_executable, current_launch_mode};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::agent_providers::{AgentProviderService, ProviderInvocation};
use crate::tools::host::fleet::{FileFleetService, FleetService, NodeRemoteUpdate};
use crate::tools::host::target_apps::TargetAppGeneratedConfig;
use crate::tools::product::next_actions::FileNextActionsService;
use crate::tools::product::nodes::{FileNodeRegistryService, NodeUpdate, detached_nodes_response};
use crate::tools::product::project_registry::{ProjectRegistryService, registry_apps_array};
use crate::tools::product::project_state::{DashboardProjectionQuery, ProjectionQuery};
use crate::tools::product::todos::FileTodoService;
use crate::tools::product::work_items::BulkGoalSelection;
use crate::tools::product::worktree_cleanup::{FileWorktreeCleanupService, WorktreeCleanupOptions};
use crate::workflow::WorkflowEngine;

use super::support::*;
use super::*;

fn configured_provider_from_settings(
    refine_dir: &std::path::Path,
    active_root: Option<&std::path::Path>,
    body: &Value,
) -> String {
    body.get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(str::to_string)
        .or_else(|| {
            let service = match active_root {
                Some(active_root) => FileSettingsService::with_active_root(refine_dir, active_root),
                None => FileSettingsService::new(refine_dir),
            };
            service.load().ok().and_then(|settings| {
                settings
                    .get("agent_cli")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|provider| !provider.is_empty())
                    .map(str::to_string)
            })
        })
        .or_else(|| {
            provider_status_value().ok().and_then(|status| {
                status
                    .get("selected_provider")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|provider| !provider.is_empty())
                    .map(str::to_string)
            })
        })
        .unwrap_or_else(|| "claude".to_string())
}

pub(super) fn dashboard_attention_items(
    indicators: &[String],
    preparation_failures: &[crate::workflow::WorkflowClaim],
    retry_delays: &[crate::workflow::WorkflowRetryDelay],
    runner_reachable: bool,
) -> Vec<Value> {
    let mut items = indicators
        .iter()
        .map(|message| {
            json!({
                "kind": "filter",
                "severity": "warn",
                "message": message,
                "filter": {
                    "status": "failed"
                }
            })
        })
        .collect::<Vec<_>>();
    items.extend(preparation_failures.iter().map(|claim| {
        let reason = claim
            .failure_message
            .as_deref()
            .unwrap_or("claim preparation failed");
        json!({
            "kind": "filter",
            "severity": "error",
            "message": format!(
                "Goal {} needs attention: {}",
                claim.goal_id, reason
            ),
            "filter": {
                "status": "failed"
            },
            "goal_id": claim.goal_id,
            "claim_id": claim.claim_id,
            "reason": reason
        })
    }));
    items.extend(retry_delays.iter().map(|delay| {
        json!({
            "kind": "filter",
            "severity": "warn",
            "message": format!(
                "Goal {} will be eligible for workflow retry at {}",
                delay.goal_id, delay.retry_not_before
            ),
            "filter": {
                "status": "todo"
            },
            "goal_id": delay.goal_id,
            "claim_id": delay.claim_id,
            "retry_not_before": delay.retry_not_before,
            "reason": delay.failure_message
        })
    }));
    if !runner_reachable {
        items.push(json!({
            "kind": "banner",
            "severity": "error",
            "message": "Refine cannot reach the runtime worker. Re-check auth after restoring provider access."
        }));
    }
    items
}

fn dashboard_active_node(
    service: &FileNodeRegistryService,
) -> RefineResult<crate::tools::product::nodes::ActiveNodeIdentity> {
    service.active_identity()
}

fn governance_generation_prompt(product: &str, constitution: &str) -> String {
    render(
        PromptTemplate::GovernanceGeneration,
        &[("product", product), ("constitution", constitution)],
    )
}

fn target_app_generation_prompt(target_root: &std::path::Path) -> String {
    let target_root = target_root.display().to_string();
    render(
        PromptTemplate::TargetAppGeneration,
        &[("target_root", &target_root)],
    )
}

fn target_config_string(value: &Value, key: &str, fallback: &str) -> String {
    let legacy_key = match key {
        "build_command" => Some("rebuild_command"),
        _ => None,
    };
    value
        .get(key)
        .or_else(|| legacy_key.and_then(|key| value.get(key)))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .trim()
        .to_string()
}

fn target_config_u64(value: &Value, key: &str, fallback: u64) -> u64 {
    let legacy_key = match key {
        "build_timeout_seconds" => Some("rebuild_timeout_seconds"),
        _ => None,
    };
    value
        .get(key)
        .or_else(|| legacy_key.and_then(|key| value.get(key)))
        .and_then(Value::as_u64)
        .or_else(|| {
            value
                .get(key)
                .and_then(Value::as_str)
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
        .unwrap_or(fallback)
}

fn parse_generated_target_app_config(output: &str) -> Option<TargetAppGeneratedConfig> {
    let value = serde_json::from_str::<Value>(output).ok()?;
    let cfg = value.get("config").unwrap_or(&value);
    let env = cfg
        .get("env")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let start_instructions = first_non_empty(
        &target_config_string(cfg, "start_instructions", ""),
        &target_config_string(cfg, "start_command", ""),
    );
    let stop_instructions = first_non_empty(
        &target_config_string(cfg, "stop_instructions", ""),
        &target_config_string(cfg, "stop_command", ""),
    );
    let build_instructions = first_non_empty(
        &target_config_string(cfg, "build_instructions", ""),
        &first_non_empty(
            &target_config_string(cfg, "rebuild_instructions", ""),
            &target_config_string(cfg, "build_command", ""),
        ),
    );
    let test_command = target_config_string(cfg, "test_command", "");
    let status_command = target_config_string(cfg, "status_command", "");
    if start_instructions.is_empty()
        && build_instructions.is_empty()
        && test_command.is_empty()
        && status_command.is_empty()
    {
        return None;
    }
    Some(TargetAppGeneratedConfig {
        start_instructions,
        stop_instructions,
        build_instructions,
        start_command: String::new(),
        stop_command: String::new(),
        build_command: String::new(),
        test_command,
        status_command,
        cwd: target_config_string(cfg, "cwd", "."),
        env,
        start_timeout_seconds: target_config_u64(cfg, "start_timeout_seconds", 120),
        stop_timeout_seconds: target_config_u64(cfg, "stop_timeout_seconds", 60),
        build_timeout_seconds: target_config_u64(cfg, "build_timeout_seconds", 300),
        test_timeout_seconds: target_config_u64(cfg, "test_timeout_seconds", 600),
        status_timeout_seconds: target_config_u64(cfg, "status_timeout_seconds", 10),
        log_path: target_config_string(cfg, "log_path", ""),
        http_check_url: target_config_string(cfg, "http_check_url", ""),
        tcp_check_host: target_config_string(cfg, "tcp_check_host", ""),
        tcp_check_port: target_config_string(cfg, "tcp_check_port", ""),
        process_check_command: target_config_string(cfg, "process_check_command", ""),
        notes: target_config_string(cfg, "notes", ""),
    })
}

fn target_app_generated_settings(config: &TargetAppGeneratedConfig) -> Value {
    json!({
        "target_app_start_instructions": config.start_instructions.clone(),
        "target_app_stop_instructions": config.stop_instructions.clone(),
        "target_app_build_instructions": config.build_instructions.clone(),
        "target_app_start_command": config.start_command.clone(),
        "target_app_stop_command": config.stop_command.clone(),
        "target_app_build_command": config.build_command.clone(),
        "target_app_test_command": config.test_command.clone(),
        "target_app_test_commands": if config.test_command.trim().is_empty() {
            String::new()
        } else {
            json!([{"command": config.test_command.clone(), "enabled": true}]).to_string()
        },
        "target_app_status_command": config.status_command.clone(),
        "target_app_cwd": config.cwd.clone(),
        "target_app_env_json": serde_json::to_string_pretty(&config.env).unwrap_or_else(|_| "{}".to_string()),
        "target_app_start_timeout_seconds": config.start_timeout_seconds.to_string(),
        "target_app_stop_timeout_seconds": config.stop_timeout_seconds.to_string(),
        "target_app_build_timeout_seconds": config.build_timeout_seconds.to_string(),
        "target_app_test_timeout_seconds": config.test_timeout_seconds.to_string(),
        "target_app_status_timeout_seconds": config.status_timeout_seconds.to_string(),
        "target_app_log_path": config.log_path.clone(),
        "target_app_http_check_url": config.http_check_url.clone(),
        "target_app_tcp_check_host": config.tcp_check_host.clone(),
        "target_app_tcp_check_port": config.tcp_check_port.clone(),
        "target_app_process_check_command": config.process_check_command.clone()
    })
}

fn generated_governance_rule(text: &str, index: usize) -> Value {
    let timestamp = Utc::now().to_rfc3339();
    json!({
        "id": format!("generated-rule-{}-{index}", Utc::now().timestamp_millis()),
        "text": text.chars().take(500).collect::<String>(),
        "created": timestamp,
        "updated": timestamp,
        "source": "generated"
    })
}

fn parse_generated_governance_rules(output: &str) -> Vec<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        let rules = value
            .get("rules")
            .or_else(|| value.get("items"))
            .unwrap_or(&value);
        if let Some(items) = rules.as_array() {
            let parsed = items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let text = item
                        .get("text")
                        .or_else(|| item.get("rule"))
                        .and_then(Value::as_str)
                        .or_else(|| item.as_str())?
                        .trim();
                    (!text.is_empty()).then(|| generated_governance_rule(text, index + 1))
                })
                .collect::<Vec<_>>();
            if !parsed.is_empty() {
                return parsed;
            }
        }
    }

    output
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(|ch: char| {
                    ch == '-' || ch == '*' || ch.is_ascii_digit() || ch == '.'
                })
                .trim()
        })
        .filter(|line| !line.is_empty())
        .enumerate()
        .map(|(index, line)| generated_governance_rule(line, index + 1))
        .collect()
}

impl InProcessWebServer {}

fn body_string<'a>(body: &'a Value, key: &str) -> &'a str {
    body.get(key).and_then(Value::as_str).unwrap_or("")
}

fn todo_list_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("/todos/lists/")
        .filter(|id| !id.is_empty() && !id.contains('/'))
}

fn todo_item_collection_list_id(path: &str) -> Option<&str> {
    path.strip_prefix("/todos/lists/")
        .and_then(|path| path.strip_suffix("/items"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
}

fn todo_item_ids_from_path(path: &str) -> Option<(&str, &str)> {
    let path = path.strip_prefix("/todos/lists/")?;
    let (list_id, item_id) = path.split_once("/items/")?;
    (!list_id.is_empty() && !item_id.is_empty() && !item_id.contains('/'))
        .then_some((list_id, item_id))
}

fn todo_route_not_found() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({
            "error": {
                "code": "not_found",
                "message": "Todo route requires valid list and item ids"
            }
        }),
    )
}

fn assignee_stats_rows(
    assignee_stats: &BTreeMap<String, BTreeMap<GoalStatus, usize>>,
) -> Vec<Value> {
    assignee_stats
        .iter()
        .filter(|(assignee, _)| assignee.as_str() != "unassigned")
        .map(|(assignee, counts)| {
            let assigned = counts.values().copied().sum::<usize>();
            let done = counts.get(&GoalStatus::Done).copied().unwrap_or_default();
            let cancelled = counts
                .get(&GoalStatus::Cancelled)
                .copied()
                .unwrap_or_default();
            let active = assigned.saturating_sub(done + cancelled);
            let assigned_review = counts.get(&GoalStatus::Review).copied().unwrap_or_default();
            let completion_rate = if assigned == 0 {
                0.0
            } else {
                (done as f64 / assigned as f64) * 100.0
            };
            json!({
                "assignee": assignee,
                "reporter": assignee,
                "active": active,
                "done": done,
                "reported": assigned,
                "assigned": assigned,
                "assigned_review": assigned_review,
                "completion_rate": completion_rate
            })
        })
        .collect()
}

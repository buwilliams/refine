mod dashboard;
mod nodes_fleet;
mod projects;
mod settings_governance;
mod target_app;
mod todos;
use crate::infrastructure::process::supervisor::config::{
    ConfigService, FileGovernanceService, FileGuidanceService, FileReporterService,
    FileSettingsService,
};
use std::collections::BTreeMap;
use std::thread;

use chrono::Utc;
use serde_json::{Value, json};

use crate::application::agent_io::prompts::{PromptTemplate, render};
use crate::application::fleet::nodes::{
    FileNodeRegistryService, NodeUpdate, detached_nodes_response,
};
use crate::application::fleet::service::{FileFleetService, FleetService, NodeRemoteUpdate};
use crate::application::guidance::FileNextActionsService;
use crate::application::maintenance::worktrees::{
    FileWorktreeCleanupService, WorktreeCleanupOptions,
};
use crate::application::projects::projection::{DashboardProjectionQuery, ProjectionQuery};
use crate::application::projects::registry::{ProjectRegistryService, registry_apps_array};
use crate::application::system::target_apps::TargetAppGeneratedConfig;
use crate::application::todos::FileTodoService;
use crate::application::work_items::BulkGoalSelection;
use crate::application::workers::FileRunnerWorkerService;
use crate::application::workflow::WorkflowEngine;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::{AgentProviderService, ProviderInvocation};
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, ProcessOwner, ProcessSupervisor,
};
use crate::infrastructure::process::supervisor::lifecycle::current_launch_executable;
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::model::workflow::GoalStatus;

use super::support::*;
use super::*;

pub(in crate::surfaces::web_server) fn configured_provider_from_settings(
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
    runner_reachable: bool,
    workflow_paused: bool,
    state_sync_health: Option<&crate::application::persistence_sync::health::StateSyncHealth>,
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
    if let Some(health) = state_sync_health {
        if health.status == "failed" {
            let since = health.failure_since.as_deref().unwrap_or("an unknown time");
            items.push(json!({
                "kind": "banner",
                "severity": "error",
                "message": format!("State sync has failed since {since}. All-node counts are local and non-authoritative until synchronization recovers.")
            }));
        } else if health.status == "stale" {
            let since = health.stale_since.as_deref().unwrap_or("an unknown time");
            items.push(json!({
                "kind": "banner",
                "severity": "warn",
                "message": format!("State sync is stale since {since}. All-node counts are local and non-authoritative.")
            }));
        }
    }
    if workflow_paused {
        items.push(json!({
            "kind": "banner",
            "severity": "info",
            "message": "Workflow is paused."
        }));
    } else if !runner_reachable {
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
) -> RefineResult<crate::application::fleet::nodes::ActiveNodeIdentity> {
    service.active_identity()
}

/// The canonical generated-rules shape shown to the agent. The parser also
/// accepts `items`, bare arrays, `{rule}` objects, bare strings, and plain
/// text lines.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct GeneratedGovernanceRulesContract {
    rules: Vec<String>,
}

impl crate::application::agent_io::structured_output::Contract
    for GeneratedGovernanceRulesContract
{
    const LABEL: &'static str = "generated Governance rules JSON";

    fn example() -> Self {
        GeneratedGovernanceRulesContract {
            rules: vec!["one concise rule".to_string()],
        }
    }
}

/// The canonical target-app config shape shown to the agent. The parser also
/// accepts legacy command spellings and string-typed timeouts.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct TargetAppConfigContract {
    start_instructions: String,
    stop_instructions: String,
    build_instructions: String,
    test_command: String,
    status_command: String,
    cwd: String,
    env: serde_json::Map<String, Value>,
    start_timeout_seconds: u64,
    stop_timeout_seconds: u64,
    build_timeout_seconds: u64,
    test_timeout_seconds: u64,
    status_timeout_seconds: u64,
    log_path: String,
    http_check_url: String,
    tcp_check_host: String,
    tcp_check_port: String,
    process_check_command: String,
    notes: String,
}

impl crate::application::agent_io::structured_output::Contract for TargetAppConfigContract {
    const LABEL: &'static str = "target-app config JSON";

    fn example() -> Self {
        TargetAppConfigContract {
            start_instructions: "...".to_string(),
            stop_instructions: "...".to_string(),
            build_instructions: "...".to_string(),
            test_command: "...".to_string(),
            status_command: "...".to_string(),
            cwd: ".".to_string(),
            env: serde_json::Map::new(),
            start_timeout_seconds: 120,
            stop_timeout_seconds: 60,
            build_timeout_seconds: 300,
            test_timeout_seconds: 600,
            status_timeout_seconds: 10,
            log_path: "".to_string(),
            http_check_url: "".to_string(),
            tcp_check_host: "".to_string(),
            tcp_check_port: "".to_string(),
            process_check_command: "".to_string(),
            notes: "".to_string(),
        }
    }
}

fn governance_generation_prompt(product: &str, constitution: &str) -> String {
    let rules_contract =
        <GeneratedGovernanceRulesContract as crate::application::agent_io::structured_output::Contract>::contract_json();
    render(
        PromptTemplate::GovernanceGeneration,
        &[
            ("product", product),
            ("constitution", constitution),
            ("rules_contract", &rules_contract),
        ],
    )
}

fn target_app_generation_prompt(target_root: &std::path::Path) -> String {
    let target_root = target_root.display().to_string();
    let target_app_contract =
        <TargetAppConfigContract as crate::application::agent_io::structured_output::Contract>::contract_json();
    render(
        PromptTemplate::TargetAppGeneration,
        &[
            ("target_root", &target_root),
            ("target_app_contract", &target_app_contract),
        ],
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
    let value = crate::application::agent_io::structured_output::json_candidates(
        output,
        &crate::application::agent_io::structured_output::DecodeOptions::new(
            <TargetAppConfigContract as crate::application::agent_io::structured_output::Contract>::LABEL,
        ),
    )
    .into_iter()
    .next()?;
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
    for value in crate::application::agent_io::structured_output::json_candidates(
        output,
        &crate::application::agent_io::structured_output::DecodeOptions::new(
            <GeneratedGovernanceRulesContract as crate::application::agent_io::structured_output::Contract>::LABEL,
        ),
    ) {
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

fn guidance_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("/guidance/")
        .filter(|id| !id.is_empty() && !id.contains('/'))
}

fn guidance_id_required() -> ApiResponse {
    ApiResponse::json(
        404,
        json!({"error": {"code": "not_found", "message": "Guidance route requires a valid entry id"}}),
    )
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

/// Ranks contributors by delivered volume weighted by delivery rate.
///
/// A plain completion percentage made two-for-two look better than forty
/// delivered out of a hundred twenty filed, and a plain count rewarded filing
/// volume regardless of what shipped. The score multiplies the two:
/// `delivered × (delivered / reported)` — shipping more raises it linearly,
/// and the share of your filed work that ships scales it. Cancelled Goals are
/// withdrawn work, so they count toward neither side. Ties fall back to
/// delivered volume, then reported volume, then name.
fn contributor_ranking_rows(
    reporter_stats: &BTreeMap<String, BTreeMap<GoalStatus, usize>>,
) -> Vec<Value> {
    let mut rows = reporter_stats
        .iter()
        .filter(|(reporter, _)| reporter.as_str() != "unknown")
        .map(|(reporter, counts)| {
            let cancelled = counts
                .get(&GoalStatus::Cancelled)
                .copied()
                .unwrap_or_default();
            let reported = counts
                .values()
                .copied()
                .sum::<usize>()
                .saturating_sub(cancelled);
            let delivered = counts.get(&GoalStatus::Done).copied().unwrap_or_default();
            let delivery_rate = if reported == 0 {
                0.0
            } else {
                delivered as f64 / reported as f64
            };
            let score = delivered as f64 * delivery_rate;
            (reporter.clone(), reported, delivered, delivery_rate, score)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.4.partial_cmp(&a.4)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| a.0.cmp(&b.0))
    });
    rows.into_iter()
        .enumerate()
        .map(
            |(index, (reporter, reported, delivered, delivery_rate, score))| {
                json!({
                    "rank": index + 1,
                    "contributor": reporter,
                    "reported": reported,
                    "delivered": delivered,
                    "delivery_rate": delivery_rate * 100.0,
                    "score": score,
                })
            },
        )
        .collect()
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

#[cfg(test)]
mod contributor_ranking_tests {
    use super::*;

    fn counts(pairs: &[(GoalStatus, usize)]) -> BTreeMap<GoalStatus, usize> {
        pairs.iter().cloned().collect()
    }

    #[test]
    fn volume_outranks_a_perfect_but_tiny_record() {
        let mut stats = BTreeMap::new();
        // Two filed, two delivered: perfect rate, little contribution.
        stats.insert("reporter-a".to_string(), counts(&[(GoalStatus::Done, 2)]));
        // 120 filed, 40 delivered: far more shipped work despite the lower rate.
        stats.insert(
            "reporter-b".to_string(),
            counts(&[
                (GoalStatus::Done, 40),
                (GoalStatus::Backlog, 60),
                (GoalStatus::Failed, 20),
            ]),
        );
        // Cancelled work is withdrawn: it must not dilute the rate.
        stats.insert(
            "reporter-c".to_string(),
            counts(&[(GoalStatus::Done, 10), (GoalStatus::Cancelled, 90)]),
        );
        stats.insert("unknown".to_string(), counts(&[(GoalStatus::Done, 99)]));

        let rows = contributor_ranking_rows(&stats);

        let order: Vec<&str> = rows
            .iter()
            .map(|row| row["contributor"].as_str().unwrap())
            .collect();
        assert_eq!(order, vec!["reporter-b", "reporter-c", "reporter-a"]);
        assert_eq!(rows[0]["rank"], 1);
        assert_eq!(rows[0]["reported"], 120);
        assert_eq!(rows[0]["delivered"], 40);
        assert!((rows[0]["score"].as_f64().unwrap() - 40.0 * (40.0 / 120.0)).abs() < 1e-9);
        // reporter-c's 90 cancelled Goals leave a perfect 10-of-10 record.
        assert_eq!(rows[1]["reported"], 10);
        assert_eq!(rows[1]["delivery_rate"], 100.0);
        assert_eq!(rows[1]["score"], 10.0);
        assert_eq!(rows[2]["score"], 2.0);
    }
}

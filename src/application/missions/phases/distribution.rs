//! Fleet distribution compilation: an approved wave compiled into the
//! existing Goal and Feature placement operations.
//!
//! Distribution uses only enabled, healthy Nodes, preserves active Goal
//! ownership, honors `preferred_node`, treats a scoped ordered Feature as one
//! placement unit, records exclusions with reasons, and revalidates
//! immediately before applying. Node assignment and Todo admission happen as
//! one engine step per Goal or Feature unit; every applied wave leaves a
//! durable distribution receipt in phase evidence. See
//! `docs/mission-spec.md` ("Fleet distribution").

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::application::fleet::nodes::FileNodeRegistryService;
use crate::application::missions::service::FileMissionService;
use crate::application::work_items::FileWorkItemService;
use crate::error::RefineResult;
use crate::model::workflow::GoalStatus;

use super::execution::{approved_wave, mission_bound_goals};

/// One compiled placement decision for a wave Goal.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DistributionAssignment {
    pub mission_goal_key: String,
    pub goal_id: String,
    /// The compiled target node, when placement is possible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_node_id: Option<String>,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The compiled distribution of one approved wave.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DistributionReport {
    pub wave: usize,
    pub eligible_nodes: Vec<String>,
    pub assignments: Vec<DistributionAssignment>,
    pub moved: usize,
    pub skipped: usize,
    pub dry_run: bool,
}

/// Preview the distribution of one approved wave without applying it.
pub fn preview_wave_distribution(
    mission_service: &FileMissionService,
    work_items: &FileWorkItemService,
    mission_id: &str,
    wave: usize,
) -> RefineResult<DistributionReport> {
    compile_distribution(mission_service, work_items, mission_id, wave, true)
}

/// Compile and apply the distribution of one approved wave, then record the
/// durable distribution receipt. Idempotent: a Goal already placed on its
/// target node reports `already-placed` without a write.
pub fn distribute_wave(
    mission_service: &FileMissionService,
    work_items: &FileWorkItemService,
    mission_id: &str,
    wave: usize,
) -> RefineResult<DistributionReport> {
    let report = compile_distribution(mission_service, work_items, mission_id, wave, false)?;
    if !report.dry_run {
        let receipt = json!({
            "wave": report.wave,
            "eligible_nodes": report.eligible_nodes,
            "moved": report.moved,
            "skipped": report.skipped,
            "assignments": report.assignments,
        });
        super::write_wave_phase_evidence(
            mission_service,
            mission_id,
            "distribution",
            wave,
            receipt,
        )?;
    }
    Ok(report)
}

fn compile_distribution(
    mission_service: &FileMissionService,
    work_items: &FileWorkItemService,
    mission_id: &str,
    wave: usize,
    dry_run: bool,
) -> RefineResult<DistributionReport> {
    let mission = mission_service.show_mission(mission_id)?;
    let approved = approved_wave(&mission, wave)?;
    let specs = approved.goal_specs.clone();

    let registry = FileNodeRegistryService::new(&work_items.refine_dir);
    let candidates = registry.distribution_candidate_nodes()?;
    let eligible_nodes: Vec<String> = candidates.iter().map(|node| node.id.clone()).collect();

    // Load counts from Goal state: current placement of every nonterminal
    // Goal, so capacity waits and spread decisions observe real load.
    let mut load: BTreeMap<String, usize> = BTreeMap::new();
    for goal in work_items.list_goal_summaries()? {
        let status = goal.goal.status;
        let terminal = matches!(
            status,
            GoalStatus::Done | GoalStatus::Failed | GoalStatus::Cancelled
        );
        if terminal {
            continue;
        }
        let owner = goal
            .goal
            .node_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        *load.entry(owner).or_default() += 1;
    }

    let bound: BTreeMap<String, String> = mission_bound_goals(&work_items.refine_dir, mission_id)?
        .into_iter()
        .map(|goal| (goal.mission_goal_key, goal.goal_id))
        .collect();
    let bound_ids: std::collections::BTreeSet<String> = bound.values().cloned().collect();

    let mut report = DistributionReport {
        wave,
        eligible_nodes: eligible_nodes.clone(),
        assignments: Vec::new(),
        moved: 0,
        skipped: 0,
        dry_run,
    };
    // A fleet with no eligible node is reported before per-goal reasons: it
    // is the recovery path that changes, not the Goal.
    if eligible_nodes.is_empty() {
        for spec in &specs {
            report.assignments.push(DistributionAssignment {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id: String::new(),
                target_node_id: None,
                applied: false,
                reason: Some("no eligible node".to_string()),
            });
        }
        report.skipped += specs.len();
        return Ok(report);
    }
    let mut placed_features: BTreeMap<String, ()> = BTreeMap::new();
    for spec in &specs {
        let Some(goal_id) = bound.get(&spec.mission_goal_key).cloned() else {
            report.assignments.push(DistributionAssignment {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id: String::new(),
                target_node_id: None,
                applied: false,
                reason: Some("goal was not materialized".to_string()),
            });
            report.skipped += 1;
            continue;
        };
        let summary = work_items.show_goal_summary(&goal_id)?;
        let owner = summary
            .goal
            .node_id
            .clone()
            .unwrap_or_else(|| "default".to_string());

        // Active Goals keep their ownership; pinned work is never moved.
        if matches!(
            summary.goal.status,
            GoalStatus::Plan | GoalStatus::Implement | GoalStatus::Quality | GoalStatus::Governance
        ) {
            report.assignments.push(DistributionAssignment {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id,
                target_node_id: None,
                applied: false,
                reason: Some(format!("active:{}", summary.goal.status.as_str())),
            });
            report.skipped += 1;
            continue;
        }
        if matches!(
            summary.goal.status,
            GoalStatus::Done | GoalStatus::Failed | GoalStatus::Cancelled
        ) {
            report.assignments.push(DistributionAssignment {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id,
                target_node_id: None,
                applied: false,
                reason: Some(format!("terminal:{}", summary.goal.status.as_str())),
            });
            report.skipped += 1;
            continue;
        }

        // Feature-bound Goals move as part of their Feature: one placement
        // unit when the whole Feature is inside the Mission, an explicit
        // mixed-scope exclusion when it is not.
        if let Some(feature_id) = summary.goal.feature_id.clone() {
            if placed_features.contains_key(&feature_id) {
                report.assignments.push(DistributionAssignment {
                    mission_goal_key: spec.mission_goal_key.clone(),
                    goal_id,
                    target_node_id: None,
                    applied: false,
                    reason: Some(format!("feature:{feature_id} placed as one unit")),
                });
                report.skipped += 1;
                continue;
            }
            let feature = work_items.show_feature_summary(&feature_id)?;
            let mixed_scope = feature
                .goal_ids
                .iter()
                .any(|member| !bound_ids.contains(member));
            if mixed_scope {
                report.assignments.push(DistributionAssignment {
                    mission_goal_key: spec.mission_goal_key.clone(),
                    goal_id,
                    target_node_id: None,
                    applied: false,
                    reason: Some(format!("mixed-feature-scope:{feature_id}")),
                });
                report.skipped += 1;
                continue;
            }
            let target = pick_target_node(
                &eligible_nodes,
                spec.preferred_node.as_deref(),
                &load,
                feature.feature.node_id.as_deref(),
                feature.goal_ids.len(),
            )?;
            let feature_owner = feature
                .feature
                .node_id
                .clone()
                .unwrap_or_else(|| "default".to_string());
            let placed = if dry_run || target == feature_owner {
                true
            } else {
                match work_items.transfer_feature_to_node(&target, &feature_id) {
                    Ok(_) => true,
                    Err(error) => {
                        report.assignments.push(DistributionAssignment {
                            mission_goal_key: spec.mission_goal_key.clone(),
                            goal_id,
                            target_node_id: Some(target),
                            applied: false,
                            reason: Some(format!("feature-transfer-failed: {error}")),
                        });
                        report.skipped += 1;
                        continue;
                    }
                }
            };
            if placed {
                placed_features.insert(feature_id, ());
                *load.entry(target.clone()).or_default() += feature.goal_ids.len();
            }
            report.assignments.push(DistributionAssignment {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id,
                target_node_id: Some(target),
                applied: placed,
                reason: placed.then(|| "feature-unit".to_string()),
            });
            if !placed {
                report.skipped += 1;
            }
            continue;
        }

        let target = pick_target_node(
            &eligible_nodes,
            spec.preferred_node.as_deref(),
            &load,
            Some(owner.as_str()),
            1,
        )?;
        if target == owner {
            report.assignments.push(DistributionAssignment {
                mission_goal_key: spec.mission_goal_key.clone(),
                goal_id,
                target_node_id: Some(target),
                applied: true,
                reason: Some("already-placed".to_string()),
            });
            continue;
        }
        let (applied, failure) = if dry_run {
            (false, None)
        } else {
            match work_items.transfer_goal_to_node(&target, &goal_id) {
                Ok(_) => (true, None),
                Err(error) => (false, Some(format!("transfer-failed: {error}"))),
            }
        };
        *load.entry(target.clone()).or_default() += 1;
        if applied {
            report.moved += 1;
        } else {
            report.skipped += 1;
        }
        report.assignments.push(DistributionAssignment {
            mission_goal_key: spec.mission_goal_key.clone(),
            goal_id,
            target_node_id: Some(target),
            applied,
            reason: applied
                .then(|| "moved".to_string())
                .or(failure)
                .or_else(|| Some("preview".to_string())),
        });
    }
    Ok(report)
}

/// Resolve the target node for one unit: the approved preferred node when it
/// is eligible, else the least-loaded eligible node (capacity-driven
/// placement within approved constraints is not a material amendment). The
/// unit's own current load is excluded from its owner so placement is stable
/// and idempotent: a queued Goal only moves when another node is genuinely
/// less loaded.
fn pick_target_node(
    eligible_nodes: &[String],
    preferred: Option<&str>,
    load: &BTreeMap<String, usize>,
    current_owner: Option<&str>,
    unit_size: usize,
) -> RefineResult<String> {
    if let Some(preferred) = preferred
        .map(str::trim)
        .filter(|preferred| !preferred.is_empty())
        && eligible_nodes.iter().any(|node| node == preferred)
    {
        return Ok(preferred.to_string());
    }
    let target = eligible_nodes
        .iter()
        .min_by_key(|node| {
            let effective = load.get(*node).copied().unwrap_or(0).saturating_sub(
                if Some(node.as_str()) == current_owner {
                    unit_size
                } else {
                    0
                },
            );
            (effective, node.to_string())
        })
        .expect("eligible nodes are nonempty when picked");
    Ok(target.clone())
}

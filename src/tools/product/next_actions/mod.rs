use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::RefineResult;
use crate::tools::product::nodes::FileNodeRegistryService;
use crate::tools::product::work_items::FileWorkItemService;
use crate::workflow::{
    WORKFLOW_AUTOMATION_STATE_FILE, WorkflowAutomationState, WorkflowClaimState,
};

/// The `refine next` oracle: reads durable state and recommends the next
/// operations, each with the exact command to run. No scheduler and no side
/// effects — an inspectable recommendation derived from flat files, equally
/// useful to a person getting oriented and to an agent planning its next
/// tool call.
#[derive(Clone, Debug)]
pub struct FileNextActionsService {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

impl FileNextActionsService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: None,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: Some(runtime_root.into()),
        }
    }

    pub fn next_response(&self) -> RefineResult<serde_json::Value> {
        let nodes_service = match &self.runtime_root {
            Some(runtime_root) => {
                FileNodeRegistryService::with_active_root(&self.refine_dir, runtime_root)
            }
            None => FileNodeRegistryService::new(&self.refine_dir),
        };
        let registry = nodes_service.load_registry()?;
        let active_node_id = nodes_service
            .active_node_id()
            .unwrap_or_else(|_| "default".to_string());
        let goals = FileWorkItemService::new(&self.refine_dir).list_goal_summaries()?;
        let claimed = self.active_claim_count();

        let mut status_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut open_by_node: BTreeMap<String, usize> = BTreeMap::new();
        let mut stranded_review = Vec::new();
        for goal in &goals {
            status_counts
                .entry(goal.goal.status.as_str())
                .and_modify(|count| *count += 1)
                .or_insert(1);
            let owner = goal
                .goal
                .node_id
                .clone()
                .unwrap_or_else(|| "default".to_string());
            if matches!(goal.goal.status, GoalStatus::Backlog | GoalStatus::Todo) {
                *open_by_node.entry(owner.clone()).or_insert(0) += 1;
            }
            if goal.goal.status == GoalStatus::Review && owner != active_node_id {
                stranded_review.push(goal.goal.id.clone());
            }
        }

        let enabled_nodes: Vec<_> = registry
            .nodes
            .iter()
            .filter(|node| node.enabled && !node.archived)
            .collect();
        let failed: Vec<String> = enabled_nodes
            .iter()
            .filter(|node| {
                node.health
                    .as_ref()
                    .is_some_and(|health| health.status == "failed")
            })
            .map(|node| node.id.clone())
            .collect();
        let healthy_node_count = enabled_nodes
            .iter()
            .filter(|node| {
                node.health
                    .as_ref()
                    .map(|health| health.status != "failed" && health.status != "deprovisioned")
                    .unwrap_or(true)
            })
            .count();

        let mut suggestions = Vec::new();
        if goals.is_empty() {
            suggest(
                &mut suggestions,
                "capture-first-goal",
                "No work is tracked yet. Capture the first goal between what the app does and what it should do.",
                "refine goal create \"<what should change>\"",
            );
        }
        for node_id in &failed {
            suggest(
                &mut suggestions,
                "inspect-failed-node",
                &format!(
                    "Node {node_id} last reported failed health; inspect before sending work to it."
                ),
                &format!("refine cluster show {node_id}"),
            );
        }
        if !stranded_review.is_empty() {
            suggest(
                &mut suggestions,
                "converge-reviewables",
                &format!(
                    "{} reviewable goal(s) sit on other nodes; converge them to this node for review.",
                    stranded_review.len()
                ),
                &format!("refine cluster distribute --converge --to {active_node_id}"),
            );
        }
        let open_total: usize = open_by_node.values().sum();
        if healthy_node_count > 1 && open_total > 0 && open_by_node.len() == 1 {
            suggest(
                &mut suggestions,
                "distribute-work",
                &format!(
                    "{open_total} open goal(s) all sit on one node while {healthy_node_count} nodes are available."
                ),
                "refine cluster distribute --dry-run",
            );
        }
        if let Some(review_count) = status_counts.get("review")
            && stranded_review.len() < *review_count
        {
            suggest(
                &mut suggestions,
                "review-work",
                &format!("{review_count} goal(s) are waiting on human review."),
                "refine goal list",
            );
        }
        if suggestions.is_empty() {
            suggest(
                &mut suggestions,
                "all-quiet",
                "Nothing needs attention: work and fleet are in steady state.",
                "refine system status",
            );
        }

        Ok(serde_json::json!({
            "ok": true,
            "active_node_id": active_node_id,
            "state": {
                "goals_by_status": status_counts,
                "open_goals_by_node": open_by_node,
                "nodes_enabled": enabled_nodes.len(),
                "nodes_healthy": healthy_node_count,
                "active_claims": claimed,
            },
            "suggestions": suggestions
        }))
    }

    fn active_claim_count(&self) -> usize {
        let Some(runtime_root) = &self.runtime_root else {
            return 0;
        };
        let Ok(bytes) = fs::read(runtime_root.join(WORKFLOW_AUTOMATION_STATE_FILE)) else {
            return 0;
        };
        serde_json::from_slice::<WorkflowAutomationState>(&bytes)
            .map(|state| {
                state
                    .claims
                    .iter()
                    .filter(|claim| {
                        matches!(
                            claim.state,
                            WorkflowClaimState::Claimed | WorkflowClaimState::Running
                        )
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}

fn suggest(suggestions: &mut Vec<serde_json::Value>, id: &str, reason: &str, command: &str) {
    suggestions.push(serde_json::json!({
        "priority": suggestions.len() + 1,
        "id": id,
        "reason": reason,
        "command": command
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::host::cluster::{FileClusterService, NodeRemoteUpdate};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
    }

    #[test]
    fn next_suggests_first_goal_when_nothing_is_tracked() {
        let temp_root = unique_temp_dir("guidance-empty");
        let refine_dir = temp_root.join(".refine");
        fs::create_dir_all(&refine_dir).unwrap();
        let response = FileNextActionsService::new(&refine_dir)
            .next_response()
            .unwrap();
        assert_eq!(response["suggestions"][0]["id"], "capture-first-goal");
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn next_suggests_distribution_and_convergence() {
        let temp_root = unique_temp_dir("guidance-fleet");
        let refine_dir = temp_root.join(".refine");
        let cluster = FileClusterService::new(&refine_dir);
        cluster
            .upsert_node("fly-worker-1", NodeRemoteUpdate::default())
            .unwrap();
        let work = FileWorkItemService::new(&refine_dir);
        work.create_goal_summary("Goal A", Some("GOAL1")).unwrap();
        work.create_goal_summary("Goal B", Some("GOAL2")).unwrap();
        // a reviewable goal stranded on the worker
        work.create_goal_summary("Goal C", Some("GOAL3")).unwrap();
        work.transfer_goal_to_node("fly-worker-1", "GOAL3").unwrap();
        let goal_path = refine_dir.join("goals/GO/AL3/goal.json");
        let review = fs::read_to_string(&goal_path)
            .unwrap()
            .replace("\"backlog\"", "\"review\"");
        fs::write(&goal_path, review).unwrap();

        let response = FileNextActionsService::new(&refine_dir)
            .next_response()
            .unwrap();
        let ids: Vec<&str> = response["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|suggestion| suggestion["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"distribute-work"), "ids: {ids:?}");
        assert!(ids.contains(&"converge-reviewables"), "ids: {ids:?}");
        let commands: Vec<&str> = response["suggestions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|suggestion| suggestion["command"].as_str().unwrap())
            .collect();
        assert!(commands.contains(&"refine cluster distribute --converge --to default"));
        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn next_reports_steady_state_when_nothing_needs_attention() {
        let temp_root = unique_temp_dir("guidance-quiet");
        let refine_dir = temp_root.join(".refine");
        let work = FileWorkItemService::new(&refine_dir);
        work.create_goal_summary("Done goal", Some("GOAL1"))
            .unwrap();
        let goal_path = refine_dir.join("goals/GO/AL1/goal.json");
        let done = fs::read_to_string(&goal_path)
            .unwrap()
            .replace("\"backlog\"", "\"done\"");
        fs::write(&goal_path, done).unwrap();
        let response = FileNextActionsService::new(&refine_dir)
            .next_response()
            .unwrap();
        assert_eq!(response["suggestions"][0]["id"], "all-quiet");
        fs::remove_dir_all(temp_root).unwrap();
    }
}

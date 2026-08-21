use std::collections::{BTreeMap, BTreeSet};

use crate::application::workflow::priority_rank;
use crate::model::goal::GoalIndexProjection;
use crate::model::workflow::GoalStatus;

fn releases_feature_order(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::Review | GoalStatus::Done | GoalStatus::Cancelled
    )
}

fn occupies_feature_slot(status: &GoalStatus) -> bool {
    automated_active(status)
}

fn automated_active(status: &GoalStatus) -> bool {
    matches!(
        status,
        GoalStatus::Plan | GoalStatus::Implement | GoalStatus::Quality | GoalStatus::Governance
    )
}

fn blocks_lower_priority_start(status: &GoalStatus) -> bool {
    *status == GoalStatus::Todo || automated_active(status)
}

fn scheduling_node_key(goal: &GoalIndexProjection) -> String {
    goal.node_id
        .as_deref()
        .unwrap_or("default")
        .trim()
        .to_ascii_lowercase()
}

pub(crate) struct SchedulingEligibility {
    feature_eligible: BTreeSet<String>,
    blocking_priority_rank: BTreeMap<String, u8>,
}

impl SchedulingEligibility {
    pub(crate) fn new<'a>(
        goals: impl IntoIterator<Item = &'a GoalIndexProjection> + Clone,
    ) -> Self {
        let feature_eligible = feature_eligible_goal_ids(goals.clone());
        let mut blocking_priority_rank = BTreeMap::new();
        for goal in goals {
            if goal.round_count == 0
                || !feature_eligible.contains(&goal.id)
                || !blocks_lower_priority_start(&goal.status)
            {
                continue;
            }
            let rank = priority_rank(&goal.priority);
            blocking_priority_rank
                .entry(scheduling_node_key(goal))
                .and_modify(|highest: &mut u8| *highest = (*highest).max(rank))
                .or_insert(rank);
        }
        Self {
            feature_eligible,
            blocking_priority_rank,
        }
    }

    pub(crate) fn feature_eligible(&self, goal_id: &str) -> bool {
        self.feature_eligible.contains(goal_id)
    }

    pub(crate) fn priority_eligible(&self, goal: &GoalIndexProjection) -> bool {
        goal.status != GoalStatus::Todo
            || self
                .blocking_priority_rank
                .get(&scheduling_node_key(goal))
                .is_none_or(|highest| priority_rank(&goal.priority) >= *highest)
    }
}

fn feature_eligible_goal_ids<'a>(
    goals: impl IntoIterator<Item = &'a GoalIndexProjection> + Clone,
) -> BTreeSet<String> {
    let mut lowest_holding_order: BTreeMap<(String, &str), i64> = BTreeMap::new();
    let mut occupying_count: BTreeMap<(String, &str), usize> = BTreeMap::new();
    for goal in goals.clone() {
        if goal.round_count == 0 {
            continue;
        }
        let (Some(feature_id), Some(order)) = (goal.feature_id.as_deref(), goal.feature_order)
        else {
            continue;
        };
        let key = (scheduling_node_key(goal), feature_id);
        if !releases_feature_order(&goal.status) {
            lowest_holding_order
                .entry(key.clone())
                .and_modify(|lowest| *lowest = (*lowest).min(order))
                .or_insert(order);
        }
        if occupies_feature_slot(&goal.status) {
            *occupying_count.entry(key).or_default() += 1;
        }
    }
    goals
        .into_iter()
        .filter(|goal| {
            if goal.round_count == 0 {
                return false;
            }
            let (Some(feature_id), Some(order)) = (goal.feature_id.as_deref(), goal.feature_order)
            else {
                return true;
            };
            let key = (scheduling_node_key(goal), feature_id);
            lowest_holding_order.get(&key).copied() == Some(order)
                && (goal.status != GoalStatus::Todo
                    || occupying_count.get(&key).copied().unwrap_or(0) == 0)
        })
        .map(|goal| goal.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::goal::GoalPriority;

    #[test]
    fn automated_active_statuses_hold_the_priority_barrier() {
        for status in [
            GoalStatus::Plan,
            GoalStatus::Implement,
            GoalStatus::Quality,
            GoalStatus::Governance,
        ] {
            let blocker = goal("high", status, GoalPriority::High, "node-a");
            let candidate = goal("low", GoalStatus::Todo, GoalPriority::Low, "node-a");
            let goals = [&blocker, &candidate];
            let eligibility = SchedulingEligibility::new(goals);

            assert!(eligibility.priority_eligible(&blocker));
            assert!(!eligibility.priority_eligible(&candidate));
        }
    }

    #[test]
    fn runner_active_todo_remains_a_priority_blocker() {
        let runner_active = goal("high", GoalStatus::Todo, GoalPriority::High, "node-a");
        let candidate = goal("low", GoalStatus::Todo, GoalPriority::Low, "node-a");
        let goals = [&runner_active, &candidate];
        let eligibility = SchedulingEligibility::new(goals);

        assert!(!eligibility.priority_eligible(&candidate));
    }

    #[test]
    fn inactive_and_terminal_statuses_release_the_priority_barrier() {
        for status in [
            GoalStatus::Backlog,
            GoalStatus::Review,
            GoalStatus::Done,
            GoalStatus::Failed,
            GoalStatus::Cancelled,
        ] {
            let released = goal("high", status, GoalPriority::High, "node-a");
            let candidate = goal("low", GoalStatus::Todo, GoalPriority::Low, "node-a");
            let goals = [&released, &candidate];
            let eligibility = SchedulingEligibility::new(goals);

            assert!(eligibility.priority_eligible(&candidate));
        }
    }

    #[test]
    fn feature_blocked_higher_priority_goal_does_not_block_priority() {
        let prerequisite = ordered_goal("first", GoalStatus::Todo, GoalPriority::Low, "node-a", 1);
        let blocked = ordered_goal("second", GoalStatus::Todo, GoalPriority::High, "node-a", 2);
        let candidate = goal(
            "independent",
            GoalStatus::Todo,
            GoalPriority::Medium,
            "node-a",
        );
        let goals = [&prerequisite, &blocked, &candidate];
        let eligibility = SchedulingEligibility::new(goals);

        assert!(!eligibility.feature_eligible("second"));
        assert!(eligibility.priority_eligible(&candidate));
    }

    #[test]
    fn equal_priorities_are_eligible_and_distinct_nodes_are_independent() {
        let same_rank = goal("same", GoalStatus::Plan, GoalPriority::High, "node-a");
        let same_node = goal("same-node", GoalStatus::Todo, GoalPriority::High, "node-a");
        let other_node = goal("other-node", GoalStatus::Todo, GoalPriority::Low, "node-b");
        let goals = [&same_rank, &same_node, &other_node];
        let eligibility = SchedulingEligibility::new(goals);

        assert!(eligibility.priority_eligible(&same_node));
        assert!(eligibility.priority_eligible(&other_node));
    }

    #[test]
    fn legacy_node_id_spellings_share_scheduling_gates() {
        let blocker = goal("high", GoalStatus::Plan, GoalPriority::High, " NODE-A ");
        let candidate = goal("low", GoalStatus::Todo, GoalPriority::Low, "node-a");
        let goals = [&blocker, &candidate];
        let eligibility = SchedulingEligibility::new(goals);

        assert!(!eligibility.priority_eligible(&candidate));

        let first = ordered_goal("first", GoalStatus::Todo, GoalPriority::Low, "NODE-A", 1);
        let second = ordered_goal("second", GoalStatus::Todo, GoalPriority::Low, "node-a", 2);
        let goals = [&first, &second];
        let eligibility = SchedulingEligibility::new(goals);

        assert!(!eligibility.feature_eligible("second"));
    }

    #[test]
    fn unauthored_todo_does_not_hold_the_priority_barrier() {
        let mut unauthored = goal("high", GoalStatus::Todo, GoalPriority::High, "node-a");
        unauthored.round_count = 0;
        let candidate = goal("low", GoalStatus::Todo, GoalPriority::Low, "node-a");
        let goals = [&unauthored, &candidate];
        let eligibility = SchedulingEligibility::new(goals);

        assert!(eligibility.priority_eligible(&candidate));
    }

    #[test]
    fn feature_order_releases_at_review_done_or_cancelled_but_not_failed() {
        for status in [GoalStatus::Review, GoalStatus::Done, GoalStatus::Cancelled] {
            let first = ordered_goal("first", status, GoalPriority::Medium, "node-a", 1);
            let second = ordered_goal(
                "second",
                GoalStatus::Todo,
                GoalPriority::Medium,
                "node-a",
                2,
            );
            let goals = [&first, &second];
            let eligibility = SchedulingEligibility::new(goals);

            assert!(eligibility.feature_eligible("second"));
        }

        let failed = ordered_goal(
            "first",
            GoalStatus::Failed,
            GoalPriority::Medium,
            "node-a",
            1,
        );
        let blocked = ordered_goal(
            "second",
            GoalStatus::Todo,
            GoalPriority::Medium,
            "node-a",
            2,
        );
        let goals = [&failed, &blocked];
        let eligibility = SchedulingEligibility::new(goals);

        assert!(!eligibility.feature_eligible("second"));
    }

    fn ordered_goal(
        id: &str,
        status: GoalStatus,
        priority: GoalPriority,
        node_id: &str,
        feature_order: i64,
    ) -> GoalIndexProjection {
        let mut goal = goal(id, status, priority, node_id);
        goal.feature_id = Some("feature".to_string());
        goal.feature_order = Some(feature_order);
        goal
    }

    fn goal(
        id: &str,
        status: GoalStatus,
        priority: GoalPriority,
        node_id: &str,
    ) -> GoalIndexProjection {
        GoalIndexProjection {
            id: id.to_string(),
            name: id.to_string(),
            status,
            priority,
            reporter: None,
            assignee: None,
            round_count: 1,
            created: "2026-08-18T00:00:00Z".to_string(),
            updated: "2026-08-18T00:00:00Z".to_string(),
            branch_name: None,
            node_id: Some(node_id.to_string()),
            feature_id: None,
            feature_order: None,
            json_path: format!("goals/{id}/goal.json"),
            mission: None,
        }
    }
}

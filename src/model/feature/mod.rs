use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::model::Timestamp;
use crate::model::goal::GoalIndexProjection;
use crate::model::workflow::GoalStatus;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Feature {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub reporter: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub node_id: Option<String>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub json_path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureIndexProjection {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub reporter: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub node_id: Option<String>,
    pub created: Timestamp,
    pub updated: Timestamp,
    pub json_path: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureDetail {
    pub feature: Feature,
    pub goals: Vec<GoalIndexProjection>,
    pub node_display_name: Option<String>,
    pub rollup: FeatureRollup,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureRollup {
    pub status: GoalStatus,
    pub goal_count: usize,
    pub done_count: usize,
    pub active_count: usize,
    pub failed_count: usize,
    pub cancelled_count: usize,
    pub blocked_count: usize,
    pub next_goal: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureGoalBlockingNotice {
    pub feature_id: String,
    pub blocking_goal_id: String,
    pub blocked_goal_ids: Vec<String>,
    pub blocked_count: usize,
    pub next_blocked_goal_id: Option<String>,
    pub message: String,
}

pub fn is_ordered_feature_goal(feature_order: Option<i64>) -> bool {
    feature_order.is_some()
}

pub fn compare_feature_goal_order(a_order: Option<i64>, b_order: Option<i64>) -> Ordering {
    match (a_order, b_order) {
        (Some(a_order), Some(b_order)) => a_order.cmp(&b_order),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

impl FeatureRollup {
    pub fn derive(goals: &[GoalIndexProjection]) -> Self {
        let goal_count = goals.len();
        let done_count = goals
            .iter()
            .filter(|goal| goal.status == GoalStatus::Done)
            .count();
        let active_count = goals
            .iter()
            .filter(|goal| {
                matches!(
                    goal.status,
                    GoalStatus::InProgress
                        | GoalStatus::Qa
                        | GoalStatus::ReadyMerge
                        | GoalStatus::Build
                        | GoalStatus::Review
                )
            })
            .count();
        let failed_count = goals
            .iter()
            .filter(|goal| goal.status == GoalStatus::Failed)
            .count();
        let cancelled_count = goals
            .iter()
            .filter(|goal| goal.status == GoalStatus::Cancelled)
            .count();
        let blocked_count = goals
            .iter()
            .filter(|goal| matches!(goal.status, GoalStatus::Failed | GoalStatus::Cancelled))
            .count();
        let next_goal = goals
            .iter()
            .find(|goal| matches!(goal.status, GoalStatus::Todo | GoalStatus::Backlog))
            .map(|goal| goal.id.clone());

        let status = if goal_count > 0 && done_count == goal_count {
            GoalStatus::Done
        } else if active_count > 0 {
            GoalStatus::InProgress
        } else if failed_count > 0 {
            GoalStatus::Failed
        } else if cancelled_count == goal_count && goal_count > 0 {
            GoalStatus::Cancelled
        } else if goals.iter().any(|goal| goal.status == GoalStatus::Todo) {
            GoalStatus::Todo
        } else {
            GoalStatus::Backlog
        };

        Self {
            status,
            goal_count,
            done_count,
            active_count,
            failed_count,
            cancelled_count,
            blocked_count,
            next_goal,
        }
    }
}

pub fn failed_goal_feature_blocking_notice(
    goal: &GoalIndexProjection,
    feature_goals: &[GoalIndexProjection],
) -> Option<FeatureGoalBlockingNotice> {
    if goal.status != GoalStatus::Failed {
        return None;
    }
    let feature_id = goal.feature_id.as_deref()?;
    let feature_order = goal.feature_order?;
    let node_id = goal.node_id.as_deref().unwrap_or("default");
    let mut blocked_goals = feature_goals
        .iter()
        .filter(|other| other.id != goal.id)
        .filter(|other| other.feature_id.as_deref() == Some(feature_id))
        .filter(|other| other.node_id.as_deref().unwrap_or("default") == node_id)
        .filter(|other| {
            other
                .feature_order
                .is_some_and(|order| order > feature_order)
        })
        .filter(|other| !matches!(other.status, GoalStatus::Done | GoalStatus::Cancelled))
        .cloned()
        .collect::<Vec<_>>();
    blocked_goals.sort_by(|a, b| {
        compare_feature_goal_order(a.feature_order, b.feature_order).then_with(|| a.id.cmp(&b.id))
    });
    if blocked_goals.is_empty() {
        return None;
    }
    let blocked_goal_ids = blocked_goals
        .iter()
        .map(|blocked_goal| blocked_goal.id.clone())
        .collect::<Vec<_>>();
    let blocked_count = blocked_goal_ids.len();
    let next_blocked_goal_id = blocked_goal_ids.first().cloned();
    let plural = if blocked_count == 1 { "Goal" } else { "Goals" };
    let message = match next_blocked_goal_id.as_deref() {
        Some(next) if blocked_count == 1 => format!(
            "This failed Goal is blocking the next Goal in Feature {feature_id} ({next}). Submit a recovery round so this Goal can finish, or cancel it to let later Feature work continue."
        ),
        Some(next) => format!(
            "This failed Goal is blocking {blocked_count} later {plural} in Feature {feature_id}; next blocked Goal: {next}. Submit a recovery round so this Goal can finish, or cancel it to let later Feature work continue."
        ),
        None => format!(
            "This failed Goal is blocking later Feature work in Feature {feature_id}. Submit a recovery round so this Goal can finish, or cancel it to let later Feature work continue."
        ),
    };
    Some(FeatureGoalBlockingNotice {
        feature_id: feature_id.to_string(),
        blocking_goal_id: goal.id.clone(),
        blocked_goal_ids,
        blocked_count,
        next_blocked_goal_id,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::goal::GoalPriority;

    #[test]
    fn failed_goal_feature_blocking_notice_names_later_feature_work() {
        let failed = goal("GOAL1", GoalStatus::Failed, Some(1));
        let blocked = goal("GOAL2", GoalStatus::Todo, Some(2));
        let done = goal("GOAL3", GoalStatus::Done, Some(3));

        let notice =
            failed_goal_feature_blocking_notice(&failed, &[blocked.clone(), done, failed.clone()])
                .expect("failed first Goal should block later unfinished Feature work");

        assert_eq!(notice.feature_id, "FEA1");
        assert_eq!(notice.blocking_goal_id, "GOAL1");
        assert_eq!(notice.blocked_goal_ids, vec!["GOAL2"]);
        assert_eq!(notice.next_blocked_goal_id.as_deref(), Some("GOAL2"));
        assert!(
            notice
                .message
                .contains("This failed Goal is blocking the next Goal")
        );
    }

    #[test]
    fn failed_goal_feature_blocking_notice_ignores_unordered_or_terminal_work() {
        let failed = goal("GOAL1", GoalStatus::Failed, Some(1));
        let done = goal("GOAL2", GoalStatus::Done, Some(2));
        let unordered = goal("GOAL3", GoalStatus::Todo, None);
        let active = goal("GOAL4", GoalStatus::Todo, Some(0));

        assert!(
            failed_goal_feature_blocking_notice(&failed, &[done, unordered, active],).is_none()
        );
        assert!(
            failed_goal_feature_blocking_notice(
                &goal("GOAL1", GoalStatus::Todo, Some(1)),
                &[goal("GOAL2", GoalStatus::Todo, Some(2))],
            )
            .is_none()
        );
    }

    fn goal(id: &str, status: GoalStatus, feature_order: Option<i64>) -> GoalIndexProjection {
        GoalIndexProjection {
            id: id.to_string(),
            name: id.to_string(),
            status,
            priority: GoalPriority::Medium,
            reporter: None,
            assignee: None,
            round_count: 1,
            created: "created".to_string(),
            updated: "updated".to_string(),
            branch_name: None,
            node_id: Some("default".to_string()),
            feature_id: Some("FEA1".to_string()),
            feature_order,
            json_path: format!("goals/{id}/goal.json"),
        }
    }
}

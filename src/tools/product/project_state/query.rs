use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::model::feature::FeatureRollup;
use crate::model::goal::{GoalIndexProjection, GoalPriority};
use crate::model::workflow::GoalStatus;

use super::helpers::*;
use super::types::*;

pub trait ProjectionQuery {
    fn status_counts(&self) -> BTreeMap<GoalStatus, usize>;
    fn dashboard_summary(&self, query: DashboardProjectionQuery) -> DashboardProjectionSummary;
    fn goal_ids_for_status(&self, status: &GoalStatus) -> Vec<String>;
    fn feature_rollup(&self, feature_id: &str) -> Option<FeatureRollup>;
    fn list_goals(&self, query: GoalProjectionQuery) -> GoalProjectionList;
    fn list_features(&self, query: FeatureProjectionQuery) -> FeatureProjectionList;
    fn list_activity(&self, query: ActivityProjectionQuery) -> ActivityProjectionList;
    fn list_changes(&self, query: ChangeProjectionQuery) -> ChangeProjectionList;
    fn cache_path_for_port(&self, runtime_root: &Path, port: u16) -> PathBuf {
        runtime_root.join(port.to_string()).join("cache")
    }
}

impl ProjectionQuery for ProjectionSnapshot {
    fn status_counts(&self) -> BTreeMap<GoalStatus, usize> {
        let mut counts = BTreeMap::new();
        for projection in self.goals.values() {
            *counts.entry(projection.goal.status.clone()).or_insert(0) += 1;
        }
        counts
    }

    fn dashboard_summary(&self, query: DashboardProjectionQuery) -> DashboardProjectionSummary {
        let current_node_id = query
            .current_node_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default")
            .to_string();
        let node_filter = if query.node.as_deref() == Some("all") {
            "all".to_string()
        } else {
            "current".to_string()
        };
        let scoped_goals = self
            .goals
            .values()
            .filter(|projection| {
                goal_matches_node(
                    projection.goal.node_id.as_deref(),
                    &node_filter,
                    Some(current_node_id.as_str()),
                )
            })
            .collect::<Vec<_>>();
        let counts = goal_status_counts(scoped_goals.iter().map(|goal| &goal.goal.status));
        let all_node_counts = goal_status_counts(self.goals.values().map(|goal| &goal.goal.status));
        let mut reporter_stats: BTreeMap<String, BTreeMap<GoalStatus, usize>> = BTreeMap::new();
        let mut assignee_stats: BTreeMap<String, BTreeMap<GoalStatus, usize>> = BTreeMap::new();
        for goal in &scoped_goals {
            let reporter = goal
                .goal
                .reporter
                .clone()
                .filter(|reporter| !reporter.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            *reporter_stats
                .entry(reporter)
                .or_default()
                .entry(goal.goal.status.clone())
                .or_default() += 1;
            let assignee = goal
                .goal
                .assignee
                .clone()
                .filter(|assignee| !assignee.is_empty())
                .unwrap_or_else(|| "unassigned".to_string());
            *assignee_stats
                .entry(assignee)
                .or_default()
                .entry(goal.goal.status.clone())
                .or_default() += 1;
        }
        let failed_count = counts.get(&GoalStatus::Failed).copied().unwrap_or_default();
        let attention_indicators = if failed_count > 0 {
            vec![format!("{failed_count} failed Goal(s) need recovery")]
        } else {
            Vec::new()
        };
        let recent_activity_ids = self
            .dashboard
            .recent_activity_ids
            .iter()
            .filter(|activity_id| {
                self.activity
                    .get(*activity_id)
                    .and_then(|activity| activity.entry.goal_id.as_deref())
                    .and_then(|goal_id| self.goals.get(goal_id))
                    .map(|goal| {
                        goal_matches_node(
                            goal.goal.node_id.as_deref(),
                            &node_filter,
                            Some(current_node_id.as_str()),
                        )
                    })
                    .unwrap_or(node_filter == "all")
            })
            .cloned()
            .collect();
        DashboardProjectionSummary {
            node_filter,
            current_node_id,
            counts,
            all_node_counts,
            reporter_stats,
            assignee_stats,
            attention_indicators,
            recent_activity_ids,
        }
    }

    fn goal_ids_for_status(&self, status: &GoalStatus) -> Vec<String> {
        self.goals
            .iter()
            .filter_map(|(goal_id, projection)| {
                if &projection.goal.status == status {
                    Some(goal_id.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn feature_rollup(&self, feature_id: &str) -> Option<FeatureRollup> {
        self.features
            .get(feature_id)
            .map(|projection| projection.rollup.clone())
    }

    fn list_goals(&self, query: GoalProjectionQuery) -> GoalProjectionList {
        let mut rows = self
            .goals
            .values()
            .filter(|projection| goal_matches(self, projection, &query))
            .map(|projection| projection.goal.clone())
            .collect::<Vec<_>>();
        sort_goals(&mut rows, &query.page.sort, &query.page.dir);
        let total = rows.len();
        let filtered_status_counts = goal_status_counts(rows.iter().map(|goal| &goal.status));
        let matching_ids = rows.iter().map(|goal| goal.id.clone()).collect::<Vec<_>>();
        let goals = rows
            .into_iter()
            .skip(query.page.offset)
            .take(query.page.limit)
            .collect();
        GoalProjectionList {
            goals,
            total,
            filtered_status_counts,
            matching_ids,
        }
    }

    fn list_features(&self, query: FeatureProjectionQuery) -> FeatureProjectionList {
        let mut rows = self
            .features
            .values()
            .filter(|projection| feature_matches(projection, &query))
            .cloned()
            .collect::<Vec<_>>();
        sort_features(&mut rows, &query.page.sort, &query.page.dir);
        let total = rows.len();
        let matching_ids = rows
            .iter()
            .map(|feature| feature.feature.id.clone())
            .collect::<Vec<_>>();
        let features = rows
            .into_iter()
            .skip(query.page.offset)
            .take(query.page.limit)
            .collect();
        FeatureProjectionList {
            features,
            total,
            matching_ids,
        }
    }

    fn list_activity(&self, query: ActivityProjectionQuery) -> ActivityProjectionList {
        let mut rows = self
            .activity
            .values()
            .filter(|projection| activity_projection_matches(projection, &query))
            .cloned()
            .collect::<Vec<_>>();
        sort_activity(&mut rows, &query.page.sort, &query.page.dir);
        let total = rows.len();
        let matching_ids = rows
            .iter()
            .map(|activity| activity.entry.id.clone())
            .collect::<Vec<_>>();
        let facets = activity_facets(self.activity.values());
        let activity = rows
            .into_iter()
            .skip(query.page.offset)
            .take(query.page.limit)
            .map(|activity| activity.entry)
            .collect();
        ActivityProjectionList {
            activity,
            total,
            matching_ids,
            facets,
        }
    }

    fn list_changes(&self, query: ChangeProjectionQuery) -> ChangeProjectionList {
        let mut rows = self
            .changes
            .values()
            .filter(|projection| change_projection_matches(projection, &query))
            .cloned()
            .collect::<Vec<_>>();
        sort_changes(&mut rows, &query.page.sort, &query.page.dir);
        let total = rows.len();
        let matching_ids = rows.iter().map(change_projection_key).collect::<Vec<_>>();
        let changes = rows
            .into_iter()
            .skip(query.page.offset)
            .take(query.page.limit)
            .collect();
        ChangeProjectionList {
            changes,
            total,
            matching_ids,
        }
    }
}

fn goal_matches(
    snapshot: &ProjectionSnapshot,
    projection: &GoalSummaryProjection,
    query: &GoalProjectionQuery,
) -> bool {
    let goal = &projection.goal;
    if query
        .status
        .as_ref()
        .is_some_and(|status| &goal.status != status)
    {
        return false;
    }
    if query
        .reporter
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|reporter| goal.reporter.as_deref() != Some(reporter))
    {
        return false;
    }
    if query
        .assignee
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|assignee| goal.assignee.as_deref() != Some(assignee))
    {
        return false;
    }
    if let Some(node) = query
        .node
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        match node {
            value => {
                if !goal_matches_node(
                    goal.node_id.as_deref(),
                    value,
                    query.current_node_id.as_deref(),
                ) {
                    return false;
                }
            }
        }
    }
    if let Some(feature) = query
        .feature
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        match feature {
            "standalone" | "__standalone" | "none" => {
                if goal.feature_id.is_some() {
                    return false;
                }
            }
            value => {
                if goal.feature_id.as_deref() != Some(value) {
                    return false;
                }
            }
        }
    }
    if query
        .rounds_gte
        .is_some_and(|minimum| goal.round_count < minimum)
    {
        return false;
    }
    if query
        .rounds_lte
        .is_some_and(|maximum| goal.round_count > maximum)
    {
        return false;
    }
    if !activity_matches(snapshot, projection, query) {
        return false;
    }
    if let Some(q) = query.q.as_deref().filter(|value| !value.trim().is_empty()) {
        let q = q.to_lowercase();
        if !projection.searchable_text.to_lowercase().contains(&q)
            && !goal.id.to_lowercase().contains(&q)
            && !goal.name.to_lowercase().contains(&q)
            && !goal
                .reporter
                .as_deref()
                .map(|reporter| reporter.to_lowercase().contains(&q))
                .unwrap_or(false)
            && !goal
                .assignee
                .as_deref()
                .map(|assignee| assignee.to_lowercase().contains(&q))
                .unwrap_or(false)
        {
            return false;
        }
    }
    true
}

fn activity_matches(
    snapshot: &ProjectionSnapshot,
    projection: &GoalSummaryProjection,
    query: &GoalProjectionQuery,
) -> bool {
    let wants_activity = query
        .severity
        .as_deref()
        .is_some_and(|value| !value.is_empty())
        || query
            .category
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        || query
            .actor
            .as_deref()
            .is_some_and(|value| !value.is_empty());
    if !wants_activity {
        return true;
    }
    projection.activity_ids.iter().any(|activity_id| {
        let Some(activity) = snapshot.activity.get(activity_id) else {
            return false;
        };
        if query
            .severity
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_some_and(|severity| activity.entry.severity != severity)
        {
            return false;
        }
        if query
            .category
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_some_and(|category| activity.entry.category != category)
        {
            return false;
        }
        if query
            .actor
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_some_and(|actor| activity.entry.actor.as_deref() != Some(actor))
        {
            return false;
        }
        true
    })
}

fn activity_projection_matches(
    projection: &ActivitySummaryProjection,
    query: &ActivityProjectionQuery,
) -> bool {
    let entry = &projection.entry;
    if query
        .goal_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|goal_id| entry.goal_id.as_deref() != Some(goal_id))
    {
        return false;
    }
    if query
        .severity
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|severity| entry.severity != severity)
    {
        return false;
    }
    if query
        .category
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|category| entry.category != category)
    {
        return false;
    }
    if query
        .actor
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|actor| entry.actor.as_deref() != Some(actor))
    {
        return false;
    }
    if let Some(q) = query.q.as_deref().filter(|value| !value.trim().is_empty()) {
        let q = q.to_lowercase();
        if !projection.searchable_text.to_lowercase().contains(&q)
            && !entry.id.to_lowercase().contains(&q)
            && !entry.message.to_lowercase().contains(&q)
        {
            return false;
        }
    }
    true
}

fn change_projection_matches(
    projection: &ChangeSummaryProjection,
    query: &ChangeProjectionQuery,
) -> bool {
    if query
        .goal_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|goal_id| projection.goal_id.as_deref() != Some(goal_id))
    {
        return false;
    }
    if query
        .status
        .as_ref()
        .is_some_and(|status| projection.goal_status.as_ref() != Some(status))
    {
        return false;
    }
    if query
        .priority
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|priority| projection.goal_priority.as_deref() != Some(priority))
    {
        return false;
    }
    if query
        .branch
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|branch| projection.branch.as_deref() != Some(branch))
    {
        return false;
    }
    if let Some(q) = query.q.as_deref().filter(|value| !value.trim().is_empty()) {
        let q = q.to_lowercase();
        if !projection.searchable_text.to_lowercase().contains(&q) {
            return false;
        }
    }
    true
}

fn activity_facets<'a>(
    activity: impl Iterator<Item = &'a ActivitySummaryProjection>,
) -> ActivityProjectionFacets {
    let mut categories = BTreeSet::new();
    let mut severities = BTreeSet::new();
    let mut actors = BTreeSet::new();
    for projection in activity {
        if !projection.entry.category.is_empty() {
            categories.insert(projection.entry.category.clone());
        }
        if !projection.entry.severity.is_empty() {
            severities.insert(projection.entry.severity.clone());
        }
        if let Some(actor) = &projection.entry.actor
            && !actor.is_empty()
        {
            actors.insert(actor.clone());
        }
    }
    ActivityProjectionFacets {
        categories: categories.into_iter().collect(),
        severities: severities.into_iter().collect(),
        actors: actors.into_iter().collect(),
    }
}

fn feature_matches(projection: &FeatureSummaryProjection, query: &FeatureProjectionQuery) -> bool {
    let feature = &projection.feature;
    if query
        .status
        .as_ref()
        .is_some_and(|status| &projection.status != status)
    {
        return false;
    }
    if query
        .reporter
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|reporter| feature.reporter.as_deref() != Some(reporter))
    {
        return false;
    }
    if query
        .assignee
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|assignee| feature.assignee.as_deref() != Some(assignee))
    {
        return false;
    }
    if let Some(node) = query
        .node
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        match node {
            value => {
                if !goal_matches_node(
                    feature.node_id.as_deref(),
                    value,
                    query.current_node_id.as_deref(),
                ) {
                    return false;
                }
            }
        }
    }
    if let Some(q) = query.q.as_deref().filter(|value| !value.trim().is_empty()) {
        let q = q.to_lowercase();
        if !feature.id.to_lowercase().contains(&q)
            && !feature.name.to_lowercase().contains(&q)
            && !feature
                .description
                .as_deref()
                .map(|description| description.to_lowercase().contains(&q))
                .unwrap_or(false)
            && !feature
                .reporter
                .as_deref()
                .map(|reporter| reporter.to_lowercase().contains(&q))
                .unwrap_or(false)
            && !feature
                .assignee
                .as_deref()
                .map(|assignee| assignee.to_lowercase().contains(&q))
                .unwrap_or(false)
        {
            return false;
        }
    }
    true
}

fn goal_matches_node(owner: Option<&str>, node: &str, current_node_id: Option<&str>) -> bool {
    match node {
        "all" => true,
        "current" => owner.unwrap_or("default") == current_node_id.unwrap_or("default"),
        "unknown" => owner.is_none(),
        value => owner == Some(value),
    }
}

fn sort_goals(rows: &mut [GoalIndexProjection], sort: &str, dir: &str) {
    rows.sort_by(|a, b| {
        let ordering = match sort {
            "name" => a.name.cmp(&b.name),
            "status" => a.status.cmp(&b.status),
            "priority" => priority_rank(&a.priority).cmp(&priority_rank(&b.priority)),
            "reporter" => a.reporter.cmp(&b.reporter),
            "assignee" => a.assignee.cmp(&b.assignee),
            "rounds" | "round_count" => a.round_count.cmp(&b.round_count),
            "node" => a.node_id.cmp(&b.node_id),
            "created" => a.created.cmp(&b.created),
            "id" => a.id.cmp(&b.id),
            _ => a.updated.cmp(&b.updated),
        }
        .then_with(|| a.id.cmp(&b.id));
        if dir == "asc" {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn sort_features(rows: &mut [FeatureSummaryProjection], sort: &str, dir: &str) {
    rows.sort_by(|a, b| {
        let ordering = match sort {
            "name" => a.feature.name.cmp(&b.feature.name),
            "status" => a.status.cmp(&b.status),
            "reporter" => a.feature.reporter.cmp(&b.feature.reporter),
            "assignee" => a.feature.assignee.cmp(&b.feature.assignee),
            "node" => a.feature.node_id.cmp(&b.feature.node_id),
            "created" => a.feature.created.cmp(&b.feature.created),
            "id" => a.feature.id.cmp(&b.feature.id),
            _ => a.feature.updated.cmp(&b.feature.updated),
        }
        .then_with(|| a.feature.id.cmp(&b.feature.id));
        if dir == "asc" {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn sort_activity(rows: &mut [ActivitySummaryProjection], sort: &str, dir: &str) {
    rows.sort_by(|a, b| {
        let ordering = match sort {
            "severity" => a.entry.severity.cmp(&b.entry.severity),
            "category" => a.entry.category.cmp(&b.entry.category),
            "actor" => a.entry.actor.cmp(&b.entry.actor),
            "goal_id" | "goal" => a.entry.goal_id.cmp(&b.entry.goal_id),
            "message" => a.entry.message.cmp(&b.entry.message),
            "id" => a.entry.id.cmp(&b.entry.id),
            _ => a.entry.datetime.cmp(&b.entry.datetime),
        }
        .then_with(|| a.entry.id.cmp(&b.entry.id));
        if dir == "asc" {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn sort_changes(rows: &mut [ChangeSummaryProjection], sort: &str, dir: &str) {
    rows.sort_by(|a, b| {
        let ordering = match sort {
            "commit" => a.commit.cmp(&b.commit),
            "subject" => a.subject.cmp(&b.subject),
            "branch" => a.branch.cmp(&b.branch),
            "goal_id" | "goal" => a.goal_id.cmp(&b.goal_id),
            "status" => a.goal_status.cmp(&b.goal_status),
            "priority" => a.goal_priority.cmp(&b.goal_priority),
            "assignee" => a.goal_assignee.cmp(&b.goal_assignee),
            _ => b
                .order
                .cmp(&a.order)
                .then_with(|| a.committed_time.cmp(&b.committed_time)),
        }
        .then_with(|| a.commit.cmp(&b.commit));
        if dir == "asc" {
            ordering
        } else {
            ordering.reverse()
        }
    });
}

fn priority_rank(priority: &GoalPriority) -> u8 {
    match priority {
        GoalPriority::Low => 0,
        GoalPriority::Medium => 1,
        GoalPriority::High => 2,
    }
}

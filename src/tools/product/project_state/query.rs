use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::model::feature::FeatureRollup;
use crate::model::gap::{GapIndexProjection, GapPriority};
use crate::model::workflow::GapStatus;

use super::helpers::*;
use super::types::*;

pub trait ProjectionQuery {
    fn status_counts(&self) -> BTreeMap<GapStatus, usize>;
    fn gap_ids_for_status(&self, status: &GapStatus) -> Vec<String>;
    fn feature_rollup(&self, feature_id: &str) -> Option<FeatureRollup>;
    fn list_gaps(&self, query: GapProjectionQuery) -> GapProjectionList;
    fn list_features(&self, query: FeatureProjectionQuery) -> FeatureProjectionList;
    fn list_activity(&self, query: ActivityProjectionQuery) -> ActivityProjectionList;
    fn list_changes(&self, query: ChangeProjectionQuery) -> ChangeProjectionList;
    fn cache_path_for_port(&self, runtime_root: &Path, port: u16) -> PathBuf {
        runtime_root.join(port.to_string()).join("cache")
    }
}

impl ProjectionQuery for ProjectionSnapshot {
    fn status_counts(&self) -> BTreeMap<GapStatus, usize> {
        let mut counts = BTreeMap::new();
        for projection in self.gaps.values() {
            *counts.entry(projection.gap.status.clone()).or_insert(0) += 1;
        }
        counts
    }

    fn gap_ids_for_status(&self, status: &GapStatus) -> Vec<String> {
        self.gaps
            .iter()
            .filter_map(|(gap_id, projection)| {
                if &projection.gap.status == status {
                    Some(gap_id.clone())
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

    fn list_gaps(&self, query: GapProjectionQuery) -> GapProjectionList {
        let mut rows = self
            .gaps
            .values()
            .filter(|projection| gap_matches(self, projection, &query))
            .map(|projection| projection.gap.clone())
            .collect::<Vec<_>>();
        sort_gaps(&mut rows, &query.page.sort, &query.page.dir);
        let total = rows.len();
        let filtered_status_counts = gap_status_counts(rows.iter().map(|gap| &gap.status));
        let matching_ids = rows.iter().map(|gap| gap.id.clone()).collect::<Vec<_>>();
        let gaps = rows
            .into_iter()
            .skip(query.page.offset)
            .take(query.page.limit)
            .collect();
        GapProjectionList {
            gaps,
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

fn gap_matches(
    snapshot: &ProjectionSnapshot,
    projection: &GapSummaryProjection,
    query: &GapProjectionQuery,
) -> bool {
    let gap = &projection.gap;
    if query
        .status
        .as_ref()
        .is_some_and(|status| &gap.status != status)
    {
        return false;
    }
    if query
        .reporter
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|reporter| gap.reporter.as_deref() != Some(reporter))
    {
        return false;
    }
    if query
        .assignee
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|assignee| gap.assignee.as_deref() != Some(assignee))
    {
        return false;
    }
    if let Some(node) = query
        .node
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        match node {
            "all" => {}
            "current" => {
                if gap.node_id.as_deref() != query.current_node_id.as_deref().or(Some("default")) {
                    return false;
                }
            }
            "unknown" => {
                if gap.node_id.is_some() {
                    return false;
                }
            }
            value => {
                if gap.node_id.as_deref() != Some(value) {
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
                if gap.feature_id.is_some() {
                    return false;
                }
            }
            value => {
                if gap.feature_id.as_deref() != Some(value) {
                    return false;
                }
            }
        }
    }
    if query
        .rounds_gte
        .is_some_and(|minimum| gap.round_count < minimum)
    {
        return false;
    }
    if query
        .rounds_lte
        .is_some_and(|maximum| gap.round_count > maximum)
    {
        return false;
    }
    if !activity_matches(snapshot, projection, query) {
        return false;
    }
    if let Some(q) = query.q.as_deref().filter(|value| !value.trim().is_empty()) {
        let q = q.to_lowercase();
        if !projection.searchable_text.to_lowercase().contains(&q)
            && !gap.id.to_lowercase().contains(&q)
            && !gap.name.to_lowercase().contains(&q)
            && !gap
                .reporter
                .as_deref()
                .map(|reporter| reporter.to_lowercase().contains(&q))
                .unwrap_or(false)
            && !gap
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
    projection: &GapSummaryProjection,
    query: &GapProjectionQuery,
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
        .gap_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|gap_id| entry.gap_id.as_deref() != Some(gap_id))
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
        .gap_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|gap_id| projection.gap_id.as_deref() != Some(gap_id))
    {
        return false;
    }
    if query
        .status
        .as_ref()
        .is_some_and(|status| projection.gap_status.as_ref() != Some(status))
    {
        return false;
    }
    if query
        .priority
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .is_some_and(|priority| projection.gap_priority.as_deref() != Some(priority))
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
            "all" => {}
            "current" => {
                if feature.node_id.as_deref()
                    != query.current_node_id.as_deref().or(Some("default"))
                {
                    return false;
                }
            }
            value => {
                if feature.node_id.as_deref() != Some(value) {
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

fn sort_gaps(rows: &mut [GapIndexProjection], sort: &str, dir: &str) {
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
            "gap_id" | "gap" => a.entry.gap_id.cmp(&b.entry.gap_id),
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
            "gap_id" | "gap" => a.gap_id.cmp(&b.gap_id),
            "status" => a.gap_status.cmp(&b.gap_status),
            "priority" => a.gap_priority.cmp(&b.gap_priority),
            "assignee" => a.gap_assignee.cmp(&b.gap_assignee),
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

fn priority_rank(priority: &GapPriority) -> u8 {
    match priority {
        GapPriority::Low => 0,
        GapPriority::Medium => 1,
        GapPriority::High => 2,
    }
}

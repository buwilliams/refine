use std::collections::BTreeSet;

use super::projection_store::{
    attach_activity_ids, cap_projected_activity, derive_dashboard, derive_features,
};
use super::*;

/// What changed on disk since a cached projection was built.
///
/// Staleness used to be a single boolean, so any change at all discarded the
/// whole projection and every Goal and Feature record was re-read and reparsed.
/// A Goal write is the most common event in a running fleet, which made the
/// expensive path the normal one. Knowing *which* sources moved lets the cached
/// projection be repaired instead.
#[derive(Debug, Default)]
pub(super) struct SourceDelta {
    pub(super) goals_changed: Vec<String>,
    /// Records the projection had never seen. Tracked apart from modifications
    /// because activity is keyed by which Goals exist, not by their contents: a
    /// Goal being edited cannot change the activity set, so the common write
    /// must not trigger a scan of every sidecar.
    pub(super) goals_added: Vec<String>,
    pub(super) goals_removed: Vec<String>,
    pub(super) features_changed: Vec<String>,
    pub(super) features_removed: Vec<String>,
    pub(super) activity_changed: bool,
    pub(super) git_head_changed: bool,
    pub(super) fingerprints: BTreeMap<String, SourceFingerprint>,
}

impl SourceDelta {
    pub(super) fn is_empty(&self) -> bool {
        self.goals_changed.is_empty()
            && self.goals_removed.is_empty()
            && self.features_changed.is_empty()
            && self.features_removed.is_empty()
            && !self.activity_changed
            && !self.git_head_changed
    }
}

/// Two fingerprints describe the same file contents.
///
/// `change_unix_ns` must be present on the stored side: an older snapshot that
/// predates it cannot be compared safely, so treating it as changed is what
/// keeps a stale projection from surviving.
fn fingerprint_matches(previous: &SourceFingerprint, current: &SourceFingerprint) -> bool {
    previous.size == current.size
        && previous.modified_unix_ms == current.modified_unix_ms
        && previous.change_unix_ns.is_some()
        && previous.change_unix_ns == current.change_unix_ns
}

impl FileProjectProjectionStore {
    /// Compares observed sources against a snapshot's fingerprints.
    ///
    /// Metadata only, so this costs one `stat` per source and never opens a
    /// file.
    pub(super) fn source_delta(
        &self,
        expected: &BTreeMap<String, SourceFingerprint>,
    ) -> RefineResult<SourceDelta> {
        let mut delta = SourceDelta::default();
        let mut observed_goal_paths = BTreeSet::new();
        let mut observed_feature_paths = BTreeSet::new();

        for path in Self::collect_json_files(&self.refine_dir.join("goals"), "goal.json")? {
            let rel_path = self.relative_path(&path)?;
            let fingerprint = Self::metadata_fingerprint(&path)?;
            match expected.get(&rel_path) {
                Some(previous) if fingerprint_matches(previous, &fingerprint) => {}
                Some(_) => delta.goals_changed.push(rel_path.clone()),
                None => {
                    delta.goals_changed.push(rel_path.clone());
                    delta.goals_added.push(rel_path.clone());
                }
            }
            observed_goal_paths.insert(rel_path.clone());
            delta.fingerprints.insert(rel_path, fingerprint);
        }

        for path in Self::collect_json_files(&self.refine_dir.join("features"), "feature.json")? {
            let rel_path = self.relative_path(&path)?;
            let fingerprint = Self::metadata_fingerprint(&path)?;
            if !expected
                .get(&rel_path)
                .is_some_and(|previous| fingerprint_matches(previous, &fingerprint))
            {
                delta.features_changed.push(rel_path.clone());
            }
            observed_feature_paths.insert(rel_path.clone());
            delta.fingerprints.insert(rel_path, fingerprint);
        }

        for path in Self::collect_json_files(&goal_logs_root(&self.refine_dir), "logs.jsonl")? {
            let rel_path = self.relative_path(&path)?;
            let fingerprint = Self::metadata_fingerprint(&path)?;
            if !expected
                .get(&rel_path)
                .is_some_and(|previous| fingerprint_matches(previous, &fingerprint))
            {
                delta.activity_changed = true;
            }
            delta.fingerprints.insert(rel_path, fingerprint);
        }

        let activity_path = self.refine_dir.join(ACTIVITY_LOG_FILE);
        if activity_path.exists() {
            let rel_path = self.relative_path(&activity_path)?;
            let fingerprint = Self::metadata_fingerprint(&activity_path)?;
            if !expected
                .get(&rel_path)
                .is_some_and(|previous| fingerprint_matches(previous, &fingerprint))
            {
                delta.activity_changed = true;
            }
            delta.fingerprints.insert(rel_path, fingerprint);
        }

        if let Some(fingerprint) = self.git_head_fingerprint() {
            if expected
                .get(&fingerprint.path)
                .is_none_or(|previous| previous.content_hash != fingerprint.content_hash)
            {
                delta.git_head_changed = true;
            }
            delta
                .fingerprints
                .insert(fingerprint.path.clone(), fingerprint);
        }

        // Anything the snapshot knew about that is no longer observed was
        // deleted. Log and activity sources are folded into the activity flag
        // because their projection is derived wholesale rather than per record.
        for rel_path in expected.keys() {
            if delta.fingerprints.contains_key(rel_path) {
                continue;
            }
            if rel_path.ends_with("/goal.json") {
                delta.goals_removed.push(rel_path.clone());
            } else if rel_path.ends_with("/feature.json") {
                delta.features_removed.push(rel_path.clone());
            } else if rel_path.ends_with("logs.jsonl") || rel_path.ends_with(ACTIVITY_LOG_FILE) {
                delta.activity_changed = true;
            } else if rel_path != "git:HEAD" {
                delta.git_head_changed = true;
            }
        }

        Ok(delta)
    }

    /// Repairs a cached projection in place from a delta.
    ///
    /// Only the records that moved are re-read. Everything derived — Feature
    /// rollups, dashboard aggregates, activity cross-references — is recomputed
    /// from the in-memory Goal map, which is arithmetic rather than I/O. A Goal
    /// write therefore costs one file read instead of a full corpus reparse.
    pub(super) fn patch_projection(
        &self,
        mut snapshot: ProjectionSnapshot,
        delta: SourceDelta,
    ) -> RefineResult<ProjectionSnapshot> {
        for rel_path in &delta.goals_removed {
            snapshot
                .goals
                .retain(|_, projection| projection.goal.json_path != *rel_path);
        }
        for rel_path in &delta.goals_changed {
            // A record is re-keyed by identity, not by path, so a Goal whose id
            // was corrected does not survive under its previous key.
            snapshot
                .goals
                .retain(|_, projection| projection.goal.json_path != *rel_path);
            if let Some(projection) = self.project_goal(&self.refine_dir.join(rel_path))? {
                snapshot
                    .goals
                    .insert(projection.goal.id.clone(), projection);
            }
        }

        for rel_path in &delta.features_removed {
            snapshot
                .features
                .retain(|_, projection| projection.feature.json_path != *rel_path);
        }
        let mut feature_records = snapshot
            .features
            .values()
            .filter(|projection| {
                !delta
                    .features_changed
                    .contains(&projection.feature.json_path)
            })
            .map(|projection| projection.feature.clone())
            .collect::<Vec<_>>();
        for rel_path in &delta.features_changed {
            if let Some(feature) = self.project_feature(&self.refine_dir.join(rel_path))? {
                feature_records.push(feature);
            }
        }
        // Rollups depend on every Goal in the Feature, so they are rederived
        // even for Features whose own record did not change.
        snapshot.features = derive_features(&snapshot.goals, feature_records);

        // Activity is derived from which Goals exist and what their sidecars
        // contain, so editing a Goal record cannot affect it. Rescanning only
        // when the Goal set or a sidecar actually moved keeps the ordinary
        // status write off the per-Goal sidecar scan.
        if delta.activity_changed
            || !delta.goals_added.is_empty()
            || !delta.goals_removed.is_empty()
        {
            let mut activity = self.project_activity()?;
            activity.extend(self.project_goal_round_activity(&snapshot.goals)?);
            cap_projected_activity(&mut activity);
            snapshot.activity = activity;
        }

        if delta.git_head_changed {
            snapshot.changes = self.project_changes(&snapshot.goals);
        }

        attach_activity_ids(&mut snapshot.goals, &snapshot.activity);
        snapshot.dashboard = derive_dashboard(&snapshot.goals, &snapshot.activity);
        snapshot.source_fingerprints = delta.fingerprints;
        // A patched projection is newly produced content, exactly as a rebuilt
        // one is, and carries the same generation marker. Leaving the cached
        // value in place would report the projection as older than it is.
        snapshot.generated_at = "unknown".to_string();
        snapshot.refine_dir = Some(self.refine_dir.clone());
        Ok(snapshot)
    }
}

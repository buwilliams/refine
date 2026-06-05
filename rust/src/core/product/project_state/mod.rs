use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::core::observability::activity::ACTIVITY_LOG_FILE;
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::feature::{FeatureIndexProjection, FeatureRollup};
use crate::model::gap::{GapIndexProjection, GapPriority};
use crate::model::log::ActivityEntry;
use crate::model::workflow::GapStatus;
use crate::model::{JsonObject, Timestamp};

pub const PROJECTION_SNAPSHOT_VERSION: u64 = 1;
pub const PROJECTION_SNAPSHOT_FILE: &str = "projection-snapshot.json";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectionSnapshot {
    pub version: u64,
    pub generated_at: Timestamp,
    pub source_fingerprints: BTreeMap<String, SourceFingerprint>,
    pub gaps: BTreeMap<String, GapSummaryProjection>,
    pub features: BTreeMap<String, FeatureSummaryProjection>,
    pub activity: BTreeMap<String, ActivitySummaryProjection>,
    pub changes: BTreeMap<String, ChangeSummaryProjection>,
    pub dashboard: DashboardProjection,
    pub runtime: RuntimeProjection,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceFingerprint {
    pub path: String,
    pub size: u64,
    pub modified_unix_ms: Option<i64>,
    pub content_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GapSummaryProjection {
    #[serde(flatten)]
    pub gap: GapIndexProjection,
    pub node_display_name: Option<String>,
    pub searchable_text: String,
    pub activity_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FeatureSummaryProjection {
    #[serde(flatten)]
    pub feature: FeatureIndexProjection,
    pub status: GapStatus,
    pub gap_ids: Vec<String>,
    pub rollup: FeatureRollup,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ActivitySummaryProjection {
    #[serde(flatten)]
    pub entry: ActivityEntry,
    pub searchable_text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ChangeSummaryProjection {
    pub commit: String,
    pub committed_time: Timestamp,
    pub subject: String,
    pub gap_id: Option<String>,
    pub branch: Option<String>,
    pub gap_name: Option<String>,
    pub gap_status: Option<GapStatus>,
    pub gap_priority: Option<String>,
    pub searchable_text: String,
    #[serde(default)]
    pub order: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DashboardProjection {
    pub all_node_status_counts: BTreeMap<GapStatus, usize>,
    pub current_node_status_counts: BTreeMap<GapStatus, usize>,
    pub reporter_stats: BTreeMap<String, BTreeMap<GapStatus, usize>>,
    pub attention_indicators: Vec<String>,
    pub recent_activity_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RuntimeProjection {
    pub supervisor: Option<JsonObject>,
    pub processes: Vec<JsonObject>,
    pub background_jobs: Vec<JsonObject>,
    pub target_app: Option<JsonObject>,
    pub performance: Option<JsonObject>,
    pub preflight: Option<JsonObject>,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectionIndex {
    pub gaps_by_status: BTreeMap<GapStatus, BTreeSet<String>>,
    pub gaps_by_node: BTreeMap<String, BTreeSet<String>>,
    pub gaps_by_feature: BTreeMap<String, BTreeSet<String>>,
    pub standalone_gap_ids: BTreeSet<String>,
    pub features_by_status: BTreeMap<GapStatus, BTreeSet<String>>,
    pub activity_by_gap: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageRequest {
    pub limit: usize,
    pub offset: usize,
    pub sort: String,
    pub dir: String,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: 50,
            offset: 0,
            sort: "updated".to_string(),
            dir: "desc".to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GapProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub status: Option<GapStatus>,
    pub reporter: Option<String>,
    pub node: Option<String>,
    pub current_node_id: Option<String>,
    pub feature: Option<String>,
    pub rounds_gte: Option<usize>,
    pub rounds_lte: Option<usize>,
    pub severity: Option<String>,
    pub category: Option<String>,
    pub actor: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FeatureProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub status: Option<GapStatus>,
    pub reporter: Option<String>,
    pub node: Option<String>,
    pub current_node_id: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActivityProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub gap_id: Option<String>,
    pub severity: Option<String>,
    pub category: Option<String>,
    pub actor: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangeProjectionQuery {
    pub page: PageRequest,
    pub q: Option<String>,
    pub gap_id: Option<String>,
    pub status: Option<GapStatus>,
    pub priority: Option<String>,
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GapProjectionList {
    pub gaps: Vec<GapIndexProjection>,
    pub total: usize,
    pub filtered_status_counts: BTreeMap<GapStatus, usize>,
    pub matching_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureProjectionList {
    pub features: Vec<FeatureSummaryProjection>,
    pub total: usize,
    pub matching_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ActivityProjectionFacets {
    pub categories: Vec<String>,
    pub severities: Vec<String>,
    pub actors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityProjectionList {
    pub activity: Vec<ActivityEntry>,
    pub total: usize,
    pub matching_ids: Vec<String>,
    pub facets: ActivityProjectionFacets,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChangeProjectionList {
    pub changes: Vec<ChangeSummaryProjection>,
    pub total: usize,
    pub matching_ids: Vec<String>,
}

impl ProjectionIndex {
    pub fn build(snapshot: &ProjectionSnapshot) -> Self {
        let mut index = Self::default();

        for (gap_id, projection) in &snapshot.gaps {
            index
                .gaps_by_status
                .entry(projection.gap.status.clone())
                .or_default()
                .insert(gap_id.clone());

            if let Some(node_id) = &projection.gap.node_id {
                index
                    .gaps_by_node
                    .entry(node_id.clone())
                    .or_default()
                    .insert(gap_id.clone());
            }

            if let Some(feature_id) = &projection.gap.feature_id {
                index
                    .gaps_by_feature
                    .entry(feature_id.clone())
                    .or_default()
                    .insert(gap_id.clone());
            } else {
                index.standalone_gap_ids.insert(gap_id.clone());
            }

            for activity_id in &projection.activity_ids {
                index
                    .activity_by_gap
                    .entry(gap_id.clone())
                    .or_default()
                    .insert(activity_id.clone());
            }
        }

        for (feature_id, projection) in &snapshot.features {
            index
                .features_by_status
                .entry(projection.status.clone())
                .or_default()
                .insert(feature_id.clone());
        }

        index
    }
}

pub trait ProjectStateStore {
    fn initialize(&self) -> RefineResult<()>;
    fn load_projection_snapshot(
        &self,
        cache_dir: &Path,
    ) -> RefineResult<Option<ProjectionSnapshot>>;
    fn persist_projection_snapshot(
        &self,
        cache_dir: &Path,
        snapshot: &ProjectionSnapshot,
    ) -> RefineResult<()>;
    fn rebuild_projection(&self) -> RefineResult<ProjectionSnapshot>;
}

#[derive(Clone, Debug)]
pub struct FileProjectStateStore {
    pub durable_root: PathBuf,
}

impl FileProjectStateStore {
    pub fn new(durable_root: impl Into<PathBuf>) -> Self {
        Self {
            durable_root: durable_root.into(),
        }
    }

    pub fn snapshot_path(cache_dir: &Path) -> PathBuf {
        cache_dir.join(PROJECTION_SNAPSHOT_FILE)
    }

    pub fn fingerprint(path: &Path) -> RefineResult<SourceFingerprint> {
        let metadata = fs::metadata(path).map_err(|error| {
            RefineError::Io(format!("failed to stat {}: {error}", path.display()))
        })?;
        let modified_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64);

        Ok(SourceFingerprint {
            path: path.display().to_string(),
            size: metadata.len(),
            modified_unix_ms,
            content_hash: Some(fingerprint_content_hash(path)?),
        })
    }

    pub fn collect_source_fingerprints(&self) -> RefineResult<BTreeMap<String, SourceFingerprint>> {
        let mut source_fingerprints = BTreeMap::new();
        for path in Self::collect_json_files(&self.durable_root.join("gaps"), "gap.json")? {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path, Self::fingerprint(&path)?);
        }
        for path in Self::collect_json_files(&self.durable_root.join("features"), "feature.json")? {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path, Self::fingerprint(&path)?);
        }
        let activity_path = self.durable_root.join(ACTIVITY_LOG_FILE);
        if activity_path.exists() {
            let rel_path = self.relative_path(&activity_path)?;
            source_fingerprints.insert(rel_path, Self::fingerprint(&activity_path)?);
        }
        if let Some(fingerprint) = self.git_head_fingerprint() {
            source_fingerprints.insert(fingerprint.path.clone(), fingerprint);
        }
        Ok(source_fingerprints)
    }

    pub fn load_or_refresh_projection(&self, cache_dir: &Path) -> RefineResult<ProjectionSnapshot> {
        let current_fingerprints = self.collect_source_fingerprints()?;
        if let Some(snapshot) = self.load_projection_snapshot(cache_dir)?
            && snapshot.source_fingerprints == current_fingerprints
        {
            return Ok(snapshot);
        }
        let snapshot = self.rebuild_projection()?;
        self.persist_projection_snapshot(cache_dir, &snapshot)?;
        Ok(snapshot)
    }

    fn relative_path(&self, path: &Path) -> RefineResult<String> {
        path.strip_prefix(&self.durable_root)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .map_err(|error| {
                RefineError::InvalidInput(format!(
                    "path {} is not under durable root {}: {error}",
                    path.display(),
                    self.durable_root.display()
                ))
            })
    }

    fn collect_json_files(root: &Path, file_name: &str) -> RefineResult<Vec<PathBuf>> {
        let mut files = Vec::new();
        if !root.exists() {
            return Ok(files);
        }
        Self::collect_json_files_inner(root, file_name, &mut files)?;
        files.sort();
        Ok(files)
    }

    fn collect_json_files_inner(
        root: &Path,
        file_name: &str,
        files: &mut Vec<PathBuf>,
    ) -> RefineResult<()> {
        for entry in fs::read_dir(root).map_err(|error| {
            RefineError::Io(format!(
                "failed to read directory {}: {error}",
                root.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                RefineError::Io(format!("failed to read directory entry: {error}"))
            })?;
            let path = entry.path();
            let metadata = entry.metadata().map_err(|error| {
                RefineError::Io(format!("failed to stat {}: {error}", path.display()))
            })?;
            if metadata.is_dir() {
                Self::collect_json_files_inner(&path, file_name, files)?;
            } else if metadata.is_file()
                && path.file_name().and_then(|name| name.to_str()) == Some(file_name)
            {
                files.push(path);
            }
        }
        Ok(())
    }

    fn read_json(path: &Path) -> RefineResult<Value> {
        let bytes = fs::read(path).map_err(|error| {
            RefineError::Io(format!("failed to read {}: {error}", path.display()))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
        })
    }

    fn project_gap(&self, path: &Path) -> RefineResult<Option<GapSummaryProjection>> {
        let value = Self::read_json(path)?;
        let Some(object) = value.as_object() else {
            return Ok(None);
        };
        let id = text(object.get("id")).unwrap_or_default();
        if id.is_empty() {
            return Ok(None);
        }
        let rel_path = self.relative_path(path)?;
        let rounds = object
            .get("rounds")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let valid_rounds: Vec<&Value> = rounds.iter().filter(|round| round.is_object()).collect();
        let reporter = valid_rounds.last().and_then(|round| {
            round
                .as_object()
                .and_then(|object| text(object.get("reporter")))
        });
        let notes = object
            .get("notes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut searchable_parts = vec![
            text(object.get("name")).unwrap_or_else(|| "Untitled Gap".to_string()),
            reporter.clone().unwrap_or_default(),
        ];
        for note in notes.iter().filter_map(Value::as_object) {
            if let Some(body) = text(note.get("body")) {
                searchable_parts.push(body);
            }
        }
        for round in &valid_rounds {
            if let Some(round) = round.as_object() {
                for key in ["reporter", "actual", "target"] {
                    if let Some(value) = text(round.get(key)) {
                        searchable_parts.push(value);
                    }
                }
            }
        }

        Ok(Some(GapSummaryProjection {
            gap: GapIndexProjection {
                id,
                name: text(object.get("name")).unwrap_or_else(|| "Untitled Gap".to_string()),
                status: gap_status(object.get("status")),
                priority: gap_priority(object.get("priority")),
                reporter,
                round_count: valid_rounds.len(),
                created: text(object.get("created")).unwrap_or_else(|| "unknown".to_string()),
                updated: text(object.get("updated"))
                    .or_else(|| text(object.get("created")))
                    .unwrap_or_else(|| "unknown".to_string()),
                branch_name: nullable_text(object.get("branch_name")),
                node_id: Some(
                    nullable_text(object.get("node_id"))
                        .or_else(|| nullable_text(object.get("instance_id")))
                        .unwrap_or_else(|| "default".to_string()),
                ),
                feature_id: nullable_text(object.get("feature_id")),
                feature_order: nullable_i64(object.get("feature_order")),
                json_path: rel_path,
            },
            node_display_name: None,
            searchable_text: searchable_parts.join("\n"),
            activity_ids: Vec::new(),
        }))
    }

    fn project_feature(&self, path: &Path) -> RefineResult<Option<FeatureIndexProjection>> {
        let value = Self::read_json(path)?;
        let Some(object) = value.as_object() else {
            return Ok(None);
        };
        let id = text(object.get("id")).unwrap_or_default();
        if id.is_empty() {
            return Ok(None);
        }
        Ok(Some(FeatureIndexProjection {
            id,
            name: text(object.get("name")).unwrap_or_else(|| "Untitled Feature".to_string()),
            description: Some(text(object.get("description")).unwrap_or_default()),
            reporter: Some(text(object.get("reporter")).unwrap_or_default()),
            node_id: Some(
                nullable_text(object.get("node_id")).unwrap_or_else(|| "default".to_string()),
            ),
            created: text(object.get("created")).unwrap_or_else(|| "unknown".to_string()),
            updated: text(object.get("updated"))
                .or_else(|| text(object.get("created")))
                .unwrap_or_else(|| "unknown".to_string()),
            json_path: self.relative_path(path)?,
        }))
    }

    fn project_activity(&self) -> RefineResult<BTreeMap<String, ActivitySummaryProjection>> {
        let path = self.durable_root.join(ACTIVITY_LOG_FILE);
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let file = fs::File::open(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to open activity log {}: {error}",
                path.display()
            ))
        })?;
        let mut activity = BTreeMap::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|error| {
                RefineError::Io(format!(
                    "failed to read activity log {}: {error}",
                    path.display()
                ))
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str::<ActivityEntry>(&line).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse activity entry in {}: {error}",
                    path.display()
                ))
            })?;
            if entry.id.trim().is_empty() {
                continue;
            }
            let searchable_text = activity_searchable_text(&entry);
            activity.insert(
                entry.id.clone(),
                ActivitySummaryProjection {
                    entry,
                    searchable_text,
                },
            );
        }
        Ok(activity)
    }

    fn project_changes(
        &self,
        gaps: &BTreeMap<String, GapSummaryProjection>,
    ) -> BTreeMap<String, ChangeSummaryProjection> {
        let Some(source_root) = self.source_root() else {
            return BTreeMap::new();
        };
        let service = FileGitWorktreeService::new(source_root);
        let branch = service.inspect("").ok().and_then(|status| status.branch);
        let Ok(changes) = service.recent_changes(1000) else {
            return BTreeMap::new();
        };
        changes
            .into_iter()
            .enumerate()
            .map(|(order, change)| {
                let branch = change.branch.or_else(|| branch.clone());
                let joined_gap = matching_change_gap(gaps, branch.as_deref(), &change.subject);
                let projection = ChangeSummaryProjection {
                    commit: change.commit,
                    committed_time: change.committed_time,
                    subject: change.subject,
                    gap_id: joined_gap.map(|gap| gap.gap.id.clone()),
                    branch,
                    gap_name: joined_gap.map(|gap| gap.gap.name.clone()),
                    gap_status: joined_gap.map(|gap| gap.gap.status.clone()),
                    gap_priority: joined_gap.map(|gap| gap.gap.priority.as_str().to_string()),
                    searchable_text: String::new(),
                    order,
                };
                let mut projection = projection;
                projection.searchable_text = change_searchable_text(&projection);
                (change_projection_key(&projection), projection)
            })
            .collect()
    }

    fn source_root(&self) -> Option<PathBuf> {
        self.durable_root.parent().map(Path::to_path_buf)
    }

    fn git_head_fingerprint(&self) -> Option<SourceFingerprint> {
        let source_root = self.source_root()?;
        let service = FileGitWorktreeService::new(source_root);
        let branch = service
            .inspect("")
            .ok()
            .and_then(|status| status.branch)
            .unwrap_or_default();
        let latest = service
            .recent_changes(1)
            .ok()
            .and_then(|changes| changes.into_iter().next())
            .map(|change| change.commit)
            .unwrap_or_default();
        if branch.is_empty() && latest.is_empty() {
            return None;
        }
        Some(SourceFingerprint {
            path: "git:HEAD".to_string(),
            size: latest.len() as u64,
            modified_unix_ms: None,
            content_hash: Some(format!("{branch}:{latest}")),
        })
    }
}

impl ProjectStateStore for FileProjectStateStore {
    fn initialize(&self) -> RefineResult<()> {
        fs::create_dir_all(&self.durable_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to initialize durable root {}: {error}",
                self.durable_root.display()
            ))
        })
    }

    fn load_projection_snapshot(
        &self,
        cache_dir: &Path,
    ) -> RefineResult<Option<ProjectionSnapshot>> {
        let snapshot_path = Self::snapshot_path(cache_dir);
        if !snapshot_path.exists() {
            return Ok(None);
        }

        let bytes = fs::read(&snapshot_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read projection snapshot {}: {error}",
                snapshot_path.display()
            ))
        })?;
        let snapshot: ProjectionSnapshot = serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse projection snapshot {}: {error}",
                snapshot_path.display()
            ))
        })?;

        if snapshot.version == PROJECTION_SNAPSHOT_VERSION {
            Ok(Some(snapshot))
        } else {
            Ok(None)
        }
    }

    fn persist_projection_snapshot(
        &self,
        cache_dir: &Path,
        snapshot: &ProjectionSnapshot,
    ) -> RefineResult<()> {
        fs::create_dir_all(cache_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create projection cache dir {}: {error}",
                cache_dir.display()
            ))
        })?;
        let snapshot_path = Self::snapshot_path(cache_dir);
        let temp_path = snapshot_path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|error| {
            RefineError::Serialization(format!("failed to encode projection snapshot: {error}"))
        })?;

        fs::write(&temp_path, bytes).map_err(|error| {
            RefineError::Io(format!(
                "failed to write projection snapshot temp file {}: {error}",
                temp_path.display()
            ))
        })?;
        fs::rename(&temp_path, &snapshot_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to commit projection snapshot {}: {error}",
                snapshot_path.display()
            ))
        })
    }

    fn rebuild_projection(&self) -> RefineResult<ProjectionSnapshot> {
        let mut source_fingerprints = BTreeMap::new();
        let mut gaps = BTreeMap::new();
        let mut features = BTreeMap::new();
        let gap_paths = Self::collect_json_files(&self.durable_root.join("gaps"), "gap.json")?;
        let feature_paths =
            Self::collect_json_files(&self.durable_root.join("features"), "feature.json")?;
        let activity_path = self.durable_root.join(ACTIVITY_LOG_FILE);

        for path in gap_paths {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path.clone(), Self::fingerprint(&path)?);
            if let Some(projection) = self.project_gap(&path)? {
                gaps.insert(projection.gap.id.clone(), projection);
            }
        }

        for path in feature_paths {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path.clone(), Self::fingerprint(&path)?);
            if let Some(feature) = self.project_feature(&path)? {
                let mut feature_gaps: Vec<GapIndexProjection> = gaps
                    .values()
                    .filter(|gap| gap.gap.feature_id.as_deref() == Some(feature.id.as_str()))
                    .map(|gap| gap.gap.clone())
                    .collect();
                feature_gaps.sort_by(|a, b| {
                    a.feature_order
                        .unwrap_or(i64::MAX)
                        .cmp(&b.feature_order.unwrap_or(i64::MAX))
                        .then_with(|| a.id.cmp(&b.id))
                });
                let rollup = FeatureRollup::derive(&feature_gaps);
                let gap_ids = feature_gaps.into_iter().map(|gap| gap.id).collect();
                features.insert(
                    feature.id.clone(),
                    FeatureSummaryProjection {
                        feature,
                        status: rollup.status.clone(),
                        gap_ids,
                        rollup,
                    },
                );
            }
        }

        let activity = self.project_activity()?;
        if activity_path.exists() {
            let rel_path = self.relative_path(&activity_path)?;
            source_fingerprints.insert(rel_path, Self::fingerprint(&activity_path)?);
        }
        if let Some(fingerprint) = self.git_head_fingerprint() {
            source_fingerprints.insert(fingerprint.path.clone(), fingerprint);
        }
        for (activity_id, projection) in &activity {
            if let Some(gap_id) = projection.entry.gap_id.as_deref()
                && let Some(gap) = gaps.get_mut(gap_id)
            {
                gap.activity_ids.push(activity_id.clone());
            }
        }
        let mut recent_activity = activity.values().collect::<Vec<_>>();
        recent_activity.sort_by(|a, b| {
            b.entry
                .datetime
                .cmp(&a.entry.datetime)
                .then_with(|| b.entry.id.cmp(&a.entry.id))
        });
        let recent_activity_ids = recent_activity
            .into_iter()
            .take(50)
            .map(|activity| activity.entry.id.clone())
            .collect::<Vec<_>>();

        let all_node_status_counts = gap_status_counts(gaps.values().map(|gap| &gap.gap.status));
        let current_node_status_counts = gap_status_counts(
            gaps.values()
                .filter(|gap| gap.gap.node_id.as_deref().unwrap_or("default") == "default")
                .map(|gap| &gap.gap.status),
        );
        let mut reporter_stats: BTreeMap<String, BTreeMap<GapStatus, usize>> = BTreeMap::new();
        for gap in gaps.values() {
            let reporter = gap
                .gap
                .reporter
                .clone()
                .filter(|reporter| !reporter.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            *reporter_stats
                .entry(reporter)
                .or_default()
                .entry(gap.gap.status.clone())
                .or_default() += 1;
        }
        let failed_count = all_node_status_counts
            .get(&GapStatus::Failed)
            .copied()
            .unwrap_or_default();
        let attention_indicators = if failed_count > 0 {
            vec![format!("{failed_count} failed Gap(s) need recovery")]
        } else {
            Vec::new()
        };
        let changes = self.project_changes(&gaps);

        Ok(ProjectionSnapshot {
            version: PROJECTION_SNAPSHOT_VERSION,
            generated_at: "unknown".to_string(),
            source_fingerprints,
            gaps,
            features,
            activity,
            changes,
            dashboard: DashboardProjection {
                all_node_status_counts,
                current_node_status_counts,
                reporter_stats,
                attention_indicators,
                recent_activity_ids,
            },
            runtime: RuntimeProjection::default(),
        })
    }
}

fn activity_searchable_text(entry: &ActivityEntry) -> String {
    let mut parts = vec![
        entry.message.clone(),
        entry.severity.clone(),
        entry.category.clone(),
    ];
    if let Some(actor) = &entry.actor {
        parts.push(actor.clone());
    }
    if let Some(gap_id) = &entry.gap_id {
        parts.push(gap_id.clone());
    }
    if let Some(details) = &entry.details
        && let Ok(encoded) = serde_json::to_string(details)
    {
        parts.push(encoded);
    }
    parts.join("\n")
}

fn change_searchable_text(change: &ChangeSummaryProjection) -> String {
    [
        Some(change.commit.as_str()),
        Some(change.subject.as_str()),
        change.branch.as_deref(),
        change.gap_id.as_deref(),
        change.gap_name.as_deref(),
        change.gap_priority.as_deref(),
        change.gap_status.as_ref().map(GapStatus::as_str),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n")
}

fn change_projection_key(change: &ChangeSummaryProjection) -> String {
    format!(
        "{}:{}",
        change.branch.as_deref().unwrap_or(""),
        change.commit
    )
}

fn matching_change_gap<'a>(
    gaps: &'a BTreeMap<String, GapSummaryProjection>,
    branch: Option<&str>,
    subject: &str,
) -> Option<&'a GapSummaryProjection> {
    gaps.values().find(|gap| {
        branch.is_some_and(|branch| gap.gap.branch_name.as_deref() == Some(branch))
            || subject.contains(&gap.gap.id)
            || branch.is_some_and(|branch| branch.contains(&gap.gap.id))
    })
}

fn fingerprint_content_hash(path: &Path) -> RefineResult<String> {
    let bytes = fs::read(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read {} for fingerprint: {error}",
            path.display()
        ))
    })?;
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(format!("{hash:016x}"))
}

fn text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(Value::Bool(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn nullable_text(value: Option<&Value>) -> Option<String> {
    text(value).and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn nullable_i64(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(value)) => value.as_i64(),
        Some(Value::String(value)) => value.parse().ok(),
        _ => None,
    }
}

fn gap_status(value: Option<&Value>) -> GapStatus {
    match nullable_text(value).as_deref() {
        Some("todo") => GapStatus::Todo,
        Some("in-progress") => GapStatus::InProgress,
        Some("qa") => GapStatus::Qa,
        Some("ready-merge") => GapStatus::ReadyMerge,
        Some("awaiting-rebuild") => GapStatus::AwaitingRebuild,
        Some("review") => GapStatus::Review,
        Some("done") => GapStatus::Done,
        Some("failed") => GapStatus::Failed,
        Some("cancelled") => GapStatus::Cancelled,
        _ => GapStatus::Backlog,
    }
}

fn gap_priority(value: Option<&Value>) -> GapPriority {
    match nullable_text(value).as_deref() {
        Some("medium") => GapPriority::Medium,
        Some("high") => GapPriority::High,
        _ => GapPriority::Low,
    }
}

fn gap_status_counts<'a>(
    statuses: impl Iterator<Item = &'a GapStatus>,
) -> BTreeMap<GapStatus, usize> {
    let mut counts = BTreeMap::new();
    for status in statuses {
        *counts.entry(status.clone()).or_default() += 1;
    }
    counts
}

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
    if let Some(node) = query
        .node
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        match node {
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
    if let Some(node) = query
        .node
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        match node {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::feature::FeatureIndexProjection;
    use crate::model::gap::{GapIndexProjection, GapPriority};
    use crate::model::log::ActivityEntry;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn projection_query_counts_gap_statuses() {
        let mut gaps = BTreeMap::new();
        gaps.insert(
            "gap-1".to_string(),
            gap_projection("gap-1", GapStatus::Todo, Some("node-a")),
        );
        gaps.insert(
            "gap-2".to_string(),
            gap_projection("gap-2", GapStatus::Todo, Some("node-a")),
        );
        gaps.insert(
            "gap-3".to_string(),
            gap_projection("gap-3", GapStatus::Done, None),
        );

        let snapshot = ProjectionSnapshot {
            version: PROJECTION_SNAPSHOT_VERSION,
            generated_at: "now".to_string(),
            source_fingerprints: BTreeMap::new(),
            gaps,
            features: BTreeMap::new(),
            activity: BTreeMap::new(),
            changes: BTreeMap::new(),
            dashboard: DashboardProjection::default(),
            runtime: RuntimeProjection::default(),
        };

        let counts = snapshot.status_counts();
        assert_eq!(counts.get(&GapStatus::Todo), Some(&2));
        assert_eq!(counts.get(&GapStatus::Done), Some(&1));

        let index = ProjectionIndex::build(&snapshot);
        assert_eq!(index.gaps_by_node["node-a"].len(), 2);
        assert!(index.standalone_gap_ids.contains("gap-3"));
    }

    #[test]
    fn projection_query_filters_sorts_and_pages_gaps_and_features() {
        let mut gap_one = gap_projection("gap-1", GapStatus::Todo, Some("default"));
        gap_one.gap.name = "OAuth callback broken".to_string();
        gap_one.gap.reporter = Some("Alice".to_string());
        gap_one.gap.round_count = 2;
        gap_one.gap.feature_id = Some("feature-1".to_string());
        gap_one.gap.priority = GapPriority::High;
        gap_one.searchable_text = "OAuth callback broken login notes".to_string();
        gap_one.activity_ids = vec!["act-1".to_string()];

        let mut gap_two = gap_projection("gap-2", GapStatus::Done, Some("node-b"));
        gap_two.gap.name = "Settings polish".to_string();
        gap_two.gap.reporter = Some("Bob".to_string());
        gap_two.gap.round_count = 1;

        let mut gaps = BTreeMap::new();
        gaps.insert(gap_one.gap.id.clone(), gap_one);
        gaps.insert(gap_two.gap.id.clone(), gap_two);

        let mut activity = BTreeMap::new();
        activity.insert(
            "act-1".to_string(),
            ActivitySummaryProjection {
                entry: ActivityEntry {
                    id: "act-1".to_string(),
                    datetime: "2026-01-01T00:00:00Z".to_string(),
                    severity: "error".to_string(),
                    category: "quality".to_string(),
                    message: "OAuth failed".to_string(),
                    gap_id: Some("gap-1".to_string()),
                    actor: Some("browser".to_string()),
                    details: None,
                    actions: Vec::new(),
                },
                searchable_text: "OAuth failed".to_string(),
            },
        );

        let feature = FeatureSummaryProjection {
            feature: FeatureIndexProjection {
                id: "feature-1".to_string(),
                name: "Auth work".to_string(),
                description: Some("OAuth fixes".to_string()),
                reporter: Some("Alice".to_string()),
                node_id: Some("default".to_string()),
                created: "created".to_string(),
                updated: "updated".to_string(),
                json_path: "feature.json".to_string(),
            },
            status: GapStatus::Todo,
            gap_ids: vec!["gap-1".to_string()],
            rollup: FeatureRollup {
                status: GapStatus::Todo,
                gap_count: 1,
                done_count: 0,
                active_count: 0,
                failed_count: 0,
                cancelled_count: 0,
                blocked_count: 0,
                next_gap: Some("gap-1".to_string()),
            },
        };
        let mut features = BTreeMap::new();
        features.insert("feature-1".to_string(), feature);

        let snapshot = ProjectionSnapshot {
            version: PROJECTION_SNAPSHOT_VERSION,
            generated_at: "now".to_string(),
            source_fingerprints: BTreeMap::new(),
            gaps,
            features,
            activity,
            changes: BTreeMap::new(),
            dashboard: DashboardProjection::default(),
            runtime: RuntimeProjection::default(),
        };

        let gaps = snapshot.list_gaps(GapProjectionQuery {
            q: Some("oauth".to_string()),
            feature: Some("feature-1".to_string()),
            severity: Some("error".to_string()),
            category: Some("quality".to_string()),
            actor: Some("browser".to_string()),
            rounds_gte: Some(2),
            page: PageRequest {
                sort: "priority".to_string(),
                dir: "desc".to_string(),
                ..PageRequest::default()
            },
            ..GapProjectionQuery::default()
        });
        assert_eq!(gaps.total, 1);
        assert_eq!(gaps.gaps[0].id, "gap-1");
        assert_eq!(gaps.filtered_status_counts.get(&GapStatus::Todo), Some(&1));
        assert_eq!(gaps.matching_ids, vec!["gap-1"]);

        let activity = snapshot.list_activity(ActivityProjectionQuery {
            q: Some("oauth".to_string()),
            severity: Some("error".to_string()),
            category: Some("quality".to_string()),
            actor: Some("browser".to_string()),
            page: PageRequest {
                sort: "message".to_string(),
                dir: "asc".to_string(),
                ..PageRequest::default()
            },
            ..ActivityProjectionQuery::default()
        });
        assert_eq!(activity.total, 1);
        assert_eq!(activity.activity[0].id, "act-1");
        assert_eq!(activity.matching_ids, vec!["act-1"]);
        assert_eq!(activity.facets.categories, vec!["quality"]);
        assert_eq!(activity.facets.severities, vec!["error"]);
        assert_eq!(activity.facets.actors, vec!["browser"]);

        let features = snapshot.list_features(FeatureProjectionQuery {
            q: Some("oauth".to_string()),
            reporter: Some("Alice".to_string()),
            status: Some(GapStatus::Todo),
            node: Some("current".to_string()),
            current_node_id: Some("default".to_string()),
            page: PageRequest::default(),
        });
        assert_eq!(features.total, 1);
        assert_eq!(features.features[0].feature.id, "feature-1");
    }

    #[test]
    fn file_store_persists_and_loads_projection_snapshot() {
        let temp_root = unique_temp_dir("projection-store");
        let durable_root = temp_root.join("durable");
        let cache_dir = temp_root.join("run").join("8080").join("cache");
        let store = FileProjectStateStore::new(&durable_root);
        store.initialize().unwrap();

        let mut gaps = BTreeMap::new();
        gaps.insert(
            "gap-1".to_string(),
            gap_projection("gap-1", GapStatus::Todo, Some("node-a")),
        );
        let snapshot = ProjectionSnapshot {
            version: PROJECTION_SNAPSHOT_VERSION,
            generated_at: "now".to_string(),
            source_fingerprints: BTreeMap::new(),
            gaps,
            features: BTreeMap::new(),
            activity: BTreeMap::new(),
            changes: BTreeMap::new(),
            dashboard: DashboardProjection::default(),
            runtime: RuntimeProjection::default(),
        };

        store
            .persist_projection_snapshot(&cache_dir, &snapshot)
            .unwrap();
        let loaded = store.load_projection_snapshot(&cache_dir).unwrap().unwrap();
        assert_eq!(loaded.gaps.len(), 1);
        assert_eq!(loaded.version, PROJECTION_SNAPSHOT_VERSION);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_store_ignores_incompatible_snapshot_versions() {
        let temp_root = unique_temp_dir("projection-version");
        let cache_dir = temp_root.join("run").join("8080").join("cache");
        let store = FileProjectStateStore::new(temp_root.join("durable"));
        let mut snapshot = store.rebuild_projection().unwrap();
        snapshot.version = PROJECTION_SNAPSHOT_VERSION + 1;

        store
            .persist_projection_snapshot(&cache_dir, &snapshot)
            .unwrap();
        assert!(
            store
                .load_projection_snapshot(&cache_dir)
                .unwrap()
                .is_none()
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_store_loads_cached_projection_until_fingerprints_change() {
        let temp_root = unique_temp_dir("projection-refresh");
        let durable_root = temp_root.join(".refine");
        let cache_dir = temp_root.join("run").join("8080").join("cache");
        let gap_dir = durable_root.join("gaps").join("GA").join("P1");
        fs::create_dir_all(&gap_dir).unwrap();
        fs::write(
            gap_dir.join("gap.json"),
            r#"{
              "id": "GAP1",
              "name": "Cached name",
              "status": "todo",
              "rounds": []
            }"#,
        )
        .unwrap();
        let store = FileProjectStateStore::new(&durable_root);
        let mut snapshot = store.load_or_refresh_projection(&cache_dir).unwrap();
        assert_eq!(snapshot.gaps["GAP1"].gap.name, "Cached name");

        snapshot.generated_at = "cached-sentinel".to_string();
        store
            .persist_projection_snapshot(&cache_dir, &snapshot)
            .unwrap();
        let cached = store.load_or_refresh_projection(&cache_dir).unwrap();
        assert_eq!(cached.generated_at, "cached-sentinel");

        fs::write(
            gap_dir.join("gap.json"),
            r#"{
              "id": "GAP1",
              "name": "Refreshed name with changed durable content",
              "status": "todo",
              "rounds": []
            }"#,
        )
        .unwrap();
        let refreshed = store.load_or_refresh_projection(&cache_dir).unwrap();
        assert_eq!(
            refreshed.gaps["GAP1"].gap.name,
            "Refreshed name with changed durable content"
        );
        assert_ne!(refreshed.generated_at, "cached-sentinel");

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn rebuild_projection_scans_python_style_gap_and_feature_records() {
        let temp_root = unique_temp_dir("projection-rebuild");
        let durable_root = temp_root.join(".refine");
        let gap_dir = durable_root.join("gaps").join("01").join("GAP1");
        let remote_gap_dir = durable_root.join("gaps").join("02").join("GAP2");
        let feature_dir = durable_root.join("features").join("01").join("FEATURE1");
        fs::create_dir_all(&gap_dir).unwrap();
        fs::create_dir_all(&remote_gap_dir).unwrap();
        fs::create_dir_all(&feature_dir).unwrap();
        fs::create_dir_all(durable_root.join("logs")).unwrap();
        fs::write(
            gap_dir.join("gap.json"),
            r#"{
              "id": "GAP1",
              "name": "Fix login",
              "status": "todo",
              "priority": "high",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "feature_id": "FEATURE1",
              "feature_order": 2,
              "rounds": [
                {"reporter": "Buddy", "actual": "Broken", "target": "Works"}
              ],
              "notes": [{"body": "OAuth path"}]
            }"#,
        )
        .unwrap();
        fs::write(
            remote_gap_dir.join("gap.json"),
            r#"{
              "id": "GAP2",
              "name": "Remote failure",
              "status": "failed",
              "priority": "medium",
              "node_id": "node-b",
              "rounds": [{"reporter": "Remote", "actual": "Broken", "target": "Fixed"}],
              "notes": []
            }"#,
        )
        .unwrap();
        fs::write(
            feature_dir.join("feature.json"),
            r#"{
              "id": "FEATURE1",
              "name": "Authentication",
              "description": "Login work",
              "reporter": "Buddy",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z"
            }"#,
        )
        .unwrap();
        fs::write(
            durable_root.join(ACTIVITY_LOG_FILE),
            concat!(
                "{\"id\":\"act-1\",\"datetime\":\"2026-01-03T00:00:00Z\",\"severity\":\"error\",\"category\":\"quality\",\"message\":\"Remote QA failed\",\"gap_id\":\"GAP2\",\"actor\":\"browser\",\"details\":{\"selector\":\"#app\"},\"actions\":[]}\n",
                "{\"id\":\"act-2\",\"datetime\":\"2026-01-04T00:00:00Z\",\"severity\":\"info\",\"category\":\"state\",\"message\":\"Feature changed\",\"gap_id\":null,\"actor\":\"system\",\"details\":null,\"actions\":[]}\n"
            ),
        )
        .unwrap();

        let snapshot = FileProjectStateStore::new(&durable_root)
            .rebuild_projection()
            .unwrap();
        let gap = &snapshot.gaps["GAP1"];
        assert_eq!(gap.gap.status, GapStatus::Todo);
        assert_eq!(gap.gap.priority, GapPriority::High);
        assert_eq!(gap.gap.reporter.as_deref(), Some("Buddy"));
        assert_eq!(gap.gap.round_count, 1);
        assert_eq!(gap.gap.node_id.as_deref(), Some("default"));
        assert!(gap.searchable_text.contains("OAuth path"));

        let feature = &snapshot.features["FEATURE1"];
        assert_eq!(feature.gap_ids, vec!["GAP1"]);
        assert_eq!(feature.rollup.gap_count, 1);
        assert_eq!(feature.rollup.next_gap.as_deref(), Some("GAP1"));
        assert!(
            snapshot
                .source_fingerprints
                .contains_key("gaps/01/GAP1/gap.json")
        );
        assert!(
            snapshot.source_fingerprints["gaps/01/GAP1/gap.json"]
                .content_hash
                .is_some()
        );
        assert_eq!(
            snapshot
                .dashboard
                .all_node_status_counts
                .get(&GapStatus::Todo),
            Some(&1)
        );
        assert_eq!(
            snapshot
                .dashboard
                .all_node_status_counts
                .get(&GapStatus::Failed),
            Some(&1)
        );
        assert_eq!(
            snapshot
                .dashboard
                .current_node_status_counts
                .get(&GapStatus::Todo),
            Some(&1)
        );
        assert_eq!(
            snapshot
                .dashboard
                .current_node_status_counts
                .get(&GapStatus::Failed),
            None
        );
        assert_eq!(snapshot.dashboard.attention_indicators.len(), 1);
        assert_eq!(snapshot.activity.len(), 2);
        assert_eq!(snapshot.gaps["GAP2"].activity_ids, vec!["act-1"]);
        assert!(snapshot.activity["act-1"].searchable_text.contains("#app"));
        assert_eq!(
            snapshot.dashboard.recent_activity_ids,
            vec!["act-2".to_string(), "act-1".to_string()]
        );
        assert!(
            snapshot
                .source_fingerprints
                .contains_key("logs/activity.jsonl")
        );
        let activity_filtered = snapshot.list_gaps(GapProjectionQuery {
            severity: Some("error".to_string()),
            category: Some("quality".to_string()),
            actor: Some("browser".to_string()),
            ..GapProjectionQuery::default()
        });
        assert_eq!(activity_filtered.matching_ids, vec!["GAP2"]);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn rebuild_projection_scans_git_changes_and_joins_gap_display_fields() {
        let temp_root = unique_temp_dir("projection-changes");
        let durable_root = temp_root.join(".refine");
        let gap_dir = durable_root.join("gaps").join("GA").join("P1");
        fs::create_dir_all(&gap_dir).unwrap();
        git(&temp_root, &["init"]).unwrap();
        git(&temp_root, &["config", "user.email", "test@example.com"]).unwrap();
        git(&temp_root, &["config", "user.name", "Test User"]).unwrap();
        fs::write(temp_root.join("app.txt"), "one\n").unwrap();
        git(&temp_root, &["add", "app.txt"]).unwrap();
        git(&temp_root, &["commit", "-m", "initial"]).unwrap();
        fs::write(
            gap_dir.join("gap.json"),
            r#"{
              "id": "GAP1",
              "name": "Change-linked Gap",
              "status": "done",
              "priority": "high",
              "branch_name": "main",
              "created": "2026-01-01T00:00:00Z",
              "updated": "2026-01-02T00:00:00Z",
              "rounds": []
            }"#,
        )
        .unwrap();
        fs::write(temp_root.join("app.txt"), "two\n").unwrap();
        git(&temp_root, &["commit", "-am", "GAP1 update app"]).unwrap();

        let snapshot = FileProjectStateStore::new(&durable_root)
            .rebuild_projection()
            .unwrap();
        assert!(snapshot.source_fingerprints.contains_key("git:HEAD"));
        let changes = snapshot.list_changes(ChangeProjectionQuery {
            q: Some("GAP1 update".to_string()),
            gap_id: Some("GAP1".to_string()),
            status: Some(GapStatus::Done),
            priority: Some("high".to_string()),
            page: PageRequest::default(),
            ..ChangeProjectionQuery::default()
        });
        assert_eq!(changes.total, 1);
        assert_eq!(changes.changes[0].gap_id.as_deref(), Some("GAP1"));
        assert_eq!(
            changes.changes[0].gap_name.as_deref(),
            Some("Change-linked Gap")
        );
        assert_eq!(changes.changes[0].gap_status, Some(GapStatus::Done));
        assert_eq!(changes.changes[0].gap_priority.as_deref(), Some("high"));

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn gap_projection(id: &str, status: GapStatus, node_id: Option<&str>) -> GapSummaryProjection {
        GapSummaryProjection {
            gap: GapIndexProjection {
                id: id.to_string(),
                name: id.to_string(),
                status,
                priority: GapPriority::Medium,
                reporter: None,
                round_count: 0,
                created: "created".to_string(),
                updated: "updated".to_string(),
                branch_name: None,
                node_id: node_id.map(str::to_string),
                feature_id: None,
                feature_order: None,
                json_path: format!("{id}/gap.json"),
            },
            node_display_name: None,
            searchable_text: id.to_string(),
            activity_ids: Vec::new(),
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn git(root: &Path, args: &[&str]) -> RefineResult<()> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RefineError::Conflict(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ))
        }
    }
}

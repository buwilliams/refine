use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::core::host::git_worktrees::{FileGitWorktreeService, GitWorktreeService};
use crate::core::observability::activity::ACTIVITY_LOG_FILE;
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::feature::{FeatureIndexProjection, FeatureRollup};
use crate::model::gap::GapIndexProjection;
use crate::model::log::ActivityEntry;
use crate::model::workflow::GapStatus;

use super::helpers::*;
use super::types::*;

static PROJECTION_SNAPSHOT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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

    fn snapshot_temp_path(cache_dir: &Path) -> PathBuf {
        let counter = PROJECTION_SNAPSHOT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        cache_dir.join(format!(
            ".{PROJECTION_SNAPSHOT_FILE}.{}.{}.{}.tmp",
            std::process::id(),
            timestamp,
            counter
        ))
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
        let temp_path = Self::snapshot_temp_path(cache_dir);
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

use super::*;

impl FileProjectProjectionStore {
    pub fn load_projection_snapshot(
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
        // Projection snapshots are derived cache data, not durable project state.
        // Inspect the version before deserializing the current schema so an older
        // snapshot that lacks newly required fields is treated as a cache miss.
        // Malformed or incomplete snapshots are likewise safe to rebuild.
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        if value.get("version").and_then(Value::as_u64) != Some(PROJECTION_SNAPSHOT_VERSION) {
            return Ok(None);
        }
        match serde_json::from_value::<ProjectionSnapshot>(value) {
            // The source directory is not serialized, so a snapshot read back
            // from cache has to be pointed at its records again before text
            // search can consult them.
            Ok(mut snapshot) => {
                snapshot.refine_dir = Some(self.refine_dir.clone());
                Ok(Some(snapshot))
            }
            Err(_) => Ok(None),
        }
    }

    pub fn persist_projection_snapshot(
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
        })?;
        // The in-memory tier must observe every explicit persist, or a reader
        // whose source fingerprints still match would keep serving the
        // pre-persist snapshot.
        self.store_shared_snapshot(cache_dir, Arc::new(snapshot.clone()));
        Ok(())
    }

    pub fn rebuild_projection(&self) -> RefineResult<ProjectionSnapshot> {
        #[cfg(test)]
        {
            *PROJECTION_REBUILD_COUNTS
                .get_or_init(|| Mutex::new(BTreeMap::new()))
                .lock()
                .unwrap()
                .entry(self.refine_dir.clone())
                .or_default() += 1;
        }
        let mut source_fingerprints = BTreeMap::new();
        let mut goals = BTreeMap::new();
        let mut features = BTreeMap::new();
        let goal_paths = Self::collect_json_files(&self.refine_dir.join("goals"), "goal.json")?;
        let feature_paths =
            Self::collect_json_files(&self.refine_dir.join("features"), "feature.json")?;
        let activity_path = self.refine_dir.join(ACTIVITY_LOG_FILE);

        for path in goal_paths {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path.clone(), Self::metadata_fingerprint(&path)?);
            if let Some(projection) = self.project_goal(&path)? {
                goals.insert(projection.goal.id.clone(), projection);
            }
        }

        let mut feature_records = Vec::new();
        for path in feature_paths {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path.clone(), Self::metadata_fingerprint(&path)?);
            if let Some(feature) = self.project_feature(&path)? {
                feature_records.push(feature);
            }
        }
        features.extend(derive_features(&goals, feature_records));

        let mut activity = self.project_activity()?;
        activity.extend(self.project_goal_round_activity(&goals)?);
        if activity_path.exists() {
            let rel_path = self.relative_path(&activity_path)?;
            source_fingerprints.insert(rel_path, Self::metadata_fingerprint(&activity_path)?);
        }
        for path in Self::collect_json_files(&goal_logs_root(&self.refine_dir), "logs.jsonl")? {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        if let Some(fingerprint) = self.git_head_fingerprint() {
            source_fingerprints.insert(fingerprint.path.clone(), fingerprint);
        }
        attach_activity_ids(&mut goals, &activity);
        let dashboard = derive_dashboard(&goals, &activity);
        let changes = self.project_changes(&goals);

        Ok(ProjectionSnapshot {
            refine_dir: Some(self.refine_dir.clone()),
            version: PROJECTION_SNAPSHOT_VERSION,
            generated_at: "unknown".to_string(),
            source_fingerprints,
            goals,
            features,
            activity,
            changes,
            dashboard,
            runtime: RuntimeProjection::default(),
        })
    }
}

/// Rebuilds each Feature's derived rollup from the Goals currently projected.
///
/// Shared by full rebuilds and incremental patches: a Goal changing status
/// changes its Feature's rollup, and rederiving from the in-memory Goal map
/// costs no I/O.
pub(super) fn derive_features(
    goals: &BTreeMap<String, GoalSummaryProjection>,
    feature_records: impl IntoIterator<Item = FeatureIndexProjection>,
) -> BTreeMap<String, FeatureSummaryProjection> {
    // Grouped once up front: filtering the full Goal map per Feature made this
    // O(Features × Goals) on every incremental patch.
    let mut goals_by_feature: BTreeMap<&str, Vec<&GoalIndexProjection>> = BTreeMap::new();
    for goal in goals.values() {
        if let Some(feature_id) = goal.goal.feature_id.as_deref() {
            goals_by_feature.entry(feature_id).or_default().push(&goal.goal);
        }
    }
    let mut features = BTreeMap::new();
    for feature in feature_records {
        let mut feature_goals: Vec<GoalIndexProjection> = goals_by_feature
            .get(feature.id.as_str())
            .map(|feature_goals| {
                feature_goals
                    .iter()
                    .map(|goal| (*goal).clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        feature_goals.sort_by(|a, b| {
            compare_feature_goal_order(a.feature_order, b.feature_order)
                .then_with(|| a.id.cmp(&b.id))
        });
        let rollup = FeatureRollup::derive(&feature_goals);
        let goal_ids = feature_goals.into_iter().map(|goal| goal.id).collect();
        features.insert(
            feature.id.clone(),
            FeatureSummaryProjection {
                feature,
                status: rollup.status.clone(),
                goal_ids,
                rollup,
            },
        );
    }
    features
}

/// Cross-references activity back onto the Goals it belongs to. Replaces rather
/// than appends so repeated derivation cannot accumulate duplicates.
pub(super) fn attach_activity_ids(
    goals: &mut BTreeMap<String, GoalSummaryProjection>,
    activity: &BTreeMap<String, ActivitySummaryProjection>,
) {
    for goal in goals.values_mut() {
        goal.activity_ids.clear();
    }
    for (activity_id, projection) in activity {
        if let Some(goal_id) = projection.entry.goal_id.as_deref()
            && let Some(goal) = goals.get_mut(goal_id)
        {
            goal.activity_ids.push(activity_id.clone());
        }
    }
}

/// Derives dashboard aggregates from projected Goals and activity. Pure
/// arithmetic over what is already in memory, so an incremental patch can
/// recompute it without touching the filesystem.
pub(super) fn derive_dashboard(
    goals: &BTreeMap<String, GoalSummaryProjection>,
    activity: &BTreeMap<String, ActivitySummaryProjection>,
) -> DashboardProjection {
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

    let all_node_status_counts = goal_status_counts(goals.values().map(|goal| &goal.goal.status));
    let current_node_status_counts = goal_status_counts(
        goals
            .values()
            .filter(|goal| goal.goal.node_id.as_deref().unwrap_or("default") == "default")
            .map(|goal| &goal.goal.status),
    );
    let mut reporter_stats: BTreeMap<String, BTreeMap<GoalStatus, usize>> = BTreeMap::new();
    let mut assignee_stats: BTreeMap<String, BTreeMap<GoalStatus, usize>> = BTreeMap::new();
    for goal in goals.values() {
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
    let failed_count = all_node_status_counts
        .get(&GoalStatus::Failed)
        .copied()
        .unwrap_or_default();
    let attention_indicators = if failed_count > 0 {
        vec![format!("{failed_count} failed Goal(s) need recovery")]
    } else {
        Vec::new()
    };

    DashboardProjection {
        all_node_status_counts,
        current_node_status_counts,
        reporter_stats,
        assignee_stats,
        attention_indicators,
        recent_activity_ids,
    }
}

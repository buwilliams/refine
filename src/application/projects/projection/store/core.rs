use super::*;

impl FileProjectProjectionStore {
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

    pub fn snapshot_path(cache_dir: &Path) -> PathBuf {
        cache_dir.join(PROJECTION_SNAPSHOT_FILE)
    }

    #[cfg(test)]
    pub(crate) fn reset_rebuild_count(refine_dir: &Path) {
        PROJECTION_REBUILD_COUNTS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap()
            .insert(refine_dir.to_path_buf(), 0);
    }

    #[cfg(test)]
    pub(crate) fn rebuild_count(refine_dir: &Path) -> u64 {
        PROJECTION_REBUILD_COUNTS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap()
            .get(refine_dir)
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn snapshot_temp_path(cache_dir: &Path) -> PathBuf {
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

    /// Source fingerprints are metadata only.
    ///
    /// Every source previously carried a content hash that required reading the
    /// whole file, and for Goal log sidecars that read was unbounded. None of it
    /// was ever consulted: `source_fingerprints_match` compares content only for
    /// `git:HEAD`, whose fingerprint is built from a branch ref rather than a
    /// file. Size, mtime, and ctime already decide staleness for real files.
    pub(super) fn metadata_fingerprint(path: &Path) -> RefineResult<SourceFingerprint> {
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
            change_unix_ns: metadata_change_unix_ns(&metadata),
            content_hash: None,
        })
    }

    pub fn source_fingerprints_match(
        &self,
        expected: &BTreeMap<String, SourceFingerprint>,
    ) -> RefineResult<bool> {
        let mut observed = BTreeMap::new();
        for path in Self::collect_json_files(&self.refine_dir.join("goals"), "goal.json")? {
            let rel_path = self.relative_path(&path)?;
            observed.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        for path in Self::collect_json_files(&self.refine_dir.join("features"), "feature.json")? {
            let rel_path = self.relative_path(&path)?;
            observed.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        for path in Self::collect_json_files(&self.refine_dir.join("missions"), "mission.json")? {
            let rel_path = self.relative_path(&path)?;
            observed.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        for path in Self::collect_json_files(&goal_logs_root(&self.refine_dir), "logs.jsonl")? {
            let rel_path = self.relative_path(&path)?;
            observed.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        let activity_path = self.refine_dir.join(ACTIVITY_LOG_FILE);
        if activity_path.exists() {
            let rel_path = self.relative_path(&activity_path)?;
            observed.insert(rel_path, Self::metadata_fingerprint(&activity_path)?);
        }
        if let Some(fingerprint) = self.git_head_fingerprint() {
            observed.insert(fingerprint.path.clone(), fingerprint);
        }
        if observed.len() != expected.len() {
            return Ok(false);
        }
        Ok(observed.iter().all(|(path, current)| {
            expected.get(path).is_some_and(|previous| {
                previous.size == current.size
                    && previous.modified_unix_ms == current.modified_unix_ms
                    && previous.change_unix_ns.is_some()
                    && previous.change_unix_ns == current.change_unix_ns
                    && (path != "git:HEAD" || previous.content_hash == current.content_hash)
            })
        }))
    }

    pub fn collect_source_fingerprints(&self) -> RefineResult<BTreeMap<String, SourceFingerprint>> {
        let mut source_fingerprints = BTreeMap::new();
        for path in Self::collect_json_files(&self.refine_dir.join("goals"), "goal.json")? {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        for path in Self::collect_json_files(&self.refine_dir.join("features"), "feature.json")? {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        for path in Self::collect_json_files(&self.refine_dir.join("missions"), "mission.json")? {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        for path in Self::collect_json_files(&goal_logs_root(&self.refine_dir), "logs.jsonl")? {
            let rel_path = self.relative_path(&path)?;
            source_fingerprints.insert(rel_path, Self::metadata_fingerprint(&path)?);
        }
        let activity_path = self.refine_dir.join(ACTIVITY_LOG_FILE);
        if activity_path.exists() {
            let rel_path = self.relative_path(&activity_path)?;
            source_fingerprints.insert(rel_path, Self::metadata_fingerprint(&activity_path)?);
        }
        if let Some(fingerprint) = self.git_head_fingerprint() {
            source_fingerprints.insert(fingerprint.path.clone(), fingerprint);
        }
        Ok(source_fingerprints)
    }

    /// Returns a current projection, repairing the cached one where possible.
    ///
    /// Any change at all used to discard the cache and re-read every Goal and
    /// Feature record. A Goal write is the most frequent event in a running
    /// fleet, so the full-corpus path was the normal path and its cost scaled
    /// with project size on every mutation. A delta names the sources that
    /// actually moved, so only those are re-read and everything derived is
    /// recomputed in memory. Full rebuilds are left to a cold or unreadable
    /// cache.
    pub fn load_or_refresh_projection(&self, cache_dir: &Path) -> RefineResult<ProjectionSnapshot> {
        self.load_or_refresh_projection_shared(cache_dir)
            .map(|snapshot| snapshot.as_ref().clone())
    }

    /// Shared-ownership variant of [`Self::load_or_refresh_projection`].
    ///
    /// Consults the process-wide in-memory snapshot first, so a current
    /// projection costs one stat per source; the disk snapshot is only read
    /// when this process has never held the projection. Prefer this variant —
    /// the owned one deep-clones the entire snapshot per call.
    pub fn load_or_refresh_projection_shared(
        &self,
        cache_dir: &Path,
    ) -> RefineResult<Arc<ProjectionSnapshot>> {
        let key = self.memory_cache_key(cache_dir);
        if let Some(snapshot) = cached_snapshot(&key) {
            let delta = self.source_delta(&snapshot.source_fingerprints)?;
            if delta.is_empty() {
                return Ok(snapshot);
            }
            let patched = self.patch_projection(snapshot.as_ref().clone(), delta)?;
            self.persist_projection_snapshot(cache_dir, &patched)?;
            let patched = Arc::new(patched);
            store_cached_snapshot(key, Arc::clone(&patched));
            return Ok(patched);
        }
        let snapshot = if let Some(snapshot) = self.load_projection_snapshot(cache_dir)? {
            let delta = self.source_delta(&snapshot.source_fingerprints)?;
            if delta.is_empty() {
                snapshot
            } else {
                let patched = self.patch_projection(snapshot, delta)?;
                self.persist_projection_snapshot(cache_dir, &patched)?;
                patched
            }
        } else {
            let snapshot = self.rebuild_projection()?;
            self.persist_projection_snapshot(cache_dir, &snapshot)?;
            snapshot
        };
        let snapshot = Arc::new(snapshot);
        store_cached_snapshot(key, Arc::clone(&snapshot));
        Ok(snapshot)
    }

    /// Replace the in-memory snapshot for this store's cache location. Used by
    /// callers that enrich a snapshot (runtime overlays) or rebuild it wholesale
    /// and want subsequent readers to share the result.
    pub fn store_shared_snapshot(&self, cache_dir: &Path, snapshot: Arc<ProjectionSnapshot>) {
        store_cached_snapshot(self.memory_cache_key(cache_dir), snapshot);
    }

    fn memory_cache_key(&self, cache_dir: &Path) -> String {
        format!("{}|{}", self.refine_dir.display(), cache_dir.display())
    }

    pub(super) fn relative_path(&self, path: &Path) -> RefineResult<String> {
        path.strip_prefix(&self.refine_dir)
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            .map_err(|error| {
                RefineError::InvalidInput(format!(
                    "path {} is not under refine dir {}: {error}",
                    path.display(),
                    self.refine_dir.display()
                ))
            })
    }

    /// Every Goal record under a state directory, in a stable order.
    pub(crate) fn collect_goal_record_paths(refine_dir: &Path) -> RefineResult<Vec<PathBuf>> {
        Self::collect_json_files(&refine_dir.join("goals"), "goal.json")
    }

    pub(super) fn collect_json_files(root: &Path, file_name: &str) -> RefineResult<Vec<PathBuf>> {
        let mut files = Vec::new();
        if !root.exists() {
            return Ok(files);
        }
        Self::collect_json_files_inner(root, file_name, &mut files)?;
        files.sort();
        Ok(files)
    }

    pub(super) fn collect_json_files_inner(
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

    pub(super) fn read_json(path: &Path) -> RefineResult<Value> {
        let bytes = fs::read(path).map_err(|error| {
            RefineError::Io(format!("failed to read {}: {error}", path.display()))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
        })
    }
}

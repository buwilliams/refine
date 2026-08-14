use super::*;

impl FileProjectProjectionStore {
    pub(super) fn project_activity(
        &self,
    ) -> RefineResult<BTreeMap<String, ActivitySummaryProjection>> {
        let path = self.refine_dir.join(ACTIVITY_LOG_FILE);
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

    /// Round-log activity for the projection, bounded per Goal and overall.
    ///
    /// This used to read every Goal's entire sidecar and hold one entry, with
    /// its own searchable text, for every line ever logged — to serve consumers
    /// that want a recent slice. Because log volume tracks retries and
    /// failures rather than Goal count, that charged resident memory for
    /// everything the fleet had ever gotten wrong.
    ///
    /// Both bounds are needed: the per-Goal window keeps one noisy Goal from
    /// dominating, and the overall cap keeps total cost independent of Goal
    /// count. Complete history stays available per Goal through
    /// `FileLogService`, which is where a Goal's own view reads it from.
    pub(super) fn project_goal_round_activity(
        &self,
        goals: &BTreeMap<String, GoalSummaryProjection>,
    ) -> RefineResult<BTreeMap<String, ActivitySummaryProjection>> {
        let log_service = FileLogService::new(&self.refine_dir);
        // Keyed by (datetime, id) so trimming keeps the newest across all Goals
        // rather than whichever Goals happened to be walked last.
        let mut newest: BTreeMap<(String, String), ActivitySummaryProjection> = BTreeMap::new();
        for goal_id in goals.keys() {
            if goal_id.len() < 2 {
                continue;
            }
            for (index, log) in
                log_service.recent_round_logs(goal_id, PROJECTED_ACTIVITY_PER_GOAL_LIMIT)?
            {
                let entry = round_log_activity_entry(goal_id, index, log);
                let searchable_text = activity_searchable_text(&entry);
                newest.insert(
                    (entry.datetime.clone(), entry.id.clone()),
                    ActivitySummaryProjection {
                        entry,
                        searchable_text,
                    },
                );
            }
            // Trim as we go so peak cost stays at the cap plus one Goal's
            // window, rather than everything collected before a final sort.
            while newest.len() > PROJECTED_ACTIVITY_LIMIT {
                newest.pop_first();
            }
        }
        Ok(newest
            .into_values()
            .map(|projection| (projection.entry.id.clone(), projection))
            .collect())
    }

    pub(super) fn project_changes(
        &self,
        goals: &BTreeMap<String, GoalSummaryProjection>,
    ) -> BTreeMap<String, ChangeSummaryProjection> {
        let Some(target_root) = self.target_root() else {
            return BTreeMap::new();
        };
        let service = self.git_service(target_root);
        let branch = service.inspect("").ok().and_then(|status| status.branch);
        let Ok(changes) = service.recent_changes(1000) else {
            return BTreeMap::new();
        };
        changes
            .into_iter()
            .enumerate()
            .filter_map(|(order, change)| {
                let branch = change.branch.or_else(|| branch.clone());
                let joined_goal = matching_change_goal(goals, branch.as_deref(), &change.subject)?;
                let projection = ChangeSummaryProjection {
                    commit: change.commit,
                    committed_time: change.committed_time,
                    subject: change.subject,
                    goal_id: Some(joined_goal.goal.id.clone()),
                    branch,
                    goal_name: Some(joined_goal.goal.name.clone()),
                    goal_status: Some(joined_goal.goal.status.clone()),
                    goal_priority: Some(joined_goal.goal.priority.as_str().to_string()),
                    goal_assignee: joined_goal.goal.assignee.clone(),
                    searchable_text: String::new(),
                    order,
                };
                let mut projection = projection;
                projection.searchable_text = change_searchable_text(&projection);
                Some((change_projection_key(&projection), projection))
            })
            .collect()
    }

    pub(super) fn target_root(&self) -> Option<PathBuf> {
        target_root_for_refine_dir(&self.refine_dir).ok()
    }

    pub(super) fn git_head_fingerprint(&self) -> Option<SourceFingerprint> {
        let target_root = self.target_root()?;
        // Staleness checks call this several times a second, so HEAD is read
        // straight from the Git directory; forking `git` twice per check is
        // kept only as a fallback for layouts the file walk cannot decode.
        let (branch, latest) = match head_identity_from_files(&target_root) {
            Some(identity) => identity,
            None => {
                let service = self.git_service(target_root);
                let head = service.head_ref().ok()?;
                (
                    head.branch.unwrap_or_default(),
                    head.commit.unwrap_or_default(),
                )
            }
        };
        if branch.is_empty() && latest.is_empty() {
            return None;
        }
        Some(SourceFingerprint {
            path: "git:HEAD".to_string(),
            size: latest.len() as u64,
            modified_unix_ms: None,
            change_unix_ns: Some(0),
            content_hash: Some(format!("{branch}:{latest}")),
        })
    }

    pub(super) fn git_service(&self, target_root: PathBuf) -> FileGitWorktreeService {
        if let Some(runtime_root) = &self.runtime_root {
            FileGitWorktreeService::with_runtime_root(target_root, runtime_root)
        } else {
            FileGitWorktreeService::new(target_root)
        }
    }
}

/// Decode `(branch, head identity)` from the Git directory without a
/// subprocess. The identity only has to change when HEAD moves: a loose ref
/// contributes its commit hash, a packed ref its pack file's size and mtime.
fn head_identity_from_files(target_root: &std::path::Path) -> Option<(String, String)> {
    let git_entry = target_root.join(".git");
    let git_dir = if git_entry.is_dir() {
        git_entry
    } else {
        // A linked worktree's `.git` is a file: `gitdir: <path>`.
        let pointer = fs::read_to_string(&git_entry).ok()?;
        let raw = pointer.strip_prefix("gitdir:")?.trim();
        let path = std::path::PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            target_root.join(path)
        }
    };
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref:") else {
        // Detached HEAD: the file holds the commit hash itself.
        return Some((String::new(), head.to_string()));
    };
    let reference = reference.trim();
    let branch = reference
        .strip_prefix("refs/heads/")
        .unwrap_or(reference)
        .to_string();
    // Refs are shared across worktrees; a linked worktree names the shared
    // directory in its `commondir` file.
    let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => {
            let path = std::path::PathBuf::from(common.trim());
            if path.is_absolute() {
                path
            } else {
                git_dir.join(path)
            }
        }
        Err(_) => git_dir,
    };
    if let Ok(commit) = fs::read_to_string(common_dir.join(reference)) {
        return Some((branch, commit.trim().to_string()));
    }
    let packed = fs::metadata(common_dir.join("packed-refs")).ok()?;
    let modified = packed
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    Some((branch, format!("packed:{}:{modified}", packed.len())))
}

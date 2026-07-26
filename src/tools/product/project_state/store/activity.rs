use super::*;

impl FileProjectStateStore {
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

    pub(super) fn project_goal_round_activity(
        &self,
        goals: &BTreeMap<String, GoalSummaryProjection>,
    ) -> RefineResult<BTreeMap<String, ActivitySummaryProjection>> {
        let log_service = FileLogService::new(&self.refine_dir);
        let mut activity = BTreeMap::new();
        for goal_id in goals.keys() {
            if goal_id.len() < 2 {
                continue;
            }
            for (index, log) in log_service.all_round_logs(goal_id)?.into_iter().enumerate() {
                let entry = round_log_activity_entry(goal_id, index, log);
                let searchable_text = activity_searchable_text(&entry);
                activity.insert(
                    entry.id.clone(),
                    ActivitySummaryProjection {
                        entry,
                        searchable_text,
                    },
                );
            }
        }
        Ok(activity)
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
        let service = self.git_service(target_root);
        let head = service.head_ref().ok()?;
        let branch = head.branch.unwrap_or_default();
        let latest = head.commit.unwrap_or_default();
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

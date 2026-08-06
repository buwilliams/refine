use super::*;

pub(super) fn run_worktree_cleanup_worker(
    runtime_root: &Path,
    project_registry_root: Option<&Path>,
) -> RefineResult<()> {
    let mut next_cleanup = Instant::now();
    loop {
        if automation_is_paused(runtime_root)? {
            return Ok(());
        }
        if Instant::now() >= next_cleanup {
            match registered_target_roots(runtime_root, project_registry_root) {
                Ok(target_roots) => {
                    for target_root in target_roots {
                        if automation_is_paused(runtime_root)? {
                            return Ok(());
                        }
                        // Each app is independently fail-closed so one unavailable
                        // repository does not prevent reclaiming inactive worktrees
                        // belonging to the other registered apps.
                        run_configured_worktree_cleanup(runtime_root, &target_root);
                    }
                }
                Err(error) => {
                    eprintln!("refine worktree cleanup: failed to read registered apps: {error}");
                }
            }
            next_cleanup = Instant::now() + WORKTREE_CLEANUP_INTERVAL;
        }
        thread::sleep(WORKTREE_CLEANUP_POLL_INTERVAL);
    }
}

pub(super) fn registered_target_roots(
    runtime_root: &Path,
    project_registry_root: Option<&Path>,
) -> RefineResult<Vec<PathBuf>> {
    let registry =
        FileProjectRegistryService::new(project_registry_root.unwrap_or(runtime_root), None)
            .load()?;
    let mut roots = registry
        .apps
        .values()
        .map(|app| PathBuf::from(&app.path))
        .collect::<Vec<_>>();
    if let Some(active) = registry.active_app {
        roots.push(PathBuf::from(active));
    }
    for root in &mut roots {
        *root = root.canonicalize().unwrap_or_else(|_| root.clone());
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

pub(super) fn run_configured_worktree_cleanup(runtime_root: &Path, target_root: &Path) {
    let result = (|| {
        let refine_dir = refine_dir_for_target_root(target_root)?;
        let settings = FileSettingsService::with_active_root(&refine_dir, runtime_root).load()?;
        let Some(older_than_seconds) = automatic_cleanup_delay_seconds(&settings) else {
            return Ok(());
        };
        let report = FileWorktreeCleanupService::new(target_root, runtime_root).run(
            WorktreeCleanupOptions {
                apply: true,
                older_than_seconds,
            },
        )?;
        if let Some(failures) = cleanup_failure_summary(&report) {
            eprintln!("refine worktree cleanup: {failures}");
        }
        Ok::<(), RefineError>(())
    })();
    if let Err(error) = result {
        eprintln!("refine worktree cleanup: {error}");
    }
}

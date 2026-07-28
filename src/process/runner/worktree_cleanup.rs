use super::*;

pub(super) fn run_worktree_cleanup_worker(
    runtime_root: &Path,
    project_registry_root: Option<&Path>,
) -> RefineResult<()> {
    let mut cleanup_root = None;
    let mut next_cleanup = Instant::now();
    loop {
        match current_target_root(runtime_root, project_registry_root) {
            Ok(Some(target_root)) => {
                let root = target_root
                    .canonicalize()
                    .unwrap_or_else(|_| target_root.clone());
                if cleanup_root.as_ref() != Some(&root) || Instant::now() >= next_cleanup {
                    run_configured_worktree_cleanup(runtime_root, &target_root);
                    cleanup_root = Some(root);
                    next_cleanup = Instant::now() + WORKTREE_CLEANUP_INTERVAL;
                }
            }
            Ok(None) => cleanup_root = None,
            Err(error) => {
                eprintln!("refine worktree cleanup: failed to read the active app: {error}");
            }
        }
        thread::sleep(WORKTREE_CLEANUP_POLL_INTERVAL);
    }
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

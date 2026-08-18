use super::*;

use crate::tools::host::git_sync::{
    StateRecoveryAuthority, StateRecoveryDecision, StateRecoveryOverride, StateRecoveryRunPolicy,
};

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    let Commands::Sync {
        preview,
        authority,
        paths,
        cache_dir,
        target_root,
    } = command
    else {
        unreachable!("command family was routed incorrectly")
    };
    match target_root {
        Some(target_root) => dispatch_direct(preview, authority, paths, cache_dir, target_root),
        None => dispatch_daemon(preview, authority, paths),
    }
}

/// Direct (daemonless) form used by tests via the hidden --target-root.
fn dispatch_direct(
    preview: bool,
    authority: Option<CliSyncAuthority>,
    paths: Vec<PathBuf>,
    cache_dir: Option<PathBuf>,
    target_root: PathBuf,
) -> RefineResult<()> {
    let refine_dir = refine_dir_for_target_root(&target_root)?;
    if preview {
        // Read-only: on error nothing has been printed or written.
        let preview = FileGitSyncService::new(&target_root, refine_dir.join("runtime"))
            .preview_state_recovery()?;
        print_json(&serde_json::to_value(preview).unwrap());
        return Ok(());
    }
    if let Some(authority) = authority {
        let result = FileGitSyncService::new(&target_root, refine_dir.join("runtime"))
            .run_state_recovery_with_policy(StateRecoveryRunPolicy::Decision(sync_decision(
                authority, paths,
            )))?;
        print_json(&serde_json::to_value(result).unwrap());
        return Ok(());
    }
    let runtime_root = cache_dir
        .as_ref()
        .and_then(|cache_dir| cache_dir.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| refine_dir.join("runtime"));
    let git_sync = FileGitSyncService::new(&target_root, &runtime_root)
        .with_agent_resolution()
        .sync()?;
    let store = cache_dir
        .as_ref()
        .and_then(|cache_dir| cache_dir.parent())
        .map(|runtime_root| {
            FileProjectProjectionStore::with_runtime_root(&refine_dir, runtime_root)
        })
        .unwrap_or_else(|| FileProjectProjectionStore::new(&refine_dir));
    let snapshot = store.rebuild_projection()?;
    if let Some(cache_dir) = &cache_dir {
        store.persist_projection_snapshot(cache_dir, &snapshot)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "goals": snapshot.goals.len(),
            "features": snapshot.features.len(),
            "source_fingerprints": snapshot.source_fingerprints.len(),
            "status_counts": snapshot.status_counts(),
            "cache_updated": cache_dir.is_some(),
            "git_sync": git_sync
        }))
        .unwrap()
    );
    Ok(())
}

pub(super) fn dispatch_daemon(
    preview: bool,
    authority: Option<CliSyncAuthority>,
    paths: Vec<PathBuf>,
) -> RefineResult<()> {
    let response = if preview {
        daemon_json("GET", "/sync/preview", None)?
    } else if authority.is_some() {
        daemon_json("POST", "/sync", sync_request_body(authority, paths))?
    } else {
        follow_daemon_operation(daemon_json("POST", "/sync", None)?)?
    };
    print_json(&response);
    Ok(())
}

/// `--path` entries flip to the opposite side of the chosen authority.
fn sync_decision(authority: CliSyncAuthority, paths: Vec<PathBuf>) -> StateRecoveryDecision {
    let (default_authority, exception) = match authority {
        CliSyncAuthority::Live => (StateRecoveryAuthority::Live, StateRecoveryAuthority::Remote),
        CliSyncAuthority::Remote => (StateRecoveryAuthority::Remote, StateRecoveryAuthority::Live),
    };
    StateRecoveryDecision {
        default_authority,
        overrides: paths
            .into_iter()
            .map(|path| StateRecoveryOverride {
                path: path.to_string_lossy().replace('\\', "/"),
                authority: exception,
            })
            .collect(),
    }
}

fn sync_request_body(
    authority: Option<CliSyncAuthority>,
    paths: Vec<PathBuf>,
) -> Option<serde_json::Value> {
    authority.map(|authority| {
        json!({
            "authority": match authority {
                CliSyncAuthority::Live => "live",
                CliSyncAuthority::Remote => "remote",
            },
            "paths": paths
                .iter()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .collect::<Vec<_>>()
        })
    })
}

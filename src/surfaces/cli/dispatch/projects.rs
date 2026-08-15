use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Project {
            action:
                ProjectAction::Status {
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root).status()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Attach {
                    path,
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root)
                .attach_with_migration(&path)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Switch {
                    name,
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root)
                .switch_with_migration(&name)?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Detach {
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root).detach()?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Register {
                    name,
                    path,
                    runtime_root,
                    target_root,
                },
        } => {
            let registry = FileProjectRegistryService::new(runtime_root, target_root)
                .register_path(Some(&name), &path, false)?;
            println!("{}", serde_json::to_string_pretty(&registry).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Clone {
                    source,
                    destination,
                    name,
                    make_current,
                    runtime_root,
                    target_root,
                },
        } => {
            let status = FileProjectRegistryService::new(runtime_root, target_root).clone_app(
                &source,
                &destination,
                name.as_deref(),
                make_current,
            )?;
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Remove {
                    name,
                    runtime_root,
                    target_root,
                },
        } => {
            let registry =
                FileProjectRegistryService::new(runtime_root, target_root).remove(&name)?;
            println!("{}", serde_json::to_string_pretty(&registry).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Migrate {
                    target_root,
                    runtime_root,
                },
        } => {
            let report =
                FileProjectRegistryService::new(runtime_root, target_root).migrate_current()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::to_value(report).unwrap()).unwrap()
            );
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Doctor {
                    target_root,
                    runtime_root,
                    repo_root,
                },
        } => {
            let report =
                FileDiagnosticsService::new(target_root, runtime_root, repo_root).doctor()?;
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::Sync {
                    target_root: Some(target_root),
                    cache_dir,
                },
        } => {
            let refine_dir = refine_dir_for_target_root(&target_root)?;
            let runtime_root = cache_dir
                .as_ref()
                .and_then(|cache_dir| cache_dir.parent())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| refine_dir.join("runtime"));
            let git_sync = FileGitSyncService::new(&target_root, &runtime_root).sync()?;
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
        Commands::Project {
            action: ProjectAction::Sync {
                target_root: None, ..
            },
        } => {
            let response = follow_daemon_operation(daemon_json("POST", "/project/sync", None)?)?;
            print_json(&response);
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::StateRecovery {
                    action:
                        ProjectStateRecoveryAction::Preview {
                            target_root: Some(target_root),
                        },
                },
        } => {
            let runtime_root = refine_dir_for_target_root(&target_root)?.join("runtime");
            let preview =
                FileGitSyncService::new(target_root, runtime_root).preview_state_recovery()?;
            print_json(&serde_json::to_value(preview).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::StateRecovery {
                    action:
                        ProjectStateRecoveryAction::Apply {
                            authority,
                            preview_file,
                            live_paths,
                            remote_paths,
                            target_root: Some(target_root),
                        },
                },
        } => {
            let preview = read_state_recovery_preview(&preview_file)?;
            let runtime_root = refine_dir_for_target_root(&target_root)?.join("runtime");
            let result = FileGitSyncService::new(target_root, runtime_root)
                .apply_state_recovery_decision(
                    recovery_decision(authority, live_paths, remote_paths),
                    preview,
                )?;
            print_json(&serde_json::to_value(result).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::StateRecovery {
                    action: ProjectStateRecoveryAction::Preview { target_root: None },
                },
        } => {
            let response = daemon_json("GET", "/project/state-recovery/preview", None)?;
            print_json(&response);
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::StateRecovery {
                    action:
                        ProjectStateRecoveryAction::Apply {
                            authority,
                            preview_file,
                            live_paths,
                            remote_paths,
                            target_root: None,
                        },
                },
        } => {
            let preview = read_state_recovery_preview(&preview_file)?;
            let response = daemon_json(
                "POST",
                "/project/state-recovery/apply",
                Some(json!({
                    "decision": recovery_decision(authority, live_paths, remote_paths),
                    "preview": preview
                })),
            )?;
            print_json(&response);
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::CleanupWorktrees {
                    apply,
                    older_than_seconds,
                    runtime_root,
                    target_root: Some(target_root),
                },
        } => {
            let report = FileWorktreeCleanupService::new(target_root, runtime_root).run(
                WorktreeCleanupOptions {
                    apply,
                    older_than_seconds,
                },
            )?;
            print_json(&serde_json::to_value(report).unwrap());
            Ok(())
        }
        Commands::Project {
            action:
                ProjectAction::CleanupWorktrees {
                    apply,
                    older_than_seconds,
                    target_root: None,
                    ..
                },
        } => {
            let response = daemon_json(
                "POST",
                "/project/worktrees/cleanup",
                Some(json!({
                    "apply": apply,
                    "older_than_seconds": older_than_seconds
                })),
            )?;
            print_json(&response);
            Ok(())
        }
        _ => unreachable!("command family was routed incorrectly"),
    }
}

#[cfg(not(test))]
pub(super) fn dispatch_project_daemon(action: ProjectAction) -> RefineResult<()> {
    let response = match action {
        ProjectAction::Status { .. } => daemon_json("GET", "/project/status", None)?,
        ProjectAction::Attach { path, .. } => {
            daemon_json("POST", "/project/attach", Some(json!({ "path": path })))?
        }
        ProjectAction::Switch { name, .. } => {
            daemon_json("POST", "/apps/switch", Some(json!({ "name": name })))?
        }
        ProjectAction::Detach { .. } => daemon_json("POST", "/project/detach", None)?,
        ProjectAction::Register { name, path, .. } => daemon_json(
            "POST",
            "/apps/register",
            Some(json!({
                "name": name,
                "path": path
            })),
        )?,
        ProjectAction::Clone {
            source,
            destination,
            name,
            make_current,
            ..
        } => daemon_json(
            "POST",
            "/apps/clone",
            Some(json!({
                "source": source,
                "destination": destination,
                "name": name,
                "make_current": make_current
            })),
        )?,
        ProjectAction::Remove { name, .. } => {
            daemon_json("DELETE", "/apps", Some(json!({ "name": name })))?
        }
        ProjectAction::Migrate { .. } => daemon_json("POST", "/project/migrate", None)?,
        ProjectAction::Sync { .. } => {
            follow_daemon_operation(daemon_json("POST", "/project/sync", None)?)?
        }
        ProjectAction::StateRecovery { action } => match action {
            ProjectStateRecoveryAction::Preview { .. } => {
                daemon_json("GET", "/project/state-recovery/preview", None)?
            }
            ProjectStateRecoveryAction::Apply {
                authority,
                preview_file,
                live_paths,
                remote_paths,
                ..
            } => {
                let preview = read_state_recovery_preview(&preview_file)?;
                daemon_json(
                    "POST",
                    "/project/state-recovery/apply",
                    Some(json!({
                        "decision": recovery_decision(authority, live_paths, remote_paths),
                        "preview": preview
                    })),
                )?
            }
        },
        ProjectAction::CleanupWorktrees {
            apply,
            older_than_seconds,
            ..
        } => daemon_json(
            "POST",
            "/project/worktrees/cleanup",
            Some(json!({
                "apply": apply,
                "older_than_seconds": older_than_seconds
            })),
        )?,
        ProjectAction::Doctor { .. } => daemon_json("GET", "/diagnostics", None)?,
    };
    print_json(&response);
    Ok(())
}

fn read_state_recovery_preview(
    path: &Path,
) -> RefineResult<crate::tools::host::git_sync::StateRecoveryPreview> {
    let bytes = fs::read(path).map_err(|error| {
        RefineError::InvalidInput(format!(
            "failed to read state recovery preview {}: {error}",
            path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        RefineError::InvalidInput(format!(
            "failed to parse state recovery preview {}: {error}",
            path.display()
        ))
    })
}

fn recovery_authority(
    authority: CliStateRecoveryAuthority,
) -> crate::tools::host::git_sync::StateRecoveryAuthority {
    match authority {
        CliStateRecoveryAuthority::Live => {
            crate::tools::host::git_sync::StateRecoveryAuthority::Live
        }
        CliStateRecoveryAuthority::Remote => {
            crate::tools::host::git_sync::StateRecoveryAuthority::Remote
        }
    }
}

fn recovery_decision(
    authority: CliStateRecoveryAuthority,
    live_paths: Vec<PathBuf>,
    remote_paths: Vec<PathBuf>,
) -> crate::tools::host::git_sync::StateRecoveryDecision {
    use crate::tools::host::git_sync::{StateRecoveryDecision, StateRecoveryOverride};
    let mut overrides = live_paths
        .into_iter()
        .map(|path| StateRecoveryOverride {
            path: path.to_string_lossy().replace('\\', "/"),
            authority: crate::tools::host::git_sync::StateRecoveryAuthority::Live,
        })
        .collect::<Vec<_>>();
    overrides.extend(remote_paths.into_iter().map(|path| StateRecoveryOverride {
        path: path.to_string_lossy().replace('\\', "/"),
        authority: crate::tools::host::git_sync::StateRecoveryAuthority::Remote,
    }));
    StateRecoveryDecision {
        default_authority: recovery_authority(authority),
        overrides,
    }
}

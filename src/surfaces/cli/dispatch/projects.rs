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

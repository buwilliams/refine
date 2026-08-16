use super::*;

pub(super) fn update_checked_out_branch(
    checkout: &Path,
    reference: &str,
    from_commit: &str,
    to_commit: &str,
) -> RefineResult<()> {
    git_ok(checkout, &["read-tree", "-m", "-u", from_commit, to_commit])?;
    if let Err(update_error) = git_ok(checkout, &["update-ref", reference, to_commit, from_commit])
    {
        return match git_ok(checkout, &["read-tree", "-m", "-u", to_commit, from_commit]) {
            Ok(()) => Err(update_error),
            Err(recovery_error) => Err(append_error_context(
                update_error,
                &format!(
                    "failed to restore the index and worktree to {from_commit} after the ref update failed: {recovery_error}"
                ),
            )),
        };
    }
    Ok(())
}

pub(super) fn cleanup_candidate_worktree(
    checkout: &Path,
    worktree: &Path,
    registered: bool,
) -> Vec<String> {
    if registered {
        match git_ok(
            checkout,
            &[
                "worktree",
                "remove",
                "--force",
                &worktree.display().to_string(),
            ],
        ) {
            Ok(()) => return Vec::new(),
            Err(remove_error) => {
                let filesystem_cleanup = if worktree.exists() {
                    fs::remove_dir_all(worktree).map_err(|error| {
                        format!("failed to remove {}: {error}", worktree.display())
                    })
                } else {
                    Ok(())
                };
                let prune_cleanup = git_ok(checkout, &["worktree", "prune"])
                    .map_err(|error| format!("failed to prune Git worktree metadata: {error}"));
                if filesystem_cleanup.is_ok() && prune_cleanup.is_ok() {
                    return Vec::new();
                }
                let mut errors = vec![format!("Git worktree removal failed: {remove_error}")];
                if let Err(error) = filesystem_cleanup {
                    errors.push(error);
                }
                if let Err(error) = prune_cleanup {
                    errors.push(error);
                }
                return errors;
            }
        }
    }

    let mut errors = Vec::new();
    if worktree.exists()
        && let Err(error) = fs::remove_dir_all(worktree)
    {
        errors.push(format!("failed to remove {}: {error}", worktree.display()));
    }
    if let Err(error) = git_ok(checkout, &["worktree", "prune"]) {
        errors.push(format!("failed to prune Git worktree metadata: {error}"));
    }
    errors
}

pub(super) fn remove_candidate_binary(binary: &Path) -> Vec<String> {
    if !binary.exists() {
        return Vec::new();
    }
    fs::remove_file(binary)
        .err()
        .map(|error| {
            vec![format!(
                "failed to remove candidate binary {}: {error}",
                binary.display()
            )]
        })
        .unwrap_or_default()
}

pub(super) fn with_candidate_cleanup(
    primary: RefineError,
    worktree: &Path,
    cleanup_errors: Vec<String>,
) -> RefineError {
    if cleanup_errors.is_empty() {
        return primary;
    }
    append_error_context(
        primary,
        &format!(
            "candidate cleanup also failed: {}; remove {} and run `git worktree prune` before retrying",
            cleanup_errors.join("; "),
            worktree.display()
        ),
    )
}

pub(super) fn append_error_context(error: RefineError, context: &str) -> RefineError {
    let append = |message: String| format!("{message}; {context}");
    match error {
        RefineError::InvalidInput(message) => RefineError::InvalidInput(append(message)),
        RefineError::NotFound(message) => RefineError::NotFound(append(message)),
        RefineError::Unauthorized(message) => RefineError::Unauthorized(append(message)),
        RefineError::Conflict(message) => RefineError::Conflict(append(message)),
        RefineError::StateSyncMissingBaseline(message) => {
            RefineError::StateSyncMissingBaseline(append(message))
        }
        RefineError::StateRecoveryConflict { reason, message } => {
            RefineError::StateRecoveryConflict {
                reason,
                message: append(message),
            }
        }
        stale @ RefineError::StaleCandidate { .. } => {
            RefineError::Conflict(append(stale.to_string()))
        }
        advanced @ RefineError::TargetAdvanced { .. } => {
            RefineError::Conflict(append(advanced.to_string()))
        }
        infrastructure @ RefineError::QualityCandidateInfrastructure(_) => {
            RefineError::Conflict(append(infrastructure.to_string()))
        }
        RefineError::Degraded(message) => RefineError::Degraded(append(message)),
        RefineError::Io(message) => RefineError::Io(append(message)),
        RefineError::Serialization(message) => RefineError::Serialization(append(message)),
        structured @ RefineError::StructuredOutput(_) => {
            RefineError::Serialization(append(structured.to_string()))
        }
        RefineError::NotImplemented(message) => RefineError::NotImplemented(append(message)),
    }
}

pub(super) fn ensure_checkout(checkout: &Path) -> RefineResult<()> {
    if checkout.join(".git").exists() && checkout.join("Cargo.toml").is_file() {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(format!(
            "source promotion requires a Refine Git checkout: {}",
            checkout.display()
        )))
    }
}

pub(super) fn git_text(checkout: &Path, args: &[&str]) -> RefineResult<String> {
    let output = git_output(checkout, args)?;
    if !output.status.success() {
        return Err(git_failure(args, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(super) fn git_optional_text(checkout: &Path, args: &[&str]) -> RefineResult<Option<String>> {
    let output = git_output(checkout, args)?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!value.is_empty()).then_some(value))
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(git_failure(args, &output))
    }
}

pub(super) fn git_ok(checkout: &Path, args: &[&str]) -> RefineResult<()> {
    let output = git_output(checkout, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(git_failure(args, &output))
    }
}

pub(super) fn git_status(checkout: &Path, args: &[&str]) -> RefineResult<bool> {
    let output = git_output(checkout, args)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_failure(args, &output)),
    }
}

pub(super) fn git_output(checkout: &Path, args: &[&str]) -> RefineResult<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(checkout)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git {}: {error}", args.join(" "))))
}

pub(super) fn git_failure(args: &[&str], output: &std::process::Output) -> RefineError {
    RefineError::Conflict(format!(
        "git {} failed with status {}: {}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

pub(super) fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

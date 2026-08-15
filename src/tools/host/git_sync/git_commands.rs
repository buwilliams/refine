use super::*;

impl FileGitSyncService {
    pub(super) fn remote_exists(&self, remote: &str) -> RefineResult<bool> {
        Ok(self
            .git_stdout(&["remote"])?
            .lines()
            .any(|candidate| candidate.trim() == remote))
    }

    pub(super) fn git_stdout(&self, args: &[&str]) -> RefineResult<String> {
        let output = self.git_checked(args)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub(super) fn git_success(&self, args: &[&str]) -> RefineResult<bool> {
        self.git(args).map(|output| output.success)
    }

    pub(super) fn git_checked(&self, args: &[&str]) -> RefineResult<GitCommandOutput> {
        let output = self.git(args)?;
        if output.success {
            Ok(output)
        } else {
            Err(command_failed(&format!("git {}", args.join(" ")), &output))
        }
    }

    pub(super) fn git(&self, args: &[&str]) -> RefineResult<GitCommandOutput> {
        self.git_at(&self.target_root, args)
    }

    pub(super) fn git_at_stdout(
        &self,
        root: &std::path::Path,
        args: &[&str],
    ) -> RefineResult<String> {
        let output = self.git_at_checked(root, args)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub(super) fn git_at_checked(
        &self,
        root: &std::path::Path,
        args: &[&str],
    ) -> RefineResult<GitCommandOutput> {
        let output = self.git_at(root, args)?;
        if output.success {
            Ok(output)
        } else {
            Err(command_failed(&format!("git {}", args.join(" ")), &output))
        }
    }

    pub(super) fn git_at(
        &self,
        root: &std::path::Path,
        args: &[&str],
    ) -> RefineResult<GitCommandOutput> {
        // Local read-only commands run as plain subprocesses; supervision only
        // earns its bookkeeping for commands that mutate the repository or
        // touch the network, where the stall budget matters.
        if is_local_read_only_git_command(args) {
            return git_at_untracked(&self.target_root, root, args);
        }
        let mut process_args = vec!["-C".to_string(), self.target_root.display().to_string()];
        if root != self.target_root {
            process_args.extend(["-C".to_string(), root.display().to_string()]);
        }
        process_args.extend(args.iter().map(|arg| arg.to_string()));
        let output = FileProcessSupervisor::new(&self.runtime_root).run_to_completion(
            ManagedProcessSpec {
                owner: ProcessOwner::Maintenance,
                command: "git".to_string(),
                args: process_args,
                cwd: None,
                env: vec![
                    ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
                    ("GIT_AUTHOR_NAME".to_string(), "Refine".to_string()),
                    (
                        "GIT_AUTHOR_EMAIL".to_string(),
                        "refine@localhost".to_string(),
                    ),
                    ("GIT_COMMITTER_NAME".to_string(), "Refine".to_string()),
                    (
                        "GIT_COMMITTER_EMAIL".to_string(),
                        "refine@localhost".to_string(),
                    ),
                ],
                stdin: None,
                // Synchronization holds the repository lock while these run; a
                // silent hung fetch or push must not wedge every queued caller
                // for the life of the daemon. Progress resets the budget, so a
                // legitimately slow transfer is unaffected.
                limits: Some(ProcessResourceLimits {
                    stall_timeout_seconds: Some(GIT_SYNC_STALL_TIMEOUT_SECONDS),
                    ..ProcessResourceLimits::default()
                }),
                authorization_command: Some(format!("git {}", args.join(" "))),
                sensitive: false,
                metadata: serde_json::from_value(json!({
                    "kind": "repository_reconcile",
                    "target_root": self.target_root.display().to_string()
                }))
                .unwrap_or_default(),
            },
        )?;
        Ok(GitCommandOutput {
            success: output.success(),
            stdout: output.stdout.into_bytes(),
            stderr: output.stderr.into_bytes(),
        })
    }
}

const GIT_SYNC_STALL_TIMEOUT_SECONDS: u64 = 300;

fn is_local_read_only_git_command(args: &[&str]) -> bool {
    match args {
        ["branch", "--show-current"] => true,
        ["worktree", "list", "--porcelain"] => true,
        // `ls-remote` reads too, but over the network — it stays supervised
        // for the stall budget.
        [command, ..] => matches!(
            *command,
            "cat-file"
                | "diff"
                | "log"
                | "ls-files"
                | "merge-base"
                | "rev-list"
                | "rev-parse"
                | "show"
                | "status"
        ),
        [] => false,
    }
}

fn git_at_untracked(
    target_root: &std::path::Path,
    root: &std::path::Path,
    args: &[&str],
) -> RefineResult<GitCommandOutput> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(target_root);
    if root != target_root {
        command.arg("-C").arg(root);
    }
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Refine")
        .env("GIT_AUTHOR_EMAIL", "refine@localhost")
        .env("GIT_COMMITTER_NAME", "Refine")
        .env("GIT_COMMITTER_EMAIL", "refine@localhost");
    let output = command.output().map_err(|error| {
        RefineError::Io(format!("failed to run git {}: {error}", args.join(" ")))
    })?;
    Ok(GitCommandOutput {
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

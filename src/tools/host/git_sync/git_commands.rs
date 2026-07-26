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
                limits: None,
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

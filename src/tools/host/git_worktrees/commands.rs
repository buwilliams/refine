use super::*;

impl FileGitWorktreeService {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            runtime_root: None,
            operation_id: None,
            process_metadata: Map::new(),
        }
    }

    pub fn with_runtime_root(root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            runtime_root: Some(runtime_root.into()),
            operation_id: None,
            process_metadata: Map::new(),
        }
    }

    pub fn with_operation_id(mut self, operation_id: impl Into<String>) -> Self {
        self.operation_id = Some(operation_id.into());
        self
    }

    pub fn with_process_metadata(mut self, metadata: Map<String, Value>) -> Self {
        self.process_metadata = metadata;
        self
    }

    pub(super) fn git_output(&self, args: &[&str]) -> RefineResult<HostCommandOutput> {
        let output = self.git_raw_with_env(args, &[])?;
        if output.success {
            Ok(output)
        } else {
            Err(RefineError::Conflict(trimmed_command_text(&output)))
        }
    }

    pub(super) fn git_raw(&self, args: &[&str]) -> RefineResult<HostCommandOutput> {
        self.git_raw_with_env(args, &[])
    }

    pub(super) fn git_output_with_env(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> RefineResult<HostCommandOutput> {
        let output = self.git_raw_with_env(args, env)?;
        if output.success {
            Ok(output)
        } else {
            Err(RefineError::Conflict(trimmed_command_text(&output)))
        }
    }

    pub(super) fn git_output_with_stdin(
        &self,
        args: &[&str],
        stdin: &str,
    ) -> RefineResult<HostCommandOutput> {
        let output = self.git_raw_with_env_and_stdin(args, &[], Some(stdin))?;
        if output.success {
            Ok(output)
        } else {
            Err(RefineError::Conflict(trimmed_command_text(&output)))
        }
    }

    pub(super) fn git_raw_with_env(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> RefineResult<HostCommandOutput> {
        self.git_raw_with_env_and_stdin(args, env, None)
    }

    pub(super) fn git_raw_with_env_and_stdin(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> RefineResult<HostCommandOutput> {
        if self.operation_id.is_none() && is_read_only_git_command(args) {
            return self.git_raw_untracked(args, env, stdin);
        }
        let mut process_args = vec!["-C".to_string(), self.root.display().to_string()];
        process_args.extend(args.iter().map(|arg| arg.to_string()));
        let mut metadata = self.process_metadata.clone();
        metadata.insert("kind".to_string(), json!("git"));
        metadata.insert("isolated_process_group".to_string(), json!(true));
        metadata.insert(
            "git_command".to_string(),
            json!(args.first().copied().unwrap_or("git")),
        );
        if let Some(operation_id) = self.operation_id.as_deref() {
            metadata.insert("operation_id".to_string(), json!(operation_id));
        }
        let output = FileProcessSupervisor::new(self.process_runtime_root()).run_to_completion(
            ManagedProcessSpec {
                owner: ProcessOwner::Maintenance,
                command: "git".to_string(),
                args: process_args,
                cwd: None,
                env: env
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
                stdin: stdin.map(str::to_string),
                // Git runs while holding the repository lock, so a command that
                // hangs — an unreachable remote, a wedged index — stalls every
                // caller queued behind it indefinitely, including the workflow
                // scheduler. A progress budget rather than a total one: fetching
                // a large repository on a slow link is legitimately slow and
                // reports progress throughout, while a wedged command is silent.
                limits: Some(ProcessResourceLimits {
                    stall_timeout_seconds: Some(GIT_STALL_TIMEOUT_SECONDS),
                    ..ProcessResourceLimits::default()
                }),
                authorization_command: Some(format!("git {}", args.join(" "))),
                sensitive: false,
                metadata,
            },
        )?;
        Ok(HostCommandOutput {
            success: output.success(),
            exit_code: output.process.exit_code,
            stdout: output.stdout.into_bytes(),
            stderr: output.stderr.into_bytes(),
        })
    }

    pub(super) fn git_raw_untracked(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> RefineResult<HostCommandOutput> {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.root)
            .args(args)
            // Read-only projection and inspection calls may overlap a serialized merge.
            // Prevent commands such as `git status` from opportunistically refreshing the
            // shared index and racing the integration owner's required index write.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .envs(env.iter().map(|(key, value)| (*key, *value)))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if stdin.is_some() {
            command.stdin(std::process::Stdio::piped());
        }
        let mut child = command
            .spawn()
            .map_err(|error| RefineError::Io(format!("failed to run git: {error}")))?;
        if let Some(input) = stdin
            && let Some(mut child_stdin) = child.stdin.take()
        {
            child_stdin
                .write_all(input.as_bytes())
                .map_err(|error| RefineError::Io(format!("failed to write Git input: {error}")))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| RefineError::Io(format!("failed to read Git output: {error}")))?;
        Ok(HostCommandOutput {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    pub(super) fn process_runtime_root(&self) -> PathBuf {
        self.runtime_root.clone().unwrap_or_else(|| {
            std::env::temp_dir()
                .join("refine")
                .join("git-worktrees")
                .join(sanitize_runtime_component(&self.root.display().to_string()))
        })
    }

    pub(super) fn commit_inner(
        &self,
        message: &str,
        pathspecs: &[String],
        allow_empty: bool,
    ) -> RefineResult<String> {
        if message.trim().is_empty() {
            return Err(RefineError::InvalidInput(
                "commit message is required".to_string(),
            ));
        }
        if pathspecs.is_empty() {
            self.git_output(&["add", "-A"])?;
        } else {
            let mut add_args = vec!["add", "--"];
            for pathspec in pathspecs {
                add_args.push(pathspec.as_str());
            }
            self.git_output(&add_args)?;
        }
        let mut commit_args = vec!["commit"];
        if allow_empty {
            commit_args.push("--allow-empty");
        }
        commit_args.extend(["-m", message]);
        self.git_output(&commit_args)?;
        let commit = stdout(self.git_output(&["rev-parse", "HEAD"])?)?
            .trim()
            .to_string();
        self.audit(
            "commit",
            "ok",
            json!({"commit": &commit, "message": message, "pathspecs": pathspecs, "allow_empty": allow_empty}),
        )?;
        Ok(commit)
    }

    pub(super) fn audit(
        &self,
        action: &str,
        status: &str,
        details: serde_json::Value,
    ) -> RefineResult<()> {
        let path = self.audit_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create Git audit directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let event = json!({
            "action": action,
            "status": status,
            "details": details,
            "created_at": now_timestamp()
        });
        let line = serde_json::to_string(&event).map_err(|error| {
            RefineError::Serialization(format!("failed to encode Git audit event: {error}"))
        })?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to open Git audit {}: {error}",
                    path.display()
                ))
            })?;
        writeln!(file, "{line}").map_err(|error| {
            RefineError::Io(format!(
                "failed to append Git audit {}: {error}",
                path.display()
            ))
        })
    }
}

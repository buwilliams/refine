use super::*;

/// Resolves conflicts through Refine's installed-agent CLI machinery — the
/// same `HostAgentProviderService` invocation path goal workflow steps use,
/// so the login-shell environment capture, prompt transport, supervision,
/// stall budget, and transcript handling are all inherited rather than
/// reimplemented. The `agent_idle_timeout_seconds` convention bounds it.
pub struct InstalledAgentResolver {
    pub provider: String,
    /// The port-scoped agents runtime root (`<runtime>/agents`), matching the
    /// workflow's `ctx.runtime_root.join("agents")` convention.
    pub agents_runtime_root: PathBuf,
    pub stall_timeout_seconds: Option<u64>,
    pub managed_worktree: Option<crate::infrastructure::git::worktrees::ManagedWorktree>,
    pub agent_subpath: String,
}

impl InstalledAgentResolver {
    /// Build from project settings: `agent_cli` picks the provider and
    /// `agent_idle_timeout_seconds` supplies the supervised stall budget,
    /// exactly as unattended workflow verdict invocations derive theirs.
    pub fn from_settings(refine_dir: &Path, runtime_root: &Path) -> Self {
        let settings = FileSettingsService::with_active_root(refine_dir, runtime_root)
            .load()
            .unwrap_or_default();
        let provider = settings
            .get("agent_cli")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .unwrap_or("claude")
            .to_string();
        let stall_timeout_seconds = settings
            .get("agent_idle_timeout_seconds")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(900);
        Self {
            provider,
            agents_runtime_root: runtime_root.join("agents"),
            stall_timeout_seconds: Some(stall_timeout_seconds),
            managed_worktree: None,
            agent_subpath: String::new(),
        }
    }
}

impl ConflictResolver for InstalledAgentResolver {
    fn resolve(&self, request: &ResolutionRequest<'_>) -> RefineResult<ResolverOutcome> {
        let service = HostAgentProviderService::with_runtime_root(&self.agents_runtime_root);
        // Not installed means unavailable, never failed: the caller falls
        // back exactly to pre-agent behavior.
        if service.authenticate(&self.provider).is_err() {
            return Ok(ResolverOutcome::Unavailable);
        }
        let mut metadata = serde_json::from_value::<serde_json::Map<String, serde_json::Value>>(json!({
            "kind": "conflict_resolution", "workspace": request.workspace_dir, "attempt": request.attempt,
        })).unwrap_or_default();
        let cwd = if let Some(workspace) = &self.managed_worktree {
            let cwd = crate::infrastructure::git::worktrees::agent_worktree_cwd(
                &request.workspace_dir.to_string_lossy(),
                &self.agent_subpath,
            )?;
            workspace.validate_cwd(&cwd)?;
            metadata.insert("managed_worktree".to_string(), json!(workspace));
            cwd
        } else {
            request.workspace_dir.to_path_buf()
        };
        let invocation = ProviderInvocation {
            provider: self.provider.clone(),
            prompt: resolution_prompt(request),
            session_id: None,
            cwd: Some(cwd.display().to_string()),
            stall_timeout_seconds: self.stall_timeout_seconds,
            process_metadata: metadata,
        };
        match service.invoke(invocation) {
            Ok(output) => Ok(match authored_question(&output) {
                Some(question) => ResolverOutcome::NeedsDecision { question },
                None => ResolverOutcome::Completed,
            }),
            Err(error) => Ok(ResolverOutcome::Failed(error.to_string())),
        }
    }
}

/// Everything the request carries that the agent cannot read off the
/// workspace: the caller's domain context, how the two lines relate, and why
/// a previous attempt was rejected.
pub(super) fn resolution_prompt(request: &ResolutionRequest<'_>) -> String {
    let mut prompt = request.context.trim_end().to_string();
    let ancestry = request.ancestry.trim();
    if !ancestry.is_empty() {
        prompt.push_str(&format!("\n\nHow the two lines relate: {ancestry}."));
    }
    if let Some(feedback) = request.feedback {
        prompt.push_str(&format!(
            "\n\nYour previous attempt was rejected: {feedback}\nEdit the conflicted files again and correct this."
        ));
    }
    prompt
}

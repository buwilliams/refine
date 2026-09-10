//! Materialize a recovery Round from an intact retained candidate.
use super::*;

/// The `automatic_retry` marker of a Round, when it is a recovery drafted from
/// Quality or Governance findings: `(kind, zero-based source round index)`.
///
/// Integration-race recoveries are excluded on purpose — their candidate
/// itself is stale, so they replay the full pipeline from a fresh base.
pub(super) fn round_scoped_recovery_retry(round: &Value) -> Option<(String, usize)> {
    let retry = round.get("automatic_retry")?;
    let kind = retry.get("kind").and_then(Value::as_str)?;
    if !matches!(kind, "quality" | "governance") {
        return None;
    }
    let source_round = retry.get("source_round").and_then(Value::as_u64)?;
    (source_round >= 1).then(|| (kind.to_string(), source_round as usize - 1))
}

/// Continue a Quality/Governance recovery Round on the source Round's retained
/// candidate: a dedicated Round worktree and fresh branch created
/// at the exact candidate commit. The configured Plan and Implement Skills use
/// the scoped recovery request, and the full Quality and Governance gates judge
/// the resulting candidate. Missing or changed source contents fall back to the
/// ordinary fresh-worktree path; ambiguous ownership fails without touching it.
pub(super) fn begin_scoped_recovery_round(
    ctx: &mut WorkflowContext<'_>,
    app_git: &FileGitWorktreeService,
) -> RefineResult<Option<WorkflowAdvanceOutcome>> {
    let detail = ctx.work_items.show_goal_detail(&ctx.goal_id)?;
    let Some(round) = detail
        .get("rounds")
        .and_then(Value::as_array)
        .and_then(|rounds| rounds.get(ctx.round_idx))
    else {
        return Ok(None);
    };
    let Some((kind, source_round)) = round_scoped_recovery_retry(round) else {
        return Ok(None);
    };
    let recorded = |key: &str| {
        detail
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let (Some(source_branch), Some(candidate), Some(base_commit)) = (
        recorded("branch_name"),
        recorded("candidate_commit"),
        recorded("base_commit"),
    ) else {
        return Ok(None);
    };
    crate::application::workflow::engine::context::validate_round_workspace_branch(
        &detail,
        &ctx.goal_id,
        source_round,
        &source_branch,
        &setting_string(&ctx.settings, "branch_name_pattern", "refine/{goal_id}"),
    )?;
    let Some(worktree_path) = app_git.existing_worktree_for_branch(&source_branch)? else {
        ctx.log(
            "git",
            "Scoped recovery fell back to a fresh worktree; the source Round worktree is gone",
            Some(json_object(json!({"source_branch": source_branch}))),
        )?;
        return Ok(None);
    };
    let worktree_path = worktree_path.display().to_string();
    let source_workspace = crate::infrastructure::git::worktrees::ManagedWorktree {
        repository: ctx.target_root.to_path_buf(),
        path: std::path::PathBuf::from(&worktree_path),
        branch: source_branch.clone(),
        commit: Some(candidate.clone()),
        allow_rebase: false,
        registration: None,
    };
    let source_workspace = source_workspace.pin()?;
    let worktree_git = FileGitWorktreeService::with_runtime_root(&worktree_path, ctx.runtime_root)
        .with_managed_worktree(source_workspace.clone())?;
    let retained_candidate_intact = |worktree_git: &FileGitWorktreeService| -> RefineResult<bool> {
        let head = worktree_git.head_ref()?;
        let status = worktree_git.inspect("")?;
        Ok(head.commit.as_deref() == Some(candidate.as_str()) && status.is_pristine())
    };
    if !retained_candidate_intact(&worktree_git)? {
        ctx.log(
            "git",
            "Scoped recovery fell back to a fresh worktree; the retained candidate checkout changed",
            Some(json_object(json!({
                "source_branch": source_branch,
                "candidate_commit": candidate
            }))),
        )?;
        return Ok(None);
    }
    let branch = implementation_branch_name(
        setting_string(&ctx.settings, "branch_name_pattern", "refine/{goal_id}").as_str(),
        &ctx.goal_id,
        ctx.round_idx,
    );
    let target_branch = setting_string(&ctx.settings, "merge_target_branch", "main");
    // Same invariant as the ordinary path: the durable Todo→Plan status write
    // lands before any Git mutation.
    ctx.request_transition(GoalStatus::Todo, GoalStatus::Plan)?;
    let (worktree_path, handoff) = match with_repository_git_lock(ctx.target_root, || {
        source_workspace.validate()?;
        if !retained_candidate_intact(&worktree_git)? {
            return Err(RefineError::Conflict(format!(
                "Goal {} retained candidate changed before scoped recovery could begin",
                ctx.goal_id
            )));
        }
        // Preserve the source Round and any conflicting recovery attempt verbatim.
        let target = app_git.managed_worktree_path(&branch)?;
        let worktree_path = app_git.ensure_worktree_at_commit(&branch, &target, &candidate)?;
        let handoff = register_candidate_handoff(
            ctx.runtime_root,
            ctx.target_root,
            &ctx.goal_id,
            ctx.round_idx,
            &ctx.node_id,
            &branch,
            &worktree_path,
            &base_commit,
        )?;
        Ok((worktree_path, handoff))
    }) {
        Ok(handoff) => handoff,
        Err(error) => return fail(ctx, "branch", error),
    };
    if let Err(error) = ctx.work_items.update_goal_git_refs(
        &ctx.goal_id,
        &branch,
        &target_branch,
        &base_commit,
        Some(&candidate),
    ) {
        return fail(ctx, "branch", error);
    }
    ctx.log(
        "git",
        &format!("Scoped {kind} recovery uses an isolated checkout of the retained candidate"),
        Some(json_object(json!({
            "branch": branch,
            "worktree": worktree_path,
            "candidate_commit": candidate,
            "source_branch": source_branch
        }))),
    )?;
    ctx.branch = Some(branch);
    ctx.worktree_path = Some(worktree_path);
    ctx.candidate_handoff_operation_id = Some(handoff.id);
    Ok(Some(WorkflowAdvanceOutcome::Transition {
        from: GoalStatus::Todo,
        to: GoalStatus::Plan,
        reason: "Scoped recovery Round entered planning on the retained candidate".to_string(),
    }))
}

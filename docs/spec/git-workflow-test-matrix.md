# Git Workflow Test Matrix

This matrix tracks Refine Git operations that are performed directly by source code or exposed through user-facing workflows. Tests use real temporary Git repositories and remotes unless noted.

| Git operation / workflow | Code path | Coverage |
| --- | --- | --- |
| Inspect repository root, current branch, status, and Refine-owned artifacts | `FileGitWorktreeService::inspect` | `file_git_worktree_service_lists_status_and_reverts_commits`, `file_git_worktree_service_separates_refine_artifacts_from_user_changes`, `file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes`, `file_git_worktree_service_hard_reset_preserves_refine_runtime_and_removes_other_noise` |
| Read Git audit path in normal and linked-worktree layouts | `FileGitWorktreeService::audit_path` | `file_git_worktree_service_lists_status_and_reverts_commits`, `file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes` |
| Recent changes / commit log | `FileGitWorktreeService::recent_changes`, project-state change projection | `file_git_worktree_service_lists_status_and_reverts_commits`, `web_server_lists_git_changes_and_reverts_commits`, `filters, sorts, and paginates Changes through URL-backed controls`, `visualizes Git changes by day, week, month, and year` |
| Create branch | `FileGitWorktreeService::branch` | `file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes`, `file_git_worktree_service_rejects_invalid_names_and_reports_git_failures` |
| Create worktree | `FileGitWorktreeService::worktree` | `file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes`, `file_git_worktree_service_rejects_invalid_names_and_reports_git_failures` |
| Diff with pathspecs | `FileGitWorktreeService::diff` | `file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes` |
| Merge clean branch | `FileGitWorktreeService::merge` | `file_git_worktree_service_merges_and_rebases_cleanly` |
| Merge conflict and dirty-worktree failure | `FileGitWorktreeService::merge` | `file_git_worktree_service_merges_rebases_and_recovers_conflicts`, `file_git_worktree_service_reports_dirty_worktree_merge_failure` |
| Rebase clean branch | `FileGitWorktreeService::rebase` | `file_git_worktree_service_merges_and_rebases_cleanly` |
| Rebase conflict | `FileGitWorktreeService::rebase` | `file_git_worktree_service_merges_rebases_and_recovers_conflicts` |
| Commit all files and selected pathspecs | `FileGitWorktreeService::commit` | `file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes`; direct Git setup helpers also exercise full-repo commits used by integration tests |
| Push branch to remote and push failure | `FileGitWorktreeService::push` | `file_git_worktree_service_branches_worktrees_diffs_commits_pathspecs_and_pushes`, `file_git_worktree_service_rejects_invalid_names_and_reports_git_failures` |
| Revert commit cleanly | `FileGitWorktreeService::revert_commit` | `file_git_worktree_service_lists_status_and_reverts_commits`, `web_server_lists_git_changes_and_reverts_commits`, `confirms Changes undo, reverts git, and cancels the linked Goal` |
| Revert conflict and invalid refs | `FileGitWorktreeService::revert_commit` | `file_git_worktree_service_revert_conflict_and_recover_preserves_history`, `file_git_worktree_service_rejects_invalid_names_and_reports_git_failures` |
| Hard reset / clean with preservation of Refine-owned artifacts | `FileGitWorktreeService::hard_reset` | `file_git_worktree_service_hard_resets_tracked_changes`, `file_git_worktree_service_hard_reset_preserves_refine_runtime_and_removes_other_noise`, `web_server_hard_resets_git_worktree`, `hard resets the target worktree from the command palette` |
| Recover from merge, rebase, and revert conflicts | `FileGitWorktreeService::recover` | `file_git_worktree_service_merges_rebases_and_recovers_conflicts`, `file_git_worktree_service_revert_conflict_and_recover_preserves_history` |
| Branch name, commit ref, branch collision, worktree collision, missing remote validation | `validate_branch_name`, `validate_commitish`, `FileGitWorktreeService` command wrappers | `file_git_worktree_service_rejects_invalid_names_and_reports_git_failures` |
| Project sync with no Git repository or upstream | `FileGitSyncService::sync` via `/api/project/sync` | `web_server_project_sync_reports_no_git_repo_and_missing_upstream` |
| Commit and push durable `.refine` state, then pull it on another node | `FileGitSyncService::sync` | `sync_commits_pushes_and_pulls_refine_state`, `multi_instance_sync.rs` |
| Rebase and push disjoint state when nodes race | `FileGitSyncService::sync` | `sync_rebases_disjoint_state_when_nodes_race`, `web_server_project_sync_rebases_and_pushes_diverged_branch` |
| Skip safely for uncommitted target-app files | `FileGitSyncService::sync` | `sync_does_not_touch_uncommitted_target_app_changes`, `web_server_project_sync_skips_pull_for_dirty_user_worktree` |
| Ignore runtime-only `.refine` artifacts | `REFINE_STATE_PATHS` exclusions | `web_server_project_sync_ignores_refine_runtime_noise` |
| Project clone | `FileProjectRegistryService::clone_app`, CLI `project clone`, web `/api/apps/clone` | `file_project_registry_clones_and_registers_app`, `project_clone_uses_shared_file_project_registry_service`, `fixture.assert_success("project clone", ...)` in `tests/cli_surface.rs` |
| Changes list API and UI | `handle_changes_list`, `changes.js` | `web_server_lists_git_changes_and_reverts_commits`, `filters, sorts, and paginates Changes through URL-backed controls`, `visualizes Git changes by day, week, month, and year` |
| Changes Undo API and UI | `handle_changes_undo`, `changes.js` | `web_server_lists_git_changes_and_reverts_commits`, `confirms Changes undo, reverts git, and cancels the linked Goal` |
| Hard reset user-facing API/UI | `handle_merger_hard_reset_worktree`, command palette, Node > Processes | `web_server_hard_resets_git_worktree`, `hard resets the target worktree from the command palette`, Processes tab hard-reset enablement assertions in `settings_tabs.spec.ts` |

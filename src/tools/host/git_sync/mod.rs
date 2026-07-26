use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{ErrorKind, Write};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::thread;
use std::time::Duration;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
use crate::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::process::supervisor::errors::{RefineError, RefineResult};
#[cfg(test)]
use crate::tools::host::project_layout::refine_dir_for_target_root;
use crate::tools::host::project_layout::{
    git_common_dir, prepare_refine_dir, state_worktree_for_target_root,
};
use crate::tools::product::nodes::FileNodeRegistryService;

const PUSH_RETRY_LIMIT: usize = 3;
const PUSH_RETRY_DELAY: Duration = Duration::from_millis(100);
pub const REFINE_STATE_BRANCH: &str = "refine/state";
const REFINE_STATE_REF: &str = "refs/heads/refine/state";
const DEFAULT_REMOTE: &str = "origin";
const STATE_BASELINE_FILE: &str = "refine-state-baseline.json";
static REPOSITORY_GIT_LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
static STATE_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
enum GitFetchScope {
    State,
    All,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitSyncResult {
    pub ok: bool,
    pub attempted: bool,
    pub committed: bool,
    pub pulled: bool,
    pub pushed: bool,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub detail: Option<String>,
    /// The repository is temporarily unsafe or busy. The reconciler should retry
    /// without requiring user action.
    pub deferred: bool,
}

#[derive(Clone, Debug)]
pub struct FileGitSyncService {
    pub target_root: PathBuf,
    pub runtime_root: PathBuf,
}

impl FileGitSyncService {
    pub fn new(target_root: impl Into<PathBuf>, runtime_root: impl Into<PathBuf>) -> Self {
        Self {
            target_root: target_root.into(),
            runtime_root: runtime_root.into(),
        }
    }

    /// Synchronize durable `.refine` state through the dedicated
    /// `refine/state` branch. The application branch, index, and worktree are
    /// never checked out, staged, pulled, or pushed by this service.
    pub fn sync(&self) -> RefineResult<GitSyncResult> {
        with_repository_git_lock(&self.target_root, || self.sync_locked(GitFetchScope::All))
    }

    /// Attempt a best-effort background sync without delaying foreground work.
    pub fn try_sync(&self) -> RefineResult<GitSyncResult> {
        self.try_sync_with(GitFetchScope::All)
    }

    /// Publish local Refine state without turning state mutations into full
    /// application-branch fetches. The project update pulse owns that cadence.
    pub fn try_sync_state(&self) -> RefineResult<GitSyncResult> {
        self.try_sync_with(GitFetchScope::State)
    }

    fn try_sync_with(&self, fetch_scope: GitFetchScope) -> RefineResult<GitSyncResult> {
        let lock = repository_git_lock(&self.target_root)?;
        let _guard = match lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                return Ok(deferred(
                    "Repository Git operations are busy; sync will retry on the next cadence.",
                ));
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(RefineError::Conflict(
                    "Repository Git lock was poisoned".to_string(),
                ));
            }
        };
        let _file_guard = match RepositoryFileLock::try_acquire(&self.target_root)? {
            Some(guard) => guard,
            None => {
                return Ok(deferred(
                    "Repository Git operations are busy; sync will retry on the next cadence.",
                ));
            }
        };
        self.sync_locked(fetch_scope)
    }

    /// Fingerprint durable Refine state without invoking Git or touching the
    /// user's checkout. The daemon uses this to debounce nearby mutations.
    pub fn durable_state_fingerprint(&self) -> RefineResult<u64> {
        let root = prepare_refine_dir(&self.target_root)?;
        if !root.exists() {
            return Ok(0);
        }
        let mut files = Vec::new();
        collect_durable_state_files(&root, &root, &mut files)?;
        files.sort();
        let mut hasher = DefaultHasher::new();
        for path in files {
            path.strip_prefix(&root).unwrap_or(&path).hash(&mut hasher);
            fs::read(&path)
                .map_err(|error| {
                    RefineError::Io(format!(
                        "failed to fingerprint durable Refine state {}: {error}",
                        path.display()
                    ))
                })?
                .hash(&mut hasher);
        }
        Ok(hasher.finish())
    }

    fn sync_locked(&self, fetch_scope: GitFetchScope) -> RefineResult<GitSyncResult> {
        if !self.target_root.join(".git").exists() {
            return Ok(skipped("Target app is not a Git repository."));
        }
        if !self.git_success(&["rev-parse", "--is-inside-work-tree"])? {
            return Ok(skipped("Target app is not a Git worktree."));
        }
        let live_refine = prepare_refine_dir(&self.target_root)?;
        self.ensure_local_state_excluded()?;
        let remote = self.configured_remote(&live_refine)?;
        let remote_configured = self.remote_exists(&remote)?;
        let remote_exists = if remote_configured {
            match fetch_scope {
                GitFetchScope::All => {
                    self.fetch_remote(&remote)?;
                    self.remote_state_tracking_exists(&remote)?
                }
                GitFetchScope::State => {
                    let exists = self.remote_state_exists(&remote)?;
                    if exists {
                        self.fetch_state_branch(&remote)?;
                    }
                    exists
                }
            }
        } else {
            false
        };
        let setup = self.ensure_state_worktree(&remote, remote_exists, &live_refine)?;
        let state_root = setup.path;
        let state_refine = state_root.join(".refine");
        let recovered_interrupted_sync = self.recover_interrupted_state_worktree(&state_root)?;
        // The checked-out state branch is not a synchronization baseline: it can
        // advance before a failed first reconciliation has copied remote records
        // into the live store. Persist the last successfully copied state so an
        // absent local record is only interpreted as a deletion after this node
        // has actually observed it.
        let base = self.load_state_baseline()?.unwrap_or_default();
        let local = durable_state_map(&live_refine)?;
        let before = self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?;

        let mut pulled = setup.pulled;
        let mut details = if remote_configured {
            Vec::new()
        } else {
            vec![format!(
                "Git remote {remote} is not configured; Refine state was committed locally."
            )]
        };
        if recovered_interrupted_sync {
            details.push(
                "Recovered an interrupted Refine state copy before reconciling current live state."
                    .to_string(),
            );
        }
        if remote_exists {
            let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
            let remote_head = self.git_stdout(&["rev-parse", &remote_ref])?;
            pulled |= before != remote_head;
            let rebase = self.git_at(&state_root, &["rebase", &remote_ref])?;
            append_output_detail(&mut details, &rebase);
            if !rebase.success {
                let _ = self.git_at(&state_root, &["rebase", "--abort"]);
                return Err(command_failed(&format!("git rebase {remote_ref}"), &rebase));
            }
        }

        let tracked_transient = self
            .git_at_stdout(&state_root, &["ls-files", "--", ".refine"])?
            .lines()
            .filter_map(|path| path.strip_prefix(".refine/"))
            .map(PathBuf::from)
            .filter(|path| is_transient_refine_path(path))
            .collect::<BTreeSet<_>>();
        let removed_transient = remove_transient_state_files(&state_refine)?
            .into_iter()
            .filter(|path| tracked_transient.contains(path))
            .collect::<Vec<_>>();
        let remote_state = durable_state_map(&state_refine)?;
        let resolved_paths = BTreeSet::new();
        let conflicts = state_conflicts(&base, &local, &remote_state, &resolved_paths);
        if !conflicts.is_empty() {
            return Err(RefineError::Conflict(format!(
                "Refine state changed on multiple nodes: {}",
                conflicts.join(", ")
            )));
        }
        apply_local_state_delta(&live_refine, &state_refine, &base, &local, &resolved_paths)?;

        let updated = durable_state_map(&state_refine)?;
        let mut changed = state_change_status(&remote_state, &updated);
        changed.extend(
            removed_transient
                .into_iter()
                .map(|path| format!("D  .refine/{}", path.to_string_lossy().replace('\\', "/"))),
        );
        let delta_committed = !changed.is_empty();
        let mut committed = setup.created || delta_committed;
        let mut commit = if delta_committed {
            self.git_at_checked(&state_root, &["add", "-f", "-A", "--", ".refine"])?;
            let node_id =
                FileNodeRegistryService::with_active_root(&live_refine, &self.runtime_root)
                    .active_node_id()
                    .unwrap_or_else(|_| "default".to_string());
            let summary = state_commit_summary(&changed.join("\n"));
            self.git_at_checked(
                &state_root,
                &["commit", "-m", &summary, "-m", &format!("Node: {node_id}")],
            )?;
            Some(self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?)
        } else if setup.created {
            Some(before.clone())
        } else {
            None
        };

        let mut pushed = false;
        if remote_configured && (!remote_exists || committed || setup.local_ahead) {
            for attempt in 1..=PUSH_RETRY_LIMIT {
                let push =
                    self.git_at(&state_root, &["push", "-u", &remote, REFINE_STATE_BRANCH])?;
                append_output_detail(&mut details, &push);
                if push.success {
                    pushed = true;
                    break;
                }
                if attempt == PUSH_RETRY_LIMIT || !push_rejected_by_race(&push) {
                    return Err(command_failed("git push", &push));
                }
                self.fetch_state_branch(&remote)?;
                let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
                self.git_at_checked(&state_root, &["reset", "--hard", &remote_ref])?;
                remove_transient_state_files(&state_refine)?;
                let retry_remote_state = durable_state_map(&state_refine)?;
                // A rejected push means both the remote and the live store may have advanced
                // since the original reconciliation. Re-evaluate the original observed base
                // against both fresh sides before replaying any local delta.
                let retry_local = durable_state_map(&live_refine)?;
                let retry_resolved_paths = BTreeSet::new();
                let retry_conflicts = state_conflicts(
                    &base,
                    &retry_local,
                    &retry_remote_state,
                    &retry_resolved_paths,
                );
                if !retry_conflicts.is_empty() {
                    return Err(RefineError::Conflict(format!(
                        "Refine state changed on multiple nodes during push retry: {}",
                        retry_conflicts.join(", ")
                    )));
                }
                apply_local_state_delta(
                    &live_refine,
                    &state_refine,
                    &base,
                    &retry_local,
                    &retry_resolved_paths,
                )?;
                let retry_updated = durable_state_map(&state_refine)?;
                let retry_changed = state_change_status(&retry_remote_state, &retry_updated);
                committed = !retry_changed.is_empty();
                if committed {
                    self.git_at_checked(&state_root, &["add", "-f", "-A", "--", ".refine"])?;
                    let node_id =
                        FileNodeRegistryService::with_active_root(&live_refine, &self.runtime_root)
                            .active_node_id()
                            .unwrap_or_else(|_| "default".to_string());
                    let summary = state_commit_summary(&retry_changed.join("\n"));
                    self.git_at_checked(
                        &state_root,
                        &["commit", "-m", &summary, "-m", &format!("Node: {node_id}")],
                    )?;
                }
                pulled = true;
                if committed {
                    commit = Some(self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?);
                } else {
                    commit = None;
                }
                thread::sleep(PUSH_RETRY_DELAY);
            }
        }

        let concurrent_local_change = merge_state_into_live(&state_refine, &live_refine, &local)?;
        self.save_state_baseline(&durable_state_map(&state_refine)?)?;
        if concurrent_local_change {
            details.push(
                "A newer local state mutation arrived during synchronization; it was preserved and will be published in the next batch."
                    .to_string(),
            );
        }
        Ok(GitSyncResult {
            ok: true,
            attempted: true,
            committed,
            pulled,
            pushed,
            branch: Some(REFINE_STATE_BRANCH.to_string()),
            commit,
            detail: nonempty_detail(details),
            deferred: concurrent_local_change,
        })
    }

    fn recover_interrupted_state_worktree(
        &self,
        state_root: &std::path::Path,
    ) -> RefineResult<bool> {
        let tracked_changes = self.git_at_stdout(
            state_root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
                "--",
                ".refine",
            ],
        )?;
        let untracked = self.git_at_stdout(
            state_root,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--",
                ".refine",
            ],
        )?;
        if tracked_changes.is_empty() && untracked.is_empty() {
            return Ok(false);
        }

        if !tracked_changes.is_empty() {
            self.git_at_checked(
                state_root,
                &[
                    "restore",
                    "--source=HEAD",
                    "--staged",
                    "--worktree",
                    "--",
                    ".refine",
                ],
            )?;
        }
        if !untracked.is_empty() {
            self.git_at_checked(state_root, &["clean", "-f", "-d", "-x", "--", ".refine"])?;
        }

        let remaining = self.git_at_stdout(
            state_root,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
                "--",
                ".refine",
            ],
        )?;
        if !remaining.is_empty() {
            return Err(RefineError::Conflict(format!(
                "failed to recover interrupted Refine state synchronization: {remaining}"
            )));
        }
        Ok(true)
    }

    fn fetch_remote(&self, remote: &str) -> RefineResult<()> {
        self.git_checked(&["fetch", "--prune", remote]).map(|_| ())
    }

    fn remote_state_exists(&self, remote: &str) -> RefineResult<bool> {
        Ok(self
            .git(&[
                "ls-remote",
                "--exit-code",
                "--heads",
                remote,
                REFINE_STATE_REF,
            ])?
            .success)
    }

    fn remote_state_tracking_exists(&self, remote: &str) -> RefineResult<bool> {
        self.git_success(&[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/remotes/{remote}/{REFINE_STATE_BRANCH}"),
        ])
    }

    fn fetch_state_branch(&self, remote: &str) -> RefineResult<()> {
        let destination = format!("refs/remotes/{remote}/{REFINE_STATE_BRANCH}");
        let refspec = format!("+{REFINE_STATE_REF}:{destination}");
        self.git_checked(&["fetch", remote, &refspec]).map(|_| ())
    }

    fn ensure_state_worktree(
        &self,
        remote: &str,
        remote_exists: bool,
        live_refine: &std::path::Path,
    ) -> RefineResult<StateWorktreeSetup> {
        let path = state_worktree_for_target_root(&self.target_root)?;
        let valid = path.exists()
            && self
                .git_at(&path, &["rev-parse", "--is-inside-work-tree"])
                .is_ok_and(|output| output.success);
        if valid {
            let branch = self.git_at_stdout(&path, &["branch", "--show-current"])?;
            if branch == REFINE_STATE_BRANCH {
                return Ok(StateWorktreeSetup {
                    path,
                    pulled: false,
                    local_ahead: self.local_state_ahead(remote, remote_exists)?,
                    created: false,
                });
            }
            return Err(RefineError::Conflict(format!(
                "Refine state worktree is on unexpected branch {branch}"
            )));
        }

        self.git_checked(&["worktree", "prune"])?;
        if path.exists() {
            fs::remove_dir_all(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to clean stale Refine state worktree {}: {error}",
                    path.display()
                ))
            })?;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create Refine state worktree parent {}: {error}",
                    parent.display()
                ))
            })?;
        }

        let local_exists =
            self.git_success(&["show-ref", "--verify", "--quiet", REFINE_STATE_REF])?;
        if !local_exists && remote_exists {
            let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
            self.git_checked(&["branch", "--track", REFINE_STATE_BRANCH, &remote_ref])?;
        }
        if local_exists || remote_exists {
            self.git_checked(&[
                "worktree",
                "add",
                path.to_str().unwrap_or_default(),
                REFINE_STATE_BRANCH,
            ])?;
            return Ok(StateWorktreeSetup {
                path,
                pulled: remote_exists && !local_exists,
                local_ahead: local_exists && self.local_state_ahead(remote, remote_exists)?,
                created: false,
            });
        }

        self.git_checked(&[
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap_or_default(),
            "HEAD",
        ])?;
        self.git_at_checked(&path, &["switch", "--orphan", REFINE_STATE_BRANCH])?;
        self.git_at_checked(&path, &["rm", "-rf", "--ignore-unmatch", "."])?;
        replace_live_durable_state(live_refine, &path.join(".refine"))?;
        if path.join(".refine").exists() {
            self.git_at_checked(&path, &["add", "-f", "-A", "--", ".refine"])?;
        }
        let initial = durable_state_map(&path.join(".refine"))?;
        let changes = state_change_status(&BTreeMap::new(), &initial);
        let message = if changes.is_empty() {
            "Initialize Refine state".to_string()
        } else {
            state_commit_summary(&changes.join("\n"))
        };
        self.git_at_checked(&path, &["commit", "--allow-empty", "-m", &message])?;
        Ok(StateWorktreeSetup {
            path,
            pulled: false,
            local_ahead: true,
            created: true,
        })
    }

    fn local_state_ahead(&self, remote: &str, remote_exists: bool) -> RefineResult<bool> {
        if !remote_exists {
            return Ok(true);
        }
        let remote_ref = format!("{remote}/{REFINE_STATE_BRANCH}");
        let range = format!("{remote_ref}..{REFINE_STATE_REF}");
        Ok(self
            .git_stdout(&["rev-list", "--count", &range])?
            .parse::<usize>()
            .unwrap_or(0)
            > 0)
    }

    fn ensure_local_state_excluded(&self) -> RefineResult<()> {
        let exclude = git_common_dir(&self.target_root)?.join("info/exclude");
        let current = fs::read_to_string(&exclude).unwrap_or_default();
        if !current.lines().any(|line| line.trim() == "/.refine/") {
            if let Some(parent) = exclude.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to create Git exclude directory {}: {error}",
                        parent.display()
                    ))
                })?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&exclude)
                .map_err(|error| {
                    RefineError::Io(format!(
                        "failed to open Git exclude file {}: {error}",
                        exclude.display()
                    ))
                })?;
            if !current.is_empty() && !current.ends_with('\n') {
                writeln!(file).map_err(|error| RefineError::Io(error.to_string()))?;
            }
            writeln!(
                file,
                "# Refine control state lives on {REFINE_STATE_BRANCH}\n/.refine/"
            )
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to update Git exclude file {}: {error}",
                    exclude.display()
                ))
            })?;
        }

        Ok(())
    }

    fn configured_remote(&self, refine_dir: &std::path::Path) -> RefineResult<String> {
        let settings =
            FileSettingsService::with_active_root(refine_dir, &self.runtime_root).load()?;
        Ok(settings
            .get("git_remote")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|remote| !remote.is_empty())
            .unwrap_or(DEFAULT_REMOTE)
            .to_string())
    }

    fn load_state_baseline(&self) -> RefineResult<Option<DurableStateMap>> {
        let path = git_common_dir(&self.target_root)?.join(STATE_BASELINE_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read Refine state baseline {}: {error}",
                    path.display()
                )));
            }
        };
        let stored = serde_json::from_slice::<BTreeMap<String, u64>>(&bytes).map_err(|error| {
            RefineError::Io(format!(
                "failed to parse Refine state baseline {}: {error}",
                path.display()
            ))
        })?;
        Ok(Some(
            stored
                .into_iter()
                .map(|(path, fingerprint)| (PathBuf::from(path), fingerprint))
                .collect(),
        ))
    }

    fn save_state_baseline(&self, baseline: &DurableStateMap) -> RefineResult<()> {
        let path = git_common_dir(&self.target_root)?.join(STATE_BASELINE_FILE);
        let stored = baseline
            .iter()
            .map(|(path, fingerprint)| (path.to_string_lossy().replace('\\', "/"), *fingerprint))
            .collect::<BTreeMap<_, _>>();
        let bytes = serde_json::to_vec_pretty(&stored).map_err(|error| {
            RefineError::Io(format!("failed to encode Refine state baseline: {error}"))
        })?;
        let temp = path.with_extension(format!(
            "tmp-{}-{}",
            std::process::id(),
            STATE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = File::create(&temp).map_err(|error| {
            RefineError::Io(format!(
                "failed to create Refine state baseline {}: {error}",
                temp.display()
            ))
        })?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| {
                let _ = fs::remove_file(&temp);
                RefineError::Io(format!(
                    "failed to write Refine state baseline {}: {error}",
                    temp.display()
                ))
            })?;
        fs::rename(&temp, &path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            RefineError::Io(format!(
                "failed to commit Refine state baseline {}: {error}",
                path.display()
            ))
        })
    }

    fn remote_exists(&self, remote: &str) -> RefineResult<bool> {
        Ok(self
            .git_stdout(&["remote"])?
            .lines()
            .any(|candidate| candidate.trim() == remote))
    }

    fn git_stdout(&self, args: &[&str]) -> RefineResult<String> {
        let output = self.git_checked(args)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn git_success(&self, args: &[&str]) -> RefineResult<bool> {
        self.git(args).map(|output| output.success)
    }

    fn git_checked(&self, args: &[&str]) -> RefineResult<GitCommandOutput> {
        let output = self.git(args)?;
        if output.success {
            Ok(output)
        } else {
            Err(command_failed(&format!("git {}", args.join(" ")), &output))
        }
    }

    fn git(&self, args: &[&str]) -> RefineResult<GitCommandOutput> {
        self.git_at(&self.target_root, args)
    }

    fn git_at_stdout(&self, root: &std::path::Path, args: &[&str]) -> RefineResult<String> {
        let output = self.git_at_checked(root, args)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn git_at_checked(
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

    fn git_at(&self, root: &std::path::Path, args: &[&str]) -> RefineResult<GitCommandOutput> {
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

#[derive(Debug)]
struct StateWorktreeSetup {
    path: PathBuf,
    pulled: bool,
    local_ahead: bool,
    created: bool,
}

type DurableStateMap = BTreeMap<PathBuf, u64>;

fn durable_state_map(root: &std::path::Path) -> RefineResult<DurableStateMap> {
    if !root.exists() {
        return Ok(BTreeMap::new());
    }
    let mut files = Vec::new();
    collect_durable_state_files(root, root, &mut files)?;
    let mut state = BTreeMap::new();
    for path in files {
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let bytes = fs::read(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read Refine state {}: {error}",
                path.display()
            ))
        })?;
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        state.insert(relative, hasher.finish());
    }
    Ok(state)
}

fn state_conflicts(
    base: &DurableStateMap,
    local: &DurableStateMap,
    remote: &DurableStateMap,
    resolved: &BTreeSet<PathBuf>,
) -> Vec<String> {
    let paths = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    paths
        .into_iter()
        .filter(|path| {
            if resolved.contains(path) {
                return false;
            }
            let base_value = base.get(path);
            let local_value = local.get(path);
            let remote_value = remote.get(path);
            local_value != base_value && remote_value != base_value && local_value != remote_value
        })
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

fn state_change_status(before: &DurableStateMap, after: &DurableStateMap) -> Vec<String> {
    before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|path| {
            let status = match (before.get(&path), after.get(&path)) {
                (None, Some(_)) => "A",
                (Some(_), None) => "D",
                (Some(left), Some(right)) if left != right => "M",
                _ => return None,
            };
            Some(format!(
                "{status}  .refine/{}",
                path.to_string_lossy().replace('\\', "/")
            ))
        })
        .collect()
}

fn apply_local_state_delta(
    live_root: &std::path::Path,
    state_root: &std::path::Path,
    base: &DurableStateMap,
    local: &DurableStateMap,
    resolved: &BTreeSet<PathBuf>,
) -> RefineResult<()> {
    let paths = base
        .keys()
        .chain(local.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for relative in paths {
        if resolved.contains(&relative) {
            continue;
        }
        if local.get(&relative) == base.get(&relative) {
            continue;
        }
        let destination = state_root.join(&relative);
        if local.contains_key(&relative) {
            copy_state_file(&live_root.join(&relative), &destination)?;
        } else if destination.exists() {
            fs::remove_file(&destination).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove synchronized Refine state {}: {error}",
                    destination.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn replace_live_durable_state(
    source_root: &std::path::Path,
    destination_root: &std::path::Path,
) -> RefineResult<()> {
    let existing = durable_state_map(destination_root)?;
    for relative in existing.keys() {
        let path = destination_root.join(relative);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| {
                RefineError::Io(format!(
                    "failed to replace Refine state {}: {error}",
                    path.display()
                ))
            })?;
        }
    }
    let source = durable_state_map(source_root)?;
    for relative in source.keys() {
        copy_state_file(
            &source_root.join(relative),
            &destination_root.join(relative),
        )?;
    }
    Ok(())
}

fn merge_state_into_live(
    source_root: &std::path::Path,
    live_root: &std::path::Path,
    original_local: &DurableStateMap,
) -> RefineResult<bool> {
    let source = durable_state_map(source_root)?;
    let current = durable_state_map(live_root)?;
    let concurrent_change = current != *original_local;
    let paths = source
        .keys()
        .chain(current.keys())
        .chain(original_local.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for relative in paths {
        // A mutation that completed after this sync captured its local snapshot
        // wins this copy-back. The daemon will publish it in the next batch.
        if current.get(&relative) != original_local.get(&relative) {
            continue;
        }
        let destination = live_root.join(&relative);
        if source.contains_key(&relative) {
            copy_state_file(&source_root.join(&relative), &destination)?;
        } else if destination.exists() {
            fs::remove_file(&destination).map_err(|error| {
                RefineError::Io(format!(
                    "failed to remove synchronized Refine state {}: {error}",
                    destination.display()
                ))
            })?;
        }
    }
    Ok(concurrent_change)
}

fn copy_state_file(source: &std::path::Path, destination: &std::path::Path) -> RefineResult<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create Refine state directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let parent = destination
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let temp = parent.join(format!(
        ".refine-sync-{}-{}",
        std::process::id(),
        STATE_COPY_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::copy(source, &temp) {
        let _ = fs::remove_file(&temp);
        return Err(RefineError::Io(format!(
            "failed to copy Refine state {} to {}: {error}",
            source.display(),
            temp.display()
        )));
    }
    fs::rename(&temp, destination).map_err(|error| {
        let _ = fs::remove_file(&temp);
        RefineError::Io(format!(
            "failed to commit synchronized Refine state {}: {error}",
            destination.display()
        ))
    })
}

fn state_commit_summary(status: &str) -> String {
    let mut goals = BTreeSet::new();
    let mut features = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    let mut other = 0usize;
    for line in status.lines() {
        let path = line.get(3..).unwrap_or("").trim().replace('\\', "/");
        if let Some(record) = state_record_key(&path, ".refine/goals/") {
            goals.insert(record);
        } else if let Some(record) = state_record_key(&path, ".refine/features/") {
            features.insert(record);
        } else if let Some(record) = state_record_key(&path, ".refine/nodes/") {
            nodes.insert(record);
        } else {
            other += 1;
        }
    }
    let mut parts = Vec::new();
    if !goals.is_empty() {
        parts.push(format!("{} goal{}", goals.len(), plural(goals.len())));
    }
    if !features.is_empty() {
        parts.push(format!(
            "{} feature{}",
            features.len(),
            plural(features.len())
        ));
    }
    if !nodes.is_empty() {
        parts.push(format!("{} node{}", nodes.len(), plural(nodes.len())));
    }
    if other > 0 || parts.is_empty() {
        parts.push(format!("{other} other file{}", plural(other)));
    }
    format!("Sync Refine state: {}", parts.join(", "))
}

fn state_record_key(path: &str, prefix: &str) -> Option<String> {
    let relative = path.strip_prefix(prefix)?;
    std::path::Path::new(relative)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().replace('\\', "/"))
        .or_else(|| Some(relative.to_string()))
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

pub fn with_repository_git_lock<T>(
    target_root: &std::path::Path,
    action: impl FnOnce() -> RefineResult<T>,
) -> RefineResult<T> {
    let lock = repository_git_lock(target_root)?;
    let _guard = lock
        .lock()
        .map_err(|_| RefineError::Conflict("Repository Git lock was poisoned".to_string()))?;
    let _file_guard = RepositoryFileLock::acquire(target_root)?;
    action()
}

fn repository_git_lock(target_root: &std::path::Path) -> RefineResult<Arc<Mutex<()>>> {
    let key = target_root
        .canonicalize()
        .unwrap_or_else(|_| target_root.to_path_buf());
    {
        let mut locks = REPOSITORY_GIT_LOCKS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .map_err(|_| RefineError::Conflict("Git lock registry was poisoned".to_string()))?;
        Ok(Arc::clone(
            locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))),
        ))
    }
}

fn skipped(detail: &str) -> GitSyncResult {
    GitSyncResult {
        ok: true,
        detail: Some(detail.to_string()),
        ..GitSyncResult::default()
    }
}

fn deferred(detail: &str) -> GitSyncResult {
    GitSyncResult {
        ok: true,
        detail: Some(detail.to_string()),
        deferred: true,
        ..GitSyncResult::default()
    }
}

#[derive(Debug)]
struct GitCommandOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct RepositoryFileLock {
    file: Option<File>,
}

impl RepositoryFileLock {
    fn acquire(target_root: &std::path::Path) -> RefineResult<Self> {
        let Some(file) = repository_lock_file(target_root)? else {
            return Ok(Self { file: None });
        };
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!(
                "failed to lock repository {}: {error}",
                target_root.display()
            ))
        })?;
        Ok(Self { file: Some(file) })
    }

    fn try_acquire(target_root: &std::path::Path) -> RefineResult<Option<Self>> {
        let Some(file) = repository_lock_file(target_root)? else {
            return Ok(Some(Self { file: None }));
        };
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file: Some(file) })),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(RefineError::Io(format!(
                "failed to lock repository {}: {error}",
                target_root.display()
            ))),
        }
    }
}

impl Drop for RepositoryFileLock {
    fn drop(&mut self) {
        if let Some(file) = &self.file {
            let _ = FileExt::unlock(file);
        }
    }
}

fn repository_lock_file(target_root: &std::path::Path) -> RefineResult<Option<File>> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(target_root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| RefineError::Io(format!("failed to locate Git directory: {error}")))?;
    if !output.status.success() {
        return Ok(None);
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Ok(None);
    }
    let common_dir = PathBuf::from(raw);
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        target_root.join(common_dir)
    };
    let path = common_dir.join("refine-repository.lock");
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map(Some)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open repository lock {}: {error}",
                path.display()
            ))
        })
}

fn collect_durable_state_files(
    root: &std::path::Path,
    current: &std::path::Path,
    files: &mut Vec<PathBuf>,
) -> RefineResult<()> {
    for entry in fs::read_dir(current).map_err(|error| {
        RefineError::Io(format!(
            "failed to inspect durable Refine state {}: {error}",
            current.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect durable Refine state entry: {error}"
            ))
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if is_runtime_only_refine_path(relative) || is_transient_refine_path(relative) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect durable Refine state {}: {error}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            collect_durable_state_files(root, &path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn remove_transient_state_files(root: &std::path::Path) -> RefineResult<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut removed = Vec::new();
    remove_transient_state_files_from(root, root, &mut removed)?;
    Ok(removed)
}

fn remove_transient_state_files_from(
    root: &std::path::Path,
    current: &std::path::Path,
    removed: &mut Vec<PathBuf>,
) -> RefineResult<()> {
    for entry in fs::read_dir(current).map_err(|error| {
        RefineError::Io(format!(
            "failed to inspect synchronized Refine state {}: {error}",
            current.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect synchronized Refine state entry: {error}"
            ))
        })?;
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path);
        if is_transient_refine_path(relative) {
            match fs::remove_file(&path) {
                Ok(()) => removed.push(relative.to_path_buf()),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to remove transient Refine state {}: {error}",
                        path.display()
                    )));
                }
            }
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to inspect synchronized Refine state {}: {error}",
                    path.display()
                )));
            }
        };
        if file_type.is_dir() {
            remove_transient_state_files_from(root, &path, removed)?;
        }
    }
    Ok(())
}

fn is_transient_refine_path(path: &std::path::Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    file_name.ends_with(".lock")
        || file_name.ends_with(".tmp")
        || file_name.starts_with(".refine-sync-")
}

fn is_runtime_only_refine_path(path: &std::path::Path) -> bool {
    matches!(
        path.components()
            .next()
            .and_then(|component| component.as_os_str().to_str()),
        Some("run" | "runtime" | "logs" | "support-bundles" | "provider-bin")
    ) || path == std::path::Path::new("manage-app.log")
}

fn append_output_detail(details: &mut Vec<String>, output: &GitCommandOutput) {
    for text in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(text).trim().to_string();
        if !text.is_empty() {
            details.push(text);
        }
    }
}

fn nonempty_detail(details: Vec<String>) -> Option<String> {
    let detail = details.join("\n");
    (!detail.is_empty()).then_some(detail)
}

fn push_rejected_by_race(output: &GitCommandOutput) -> bool {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    text.contains("rejected") || text.contains("non-fast-forward") || text.contains("fetch first")
}

fn command_failed(command: &str, output: &GitCommandOutput) -> RefineError {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    RefineError::Conflict(format!(
        "{command} failed{}",
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    ))
}

#[cfg(test)]
mod tests;

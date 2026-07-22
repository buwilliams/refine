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
        let checked_out = durable_state_map(&state_refine)?;
        // A node seeing the state branch for the first time has no synchronized
        // baseline yet. Its absent local files are not deletions of records
        // that already exist remotely.
        let base = if setup.pulled {
            BTreeMap::new()
        } else {
            checked_out
        };
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
        let conflicts = state_conflicts(&base, &local, &remote_state);
        if !conflicts.is_empty() {
            return Err(RefineError::Conflict(format!(
                "Refine state changed on multiple nodes: {}",
                conflicts.join(", ")
            )));
        }
        apply_local_state_delta(&live_refine, &state_refine, &base, &local)?;

        let updated = durable_state_map(&state_refine)?;
        let mut changed = state_change_status(&remote_state, &updated);
        changed.extend(
            removed_transient
                .into_iter()
                .map(|path| format!("D  .refine/{}", path.to_string_lossy().replace('\\', "/"))),
        );
        let delta_committed = !changed.is_empty();
        let committed = setup.created || delta_committed;
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
                let rebase = self.git_at(&state_root, &["rebase", &remote_ref])?;
                append_output_detail(&mut details, &rebase);
                if !rebase.success {
                    let _ = self.git_at(&state_root, &["rebase", "--abort"]);
                    return Err(command_failed(&format!("git rebase {remote_ref}"), &rebase));
                }
                pulled = true;
                if committed {
                    commit = Some(self.git_at_stdout(&state_root, &["rev-parse", "HEAD"])?);
                }
                thread::sleep(PUSH_RETRY_DELAY);
            }
        }

        let concurrent_local_change = merge_state_into_live(&state_refine, &live_refine, &local)?;
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
) -> RefineResult<()> {
    let paths = base
        .keys()
        .chain(local.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for relative in paths {
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
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn sync_commits_pushes_and_pulls_refine_state() {
        let fixture = SyncFixture::new("round-trip");
        write_goal(&fixture.a, "GOALA");

        let pushed = fixture.service(&fixture.a).sync().unwrap();
        assert!(pushed.ok && pushed.committed && pushed.pushed, "{pushed:?}");

        let pulled = fixture.service(&fixture.b).sync().unwrap();
        assert!(pulled.ok && pulled.pulled && !pulled.pushed, "{pulled:?}");
        assert!(
            refine_dir_for_target_root(&fixture.b)
                .unwrap()
                .join("goals/GOALA/goal.json")
                .exists()
        );
        assert!(!fixture.a.join(".refine").exists());
        assert!(!fixture.b.join(".refine").exists());
        let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
        assert_eq!(state_worktree, fixture.a.join(".git/refine-state-worktree"));
        assert!(
            state_worktree
                .join(".refine/goals/GOALA/goal.json")
                .exists()
        );
        assert_eq!(
            git_stdout(&fixture.a, &["branch", "--show-current"]),
            "main"
        );
        assert_eq!(
            git_stdout(&fixture.b, &["branch", "--show-current"]),
            "main"
        );
        assert_eq!(
            git_stdout(
                &fixture.a,
                &["ls-tree", "-r", "--name-only", "refine/state"]
            ),
            ".refine/goals/GOALA/goal.json"
        );
    }

    #[test]
    fn sync_rebases_disjoint_state_when_nodes_race() {
        let fixture = SyncFixture::new("race");
        write_goal(&fixture.a, "GOALA");
        write_goal(&fixture.b, "GOALB");

        fixture.service(&fixture.a).sync().unwrap();
        let second = fixture.service(&fixture.b).sync().unwrap();
        assert!(
            second.committed && second.pulled && second.pushed,
            "{second:?}"
        );

        fixture.service(&fixture.a).sync().unwrap();
        let refine_dir = refine_dir_for_target_root(&fixture.a).unwrap();
        assert!(refine_dir.join("goals/GOALA/goal.json").exists());
        assert!(refine_dir.join("goals/GOALB/goal.json").exists());
    }

    #[test]
    fn sync_recovers_completed_state_copy_interrupted_before_commit() {
        let fixture = SyncFixture::new("interrupted-copy-restart");
        write_goal(&fixture.a, "GOALA");
        fixture.service(&fixture.a).sync().unwrap();
        fixture.service(&fixture.b).sync().unwrap();

        let live_goal = refine_dir_for_target_root(&fixture.a)
            .unwrap()
            .join("goals/GOALA/goal.json");
        let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
        let state_goal = state_worktree.join(".refine/goals/GOALA/goal.json");
        fs::write(&live_goal, "{\"id\":\"GOALA\",\"status\":\"copied\"}\n").unwrap();
        copy_state_file(&live_goal, &state_goal).unwrap();
        assert_eq!(
            git_stdout(&state_worktree, &["status", "--short"]),
            "M .refine/goals/GOALA/goal.json"
        );

        write_goal(&fixture.b, "GOALB");
        fixture.service(&fixture.b).sync().unwrap();
        let remote_before_recovery =
            git_stdout(&fixture.a, &["ls-remote", "origin", REFINE_STATE_REF])
                .split_whitespace()
                .next()
                .unwrap()
                .to_string();
        fs::write(&live_goal, "{\"id\":\"GOALA\",\"status\":\"concurrent\"}\n").unwrap();

        let recovered = fixture.service(&fixture.a).sync().unwrap();

        assert!(
            recovered.committed && recovered.pulled && recovered.pushed,
            "{recovered:?}"
        );
        assert!(
            recovered
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("Recovered an interrupted Refine state copy")),
            "{recovered:?}"
        );
        assert_eq!(git_stdout(&state_worktree, &["status", "--short"]), "");
        assert_eq!(
            fs::read_to_string(&live_goal).unwrap(),
            "{\"id\":\"GOALA\",\"status\":\"concurrent\"}\n"
        );
        assert_eq!(
            git_stdout(
                &fixture.a,
                &["show", "origin/refine/state:.refine/goals/GOALA/goal.json",],
            ),
            "{\"id\":\"GOALA\",\"status\":\"concurrent\"}"
        );
        assert!(
            !git_stdout(
                &fixture.a,
                &["show", "origin/refine/state:.refine/goals/GOALB/goal.json",],
            )
            .is_empty()
        );
        git(
            &fixture.a,
            &[
                "merge-base",
                "--is-ancestor",
                &remote_before_recovery,
                "origin/refine/state",
            ],
        );
    }

    #[test]
    fn sync_skips_noop_commits_and_summarizes_batches() {
        let fixture = SyncFixture::new("batch");
        write_goal(&fixture.a, "GOALA");
        write_goal(&fixture.a, "GOALB");

        let first = fixture.service(&fixture.a).sync().unwrap();
        assert!(first.committed && first.pushed, "{first:?}");
        let subject = git_stdout(&fixture.a, &["log", "-1", "--format=%s", "refine/state"]);
        assert_eq!(subject, "Sync Refine state: 2 goals");

        let second = fixture.service(&fixture.a).sync().unwrap();
        assert!(!second.committed && !second.pushed, "{second:?}");
        assert_eq!(
            git_stdout(&fixture.a, &["rev-list", "--count", "refine/state"]),
            "1"
        );
    }

    #[test]
    fn sync_reports_same_record_multi_node_conflicts() {
        let fixture = SyncFixture::new("same-record-conflict");
        write_goal(&fixture.a, "GOALA");
        fixture.service(&fixture.a).sync().unwrap();
        fixture.service(&fixture.b).sync().unwrap();

        fs::write(
            refine_dir_for_target_root(&fixture.a)
                .unwrap()
                .join("goals/GOALA/goal.json"),
            "{\"id\":\"GOALA\",\"status\":\"review\"}\n",
        )
        .unwrap();
        fs::write(
            refine_dir_for_target_root(&fixture.b)
                .unwrap()
                .join("goals/GOALA/goal.json"),
            "{\"id\":\"GOALA\",\"status\":\"qa\"}\n",
        )
        .unwrap();
        fixture.service(&fixture.a).sync().unwrap();

        let error = fixture.service(&fixture.b).sync().unwrap_err();
        assert!(
            error.to_string().contains("goals/GOALA/goal.json"),
            "{error}"
        );
    }

    #[test]
    fn state_commit_summary_counts_sharded_records() {
        assert_eq!(
            state_commit_summary(
                "M  .refine/goals/GO/AL1/goal.json\nM  .refine/goals/GO/AL2/goal.json"
            ),
            "Sync Refine state: 2 goals"
        );
    }

    #[test]
    fn durable_state_ignores_transient_lock_temp_and_copy_files() {
        let root = unique_temp_dir("transient-state");
        let sessions = root.join("chat/sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("session.json"), "{}\n").unwrap();
        fs::write(sessions.join(".session.lock"), "").unwrap();
        fs::write(sessions.join("session.json.interrupted.tmp"), "partial\n").unwrap();
        fs::write(sessions.join(".refine-sync-123-0"), "partial\n").unwrap();
        fs::write(root.join("supervisor-agent.lock"), "").unwrap();

        let state = durable_state_map(&root).unwrap();

        assert_eq!(
            state.keys().cloned().collect::<Vec<_>>(),
            vec![PathBuf::from("chat/sessions/session.json")]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_does_not_publish_transient_state_artifacts() {
        let fixture = SyncFixture::new("transient-artifacts");
        write_goal(&fixture.a, "GOALA");
        let refine_dir = refine_dir_for_target_root(&fixture.a).unwrap();
        let sessions = refine_dir.join("chat/sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join(".session.lock"), "").unwrap();
        fs::write(sessions.join("session.json.interrupted.tmp"), "partial\n").unwrap();
        fs::write(sessions.join(".refine-sync-123-0"), "partial\n").unwrap();

        let result = fixture.service(&fixture.a).sync().unwrap();

        assert!(result.committed && result.pushed, "{result:?}");
        assert_eq!(
            git_stdout(
                &fixture.a,
                &["ls-tree", "-r", "--name-only", REFINE_STATE_BRANCH]
            ),
            ".refine/goals/GOALA/goal.json"
        );
    }

    #[test]
    fn sync_removes_transient_artifacts_already_on_state_branch() {
        let fixture = SyncFixture::new("stale-transient-artifacts");
        write_goal(&fixture.a, "GOALA");
        fixture.service(&fixture.a).sync().unwrap();
        let state_worktree = state_worktree_for_target_root(&fixture.a).unwrap();
        let stale = state_worktree.join(".refine/chat/sessions/.session.lock");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, "stale\n").unwrap();
        git(&state_worktree, &["add", "-f", ".refine"]);
        git(
            &state_worktree,
            &["commit", "-q", "-m", "publish stale lock"],
        );
        git(
            &state_worktree,
            &["push", "-q", "origin", REFINE_STATE_BRANCH],
        );

        let result = fixture.service(&fixture.a).sync().unwrap();

        assert!(result.committed && result.pushed, "{result:?}");
        assert!(!stale.exists());
        assert_eq!(
            git_stdout(
                &fixture.a,
                &["ls-tree", "-r", "--name-only", REFINE_STATE_BRANCH]
            ),
            ".refine/goals/GOALA/goal.json"
        );
    }

    #[test]
    fn failed_state_copy_removes_its_partial_temp_file() {
        let root = unique_temp_dir("failed-copy-cleanup");
        let source = root.join("source-directory");
        let destination = root.join("destination/state.json");
        fs::create_dir_all(&source).unwrap();

        assert!(copy_state_file(&source, &destination).is_err());
        assert_eq!(
            fs::read_dir(destination.parent().unwrap()).unwrap().count(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_back_preserves_mutations_that_arrive_during_sync() {
        let root = unique_temp_dir("copy-back-race");
        let live = root.join("live");
        let state = root.join("state");
        fs::create_dir_all(&live).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(live.join("goal.json"), "before\n").unwrap();
        let original = durable_state_map(&live).unwrap();
        fs::write(live.join("goal.json"), "concurrent\n").unwrap();
        fs::write(state.join("goal.json"), "remote\n").unwrap();
        fs::write(state.join("remote.json"), "remote-only\n").unwrap();

        assert!(merge_state_into_live(&state, &live, &original).unwrap());
        assert_eq!(
            fs::read_to_string(live.join("goal.json")).unwrap(),
            "concurrent\n"
        );
        assert_eq!(
            fs::read_to_string(live.join("remote.json")).unwrap(),
            "remote-only\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_does_not_touch_uncommitted_target_app_changes() {
        let fixture = SyncFixture::new("dirty");
        fs::write(fixture.a.join("app.txt"), "dirty\n").unwrap();
        write_goal(&fixture.a, "GOALA");
        let head = git_stdout(&fixture.a, &["rev-parse", "HEAD"]);

        let result = fixture.service(&fixture.a).sync().unwrap();
        assert!(result.attempted && result.committed && result.pushed);
        assert!(
            refine_dir_for_target_root(&fixture.a)
                .unwrap()
                .join("goals/GOALA/goal.json")
                .exists()
        );
        assert_eq!(git_stdout(&fixture.a, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            fs::read_to_string(fixture.a.join("app.txt")).unwrap(),
            "dirty\n"
        );
    }

    #[test]
    fn state_demand_fetches_only_state_while_project_pulse_fetches_all_branches() {
        let fixture = SyncFixture::new("fetch-scopes");
        let original_remote_main = git_stdout(&fixture.a, &["rev-parse", "origin/main"]);
        fs::write(fixture.b.join("app.txt"), "human change\n").unwrap();
        git(&fixture.b, &["add", "app.txt"]);
        git(&fixture.b, &["commit", "-q", "-m", "human change"]);
        git(&fixture.b, &["push", "-q", "origin", "main"]);
        let human_commit = git_stdout(&fixture.b, &["rev-parse", "HEAD"]);
        assert_ne!(human_commit, original_remote_main);

        write_goal(&fixture.a, "GOALA");
        fixture.service(&fixture.a).try_sync_state().unwrap();
        assert_eq!(
            git_stdout(&fixture.a, &["rev-parse", "origin/main"]),
            original_remote_main
        );

        fixture.service(&fixture.a).try_sync().unwrap();
        assert_eq!(
            git_stdout(&fixture.a, &["rev-parse", "origin/main"]),
            human_commit
        );
        assert_eq!(
            git_stdout(&fixture.a, &["branch", "--show-current"]),
            "main"
        );
        assert_eq!(
            git_stdout(&fixture.a, &["rev-parse", "HEAD"]),
            original_remote_main
        );
    }

    #[test]
    fn sync_requires_legacy_state_to_be_removed_from_application_branch() {
        let fixture = SyncFixture::new("legacy-tracked");
        let legacy_goal = fixture.a.join(".refine/goals/GOALA");
        fs::create_dir_all(&legacy_goal).unwrap();
        fs::write(legacy_goal.join("goal.json"), "{\"id\":\"GOALA\"}\n").unwrap();
        git(&fixture.a, &["add", ".refine"]);
        git(&fixture.a, &["commit", "-m", "legacy Refine state"]);
        fs::write(
            fixture.a.join(".refine/goals/GOALA/goal.json"),
            "{\"id\":\"GOALA\",\"status\":\"review\"}\n",
        )
        .unwrap();

        let error = fixture.service(&fixture.a).sync().unwrap_err();
        assert!(error.to_string().contains("still tracks legacy .refine"));
        assert!(!fixture.a.join(".refine").exists());

        git(&fixture.a, &["add", "-u", "--", ".refine"]);
        git(&fixture.a, &["commit", "-m", "Remove legacy Refine state"]);
        let app_head = git_stdout(&fixture.a, &["rev-parse", "HEAD"]);
        let result = fixture.service(&fixture.a).sync().unwrap();
        assert!(result.committed && result.pushed, "{result:?}");
        assert_eq!(git_stdout(&fixture.a, &["rev-parse", "HEAD"]), app_head);
        assert_eq!(git_stdout(&fixture.a, &["status", "--porcelain"]), "");
        assert!(!fixture.a.join(".refine").exists());
        assert!(
            git_stdout(
                &fixture.a,
                &["show", "refine/state:.refine/goals/GOALA/goal.json"]
            )
            .contains("review")
        );
    }

    #[test]
    fn sync_uses_the_configured_git_remote() {
        let fixture = SyncFixture::new("configured-remote");
        git(&fixture.a, &["remote", "rename", "origin", "upstream"]);
        let refine_dir = refine_dir_for_target_root(&fixture.a).unwrap();
        FileSettingsService::new(&refine_dir)
            .update(&json!({"git_remote": "upstream"}))
            .unwrap();
        write_goal(&fixture.a, "GOALA");

        let result = fixture.service(&fixture.a).sync().unwrap();

        assert!(result.pushed, "{result:?}");
        assert!(!fixture.a.join(".refine").exists());
        assert!(!git_stdout(&fixture.a, &["ls-remote", "upstream", REFINE_STATE_REF]).is_empty());
    }

    struct SyncFixture {
        root: PathBuf,
        a: PathBuf,
        b: PathBuf,
    }

    impl SyncFixture {
        fn new(name: &str) -> Self {
            let root = unique_temp_dir(name);
            let remote = root.join("remote.git");
            let seed = root.join("seed");
            let a = root.join("a");
            let b = root.join("b");
            fs::create_dir_all(&seed).unwrap();
            git(&root, &["init", "--bare", remote.to_str().unwrap()]);
            git(&seed, &["init", "-q"]);
            configure(&seed);
            fs::write(seed.join("app.txt"), "base\n").unwrap();
            git(&seed, &["add", "app.txt"]);
            git(&seed, &["commit", "-q", "-m", "initial"]);
            git(&seed, &["branch", "-M", "main"]);
            git(
                &seed,
                &["remote", "add", "origin", remote.to_str().unwrap()],
            );
            git(&seed, &["push", "-q", "-u", "origin", "main"]);
            git(
                &root,
                &["clone", "-q", remote.to_str().unwrap(), a.to_str().unwrap()],
            );
            git(
                &root,
                &["clone", "-q", remote.to_str().unwrap(), b.to_str().unwrap()],
            );
            configure(&a);
            configure(&b);
            Self { root, a, b }
        }

        fn service(&self, root: &Path) -> FileGitSyncService {
            FileGitSyncService::new(root, root.join("run"))
        }
    }

    impl Drop for SyncFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write_goal(root: &Path, id: &str) {
        let dir = refine_dir_for_target_root(root)
            .unwrap()
            .join("goals")
            .join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("goal.json"), format!("{{\"id\":\"{id}\"}}\n")).unwrap();
    }

    fn configure(root: &Path) {
        git(root, &["config", "user.email", "sync@test"]);
        git(root, &["config", "user.name", "Sync Test"]);
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-git-sync-{name}-{}-{nanos}",
            std::process::id()
        ))
    }
}

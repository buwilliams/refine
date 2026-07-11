use std::collections::BTreeMap;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::product::nodes::FileNodeRegistryService;

const PUSH_RETRY_LIMIT: usize = 3;
const PUSH_RETRY_DELAY: Duration = Duration::from_millis(100);
static REPOSITORY_GIT_LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

// Runtime-only Refine artifacts must never become part of the shared state
// commit. Everything else under .refine is durable collaboration state.
const REFINE_STATE_PATHS: &[&str] = &[
    ".refine",
    ":(exclude).refine/run",
    ":(exclude).refine/run/**",
    ":(exclude).refine/runtime",
    ":(exclude).refine/runtime/**",
    ":(exclude).refine/logs",
    ":(exclude).refine/logs/**",
    ":(exclude).refine/support-bundles",
    ":(exclude).refine/support-bundles/**",
    ":(exclude).refine/provider-bin",
    ":(exclude).refine/provider-bin/**",
    ":(exclude).refine/manage-app.log",
];

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

    /// Synchronize the current branch through its configured upstream.
    ///
    /// Refine commits only durable `.refine` state, rebases onto the upstream,
    /// and pushes all local commits (including workflow merge commits). A push
    /// rejected because another node won the race is retried after another
    /// pull/rebase. Uncommitted target-app changes are never staged or stashed;
    /// sync waits for the worktree to become safe instead.
    pub fn sync(&self) -> RefineResult<GitSyncResult> {
        let lock = repository_git_lock(&self.target_root)?;
        let _guard = lock
            .lock()
            .map_err(|_| RefineError::Conflict("Repository Git lock was poisoned".to_string()))?;
        self.sync_locked()
    }

    /// Attempt a best-effort background sync without delaying foreground work.
    pub fn try_sync(&self) -> RefineResult<GitSyncResult> {
        let lock = repository_git_lock(&self.target_root)?;
        let _guard = match lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                return Ok(skipped(
                    "Repository Git operations are busy; sync will retry on the next cadence.",
                ));
            }
            Err(TryLockError::Poisoned(_)) => {
                return Err(RefineError::Conflict(
                    "Repository Git lock was poisoned".to_string(),
                ));
            }
        };
        self.sync_locked()
    }

    fn sync_locked(&self) -> RefineResult<GitSyncResult> {
        if !self.target_root.join(".git").exists() {
            return Ok(skipped("Target app is not a Git repository."));
        }
        if !self.git_success(&["rev-parse", "--is-inside-work-tree"])? {
            return Ok(skipped("Target app is not a Git worktree."));
        }

        let branch = self.git_stdout(&["branch", "--show-current"])?;
        if branch.is_empty() {
            return Ok(skipped(
                "Detached HEAD cannot be synchronized automatically.",
            ));
        }
        let upstream = self.git(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])?;
        if !upstream.status.success() {
            return Ok(GitSyncResult {
                ok: true,
                branch: Some(branch),
                detail: Some("No upstream branch is configured.".to_string()),
                ..GitSyncResult::default()
            });
        }

        let status = self.git_checked(&["status", "--porcelain=v1", "-uall"])?;
        if let Some(path) = first_non_refine_change(&String::from_utf8_lossy(&status.stdout)) {
            return Ok(GitSyncResult {
                ok: true,
                branch: Some(branch),
                detail: Some(format!(
                    "Uncommitted target-app changes are present ({path}); sync will retry after they are committed or removed."
                )),
                ..GitSyncResult::default()
            });
        }

        self.stage_refine_state()?;
        let committed = self.has_staged_refine_state()?;
        let commit = if committed {
            let node_id = FileNodeRegistryService::with_active_root(
                self.target_root.join(".refine"),
                &self.runtime_root,
            )
            .active_node_id()
            .unwrap_or_else(|_| "default".to_string());
            self.git_checked(&["commit", "-m", &format!("Sync Refine state from {node_id}")])?;
            Some(self.git_stdout(&["rev-parse", "HEAD"])?)
        } else {
            None
        };

        let mut pulled = false;
        let mut details = Vec::new();
        for attempt in 1..=PUSH_RETRY_LIMIT {
            let before_pull = self.git_stdout(&["rev-parse", "HEAD"])?;
            let pull = self.git(&["pull", "--rebase"])?;
            if !pull.status.success() {
                let _ = self.git(&["rebase", "--abort"]);
                return Err(command_failed("git pull --rebase", &pull));
            }
            let after_pull = self.git_stdout(&["rev-parse", "HEAD"])?;
            pulled |= before_pull != after_pull;
            append_output_detail(&mut details, &pull);

            let push = self.git(&["push"])?;
            append_output_detail(&mut details, &push);
            if push.status.success() {
                return Ok(GitSyncResult {
                    ok: true,
                    attempted: true,
                    committed,
                    pulled,
                    pushed: true,
                    branch: Some(branch),
                    commit,
                    detail: nonempty_detail(details),
                });
            }
            if attempt == PUSH_RETRY_LIMIT || !push_rejected_by_race(&push) {
                return Err(command_failed("git push", &push));
            }
            thread::sleep(PUSH_RETRY_DELAY);
        }

        unreachable!("push retry loop always returns")
    }

    fn stage_refine_state(&self) -> RefineResult<()> {
        if !self.target_root.join(".refine").exists() {
            return Ok(());
        }
        let mut args = vec!["add", "-A", "--"];
        args.extend_from_slice(REFINE_STATE_PATHS);
        self.git_checked(&args).map(|_| ())
    }

    fn has_staged_refine_state(&self) -> RefineResult<bool> {
        if !self.target_root.join(".refine").exists() {
            return Ok(false);
        }
        let output = self.git(&["diff", "--cached", "--quiet", "--", ".refine"])?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(command_failed("git diff --cached --quiet", &output)),
        }
    }

    fn git_stdout(&self, args: &[&str]) -> RefineResult<String> {
        let output = self.git_checked(args)?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn git_success(&self, args: &[&str]) -> RefineResult<bool> {
        self.git(args).map(|output| output.status.success())
    }

    fn git_checked(&self, args: &[&str]) -> RefineResult<Output> {
        let output = self.git(args)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(command_failed(&format!("git {}", args.join(" ")), &output))
        }
    }

    fn git(&self, args: &[&str]) -> RefineResult<Output> {
        Command::new("git")
            .args(args)
            .current_dir(&self.target_root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|error| {
                RefineError::Io(format!("failed to run git {}: {error}", args.join(" ")))
            })
    }
}

pub fn with_repository_git_lock<T>(
    target_root: &std::path::Path,
    action: impl FnOnce() -> RefineResult<T>,
) -> RefineResult<T> {
    let lock = repository_git_lock(target_root)?;
    let _guard = lock
        .lock()
        .map_err(|_| RefineError::Conflict("Repository Git lock was poisoned".to_string()))?;
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

fn first_non_refine_change(status: &str) -> Option<String> {
    status.lines().find_map(|line| {
        let path = line.get(3..).unwrap_or("").trim();
        let destination = path.rsplit(" -> ").next().unwrap_or(path);
        (!destination.is_empty()
            && destination != ".refine"
            && !destination.starts_with(".refine/"))
        .then(|| destination.to_string())
    })
}

fn append_output_detail(details: &mut Vec<String>, output: &Output) {
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

fn push_rejected_by_race(output: &Output) -> bool {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    text.contains("rejected") || text.contains("non-fast-forward") || text.contains("fetch first")
}

fn command_failed(command: &str, output: &Output) -> RefineError {
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
        assert!(pulled.ok && pulled.pulled && pulled.pushed, "{pulled:?}");
        assert!(fixture.b.join(".refine/goals/GOALA/goal.json").exists());
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
        assert!(fixture.a.join(".refine/goals/GOALA/goal.json").exists());
        assert!(fixture.a.join(".refine/goals/GOALB/goal.json").exists());
    }

    #[test]
    fn sync_does_not_touch_uncommitted_target_app_changes() {
        let fixture = SyncFixture::new("dirty");
        fs::write(fixture.a.join("app.txt"), "dirty\n").unwrap();
        write_goal(&fixture.a, "GOALA");

        let result = fixture.service(&fixture.a).sync().unwrap();
        assert!(!result.attempted && !result.committed && !result.pushed);
        assert!(result.detail.unwrap().contains("app.txt"));
        assert!(fixture.a.join(".refine/goals/GOALA/goal.json").exists());
        assert_eq!(
            fs::read_to_string(fixture.a.join("app.txt")).unwrap(),
            "dirty\n"
        );
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
        let dir = root.join(".refine/goals").join(id);
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

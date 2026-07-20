use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock, TryLockError};
use std::thread;
use std::time::Duration;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::process::subprocess::{FileProcessSupervisor, ManagedProcessSpec, ProcessOwner};
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

    /// Synchronize the current branch through its configured upstream.
    ///
    /// Refine commits only durable `.refine` state, rebases onto the upstream,
    /// and pushes all local commits (including workflow merge commits). A push
    /// rejected because another node won the race is retried after another
    /// pull/rebase. Uncommitted target-app changes are never staged or stashed;
    /// the daemon defers reconciliation and retries without user involvement.
    pub fn sync(&self) -> RefineResult<GitSyncResult> {
        with_repository_git_lock(&self.target_root, || self.sync_locked())
    }

    /// Attempt a best-effort background sync without delaying foreground work.
    pub fn try_sync(&self) -> RefineResult<GitSyncResult> {
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
        self.sync_locked()
    }

    /// Fingerprint durable Refine state without invoking Git or touching the
    /// user's checkout. The daemon uses this to wake reconciliation after any
    /// shared-service mutation, independent of which surface initiated it.
    pub fn durable_state_fingerprint(&self) -> RefineResult<u64> {
        let root = self.target_root.join(".refine");
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
        if !upstream.success {
            return Ok(GitSyncResult {
                ok: true,
                branch: Some(branch),
                detail: Some("No upstream branch is configured.".to_string()),
                ..GitSyncResult::default()
            });
        }

        let status = self.git_checked(&["status", "--porcelain=v1", "-uall"])?;
        let managed_runtime = self
            .runtime_root
            .strip_prefix(&self.target_root)
            .ok()
            .map(|path| path.to_string_lossy().replace('\\', "/"));
        if let Some(path) = first_non_refine_change(
            &String::from_utf8_lossy(&status.stdout),
            managed_runtime.as_deref(),
        ) {
            return Ok(GitSyncResult {
                ok: true,
                branch: Some(branch),
                detail: Some(format!(
                    "Repository reconciliation is temporarily deferred while the target worktree is active ({path}); Refine will retry automatically."
                )),
                deferred: true,
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
            if !pull.success {
                let _ = self.git(&["rebase", "--abort"]);
                return Err(command_failed("git pull --rebase", &pull));
            }
            let after_pull = self.git_stdout(&["rev-parse", "HEAD"])?;
            pulled |= before_pull != after_pull;
            append_output_detail(&mut details, &pull);

            let push = self.git(&["push"])?;
            append_output_detail(&mut details, &push);
            if push.success {
                return Ok(GitSyncResult {
                    ok: true,
                    attempted: true,
                    committed,
                    pulled,
                    pushed: true,
                    branch: Some(branch),
                    commit,
                    detail: nonempty_detail(details),
                    deferred: false,
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
        match output.code {
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
        let mut process_args = vec!["-C".to_string(), self.target_root.display().to_string()];
        process_args.extend(args.iter().map(|arg| arg.to_string()));
        let output = FileProcessSupervisor::new(&self.runtime_root).run_to_completion(
            ManagedProcessSpec {
                owner: ProcessOwner::Maintenance,
                command: "git".to_string(),
                args: process_args,
                cwd: None,
                env: vec![("GIT_TERMINAL_PROMPT".to_string(), "0".to_string())],
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
            code: output.process.exit_code,
            stdout: output.stdout.into_bytes(),
            stderr: output.stderr.into_bytes(),
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
    code: Option<i32>,
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
        if is_runtime_only_refine_path(relative) {
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

fn is_runtime_only_refine_path(path: &std::path::Path) -> bool {
    matches!(
        path.components()
            .next()
            .and_then(|component| component.as_os_str().to_str()),
        Some("run" | "runtime" | "logs" | "support-bundles" | "provider-bin")
    ) || path == std::path::Path::new("manage-app.log")
}

fn first_non_refine_change(status: &str, managed_runtime: Option<&str>) -> Option<String> {
    status.lines().find_map(|line| {
        let path = line.get(3..).unwrap_or("").trim();
        let destination = path.rsplit(" -> ").next().unwrap_or(path);
        let managed = managed_runtime.is_some_and(|runtime| {
            destination == runtime || destination.starts_with(&format!("{runtime}/"))
        });
        (!destination.is_empty()
            && !managed
            && destination != ".refine"
            && !destination.starts_with(".refine/"))
        .then(|| destination.to_string())
    })
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

mod remote;
mod safety;
mod synchronization;

use super::*;
use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
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

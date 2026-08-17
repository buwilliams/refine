use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::git::repo::command_failed;
use crate::tools::host::git_sync::FileGitSyncService;

/// How two commits relate in history. `FastForwardToA` means `b` is an
/// ancestor of `a`, so a ref at `b` reaches `a` by fast-forward alone — and
/// symmetrically for `FastForwardToB`. Only `Diverged` ever justifies a
/// merge; ancestor-related heads never do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Ancestry {
    Equal,
    FastForwardToA,
    FastForwardToB,
    Diverged { merge_base: String },
}

/// Classify the relationship between two commits with one `merge-base` call.
///
/// Both inputs are resolved to peeled commit ids first, so refs, short ids,
/// and `HEAD` are all acceptable. Unrelated histories (no common ancestor)
/// surface as a named error rather than a variant: every repository this
/// runs against shares a root commit by construction.
pub fn classify(repo: &FileGitSyncService, a: &str, b: &str) -> RefineResult<Ancestry> {
    let a = repo.git_stdout(&["rev-parse", "--verify", &format!("{a}^{{commit}}")])?;
    let b = repo.git_stdout(&["rev-parse", "--verify", &format!("{b}^{{commit}}")])?;
    if a == b {
        return Ok(Ancestry::Equal);
    }
    let output = repo.git(&["merge-base", &a, &b])?;
    if !output.success {
        // `merge-base` fails printing nothing when the commits share no
        // ancestor; name that condition instead of surfacing a detail-free
        // command failure.
        if output.stdout.is_empty() && output.stderr.is_empty() {
            return Err(RefineError::Conflict(format!(
                "commits {a} and {b} share no common ancestor (unrelated histories)"
            )));
        }
        return Err(command_failed(&format!("git merge-base {a} {b}"), &output));
    }
    let merge_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if merge_base == b {
        return Ok(Ancestry::FastForwardToA);
    }
    if merge_base == a {
        return Ok(Ancestry::FastForwardToB);
    }
    Ok(Ancestry::Diverged { merge_base })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct RepoFixture {
        root: PathBuf,
    }

    impl RepoFixture {
        fn new(name: &str) -> Self {
            let root = unique_temp_dir(name);
            fs::create_dir_all(&root).unwrap();
            git(&root, &["init", "-q", "-b", "main"]);
            git(&root, &["config", "user.email", "ancestry@test"]);
            git(&root, &["config", "user.name", "Ancestry Test"]);
            Self { root }
        }

        fn service(&self) -> FileGitSyncService {
            FileGitSyncService::new(&self.root, self.root.join("run"))
        }

        fn commit_file(&self, path: &str, contents: &str, message: &str) -> String {
            fs::write(self.root.join(path), contents).unwrap();
            git(&self.root, &["add", path]);
            git(&self.root, &["commit", "-q", "-m", message]);
            git_stdout(&self.root, &["rev-parse", "HEAD"])
        }

        fn checkout(&self, commitish: &str) {
            git(&self.root, &["checkout", "-q", commitish]);
        }
    }

    impl Drop for RepoFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
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
            "refine-git-ancestry-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn equal_heads_classify_as_equal() {
        let fixture = RepoFixture::new("equal");
        let head = fixture.commit_file("a.txt", "base\n", "initial");
        let ancestry = classify(&fixture.service(), &head, &head).unwrap();
        assert_eq!(ancestry, Ancestry::Equal);
        // Symbolic and peeled names resolve to the same classification.
        let ancestry = classify(&fixture.service(), "HEAD", &head).unwrap();
        assert_eq!(ancestry, Ancestry::Equal);
    }

    #[test]
    fn ancestor_heads_classify_as_fast_forward_in_both_directions() {
        let fixture = RepoFixture::new("fast-forward");
        let older = fixture.commit_file("a.txt", "base\n", "initial");
        let newer = fixture.commit_file("a.txt", "advanced\n", "advance");
        let ancestry = classify(&fixture.service(), &newer, &older).unwrap();
        assert_eq!(ancestry, Ancestry::FastForwardToA);
        let ancestry = classify(&fixture.service(), &older, &newer).unwrap();
        assert_eq!(ancestry, Ancestry::FastForwardToB);
    }

    #[test]
    fn diverged_heads_carry_the_real_merge_base() {
        let fixture = RepoFixture::new("diverged");
        let base = fixture.commit_file("a.txt", "base\n", "initial");
        let ours = fixture.commit_file("ours.txt", "ours\n", "ours");
        fixture.checkout(&base);
        let theirs = fixture.commit_file("theirs.txt", "theirs\n", "theirs");
        let ancestry = classify(&fixture.service(), &ours, &theirs).unwrap();
        assert_eq!(ancestry, Ancestry::Diverged { merge_base: base });
    }

    #[test]
    fn unrelated_histories_surface_a_named_error() {
        let fixture = RepoFixture::new("unrelated");
        let rooted = fixture.commit_file("a.txt", "base\n", "initial");
        git(&fixture.root, &["checkout", "-q", "--orphan", "orphan"]);
        let orphaned = fixture.commit_file("b.txt", "other\n", "orphan root");
        let error = classify(&fixture.service(), &rooted, &orphaned).unwrap_err();
        assert!(
            error.to_string().contains("share no common ancestor"),
            "unexpected error: {error}"
        );
    }
}

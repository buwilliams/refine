//! Three-way tree merge over object ids, performed by Git itself. No file is
//! ever checked out: `git merge-tree --write-tree` reads the two commits,
//! writes the merged tree, and names the paths that did not merge cleanly for
//! the state driver, agent resolution, or a conflict report to consume.
//!
//! There is exactly one implementation and it is Git's. Refine used to carry a
//! hand-rolled per-path three-way merge as a fallback for older Git; that is
//! precisely the category of machinery this capability exists to delete, and a
//! second implementation meant two sets of semantics, a version branch through
//! the most safety-critical code in the product, and no way to know from a
//! production report which one had run. Requiring a Git that can do the job
//! ([`REQUIRED_GIT_VERSION`]) costs one preflight and removes all of it.
//!
//! ## The merge is blob-level, never textual — and that is not Git's default
//!
//! A record changed on one side takes that side; changed on both sides it is
//! CONFLICTED and falls through to the structural driver, which merges only
//! what it can prove disjoint. A line-level merge must never decide a state
//! record: two nodes editing different members of the same Goal is exactly the
//! case the ownership doctrine exists for, and splicing their lines together
//! would split a Round from the workflow authority that produced it with no
//! schema gate, no driver proof, and no agent ever seeing it.
//!
//! Left to itself `merge-tree` does content merges, so it does exactly that
//! forbidden thing: with the two sides' edits far enough apart inside one
//! record it reports a CLEAN merge of a record neither node wrote. It is
//! therefore never run bare. [`NO_TEXTUAL_MERGE_ATTRIBUTE`] is supplied as the
//! attribute source on every invocation, which drops the merge to a blob-level
//! decision — one-sided changes taken, two-sided changes conflicted with
//! `ours` left in the written tree. That attribute source is not a tuning
//! knob; it is what makes this merge correct, and it is why the required Git
//! version is the one that added `--attr-source` rather than the one that
//! added `--write-tree`.

use std::sync::OnceLock;

use crate::error::{RefineError, RefineResult};
use crate::infrastructure::git::repository::FileGitRepository;
use crate::infrastructure::git::repository::command_failed;

/// The Git this node must have to synchronize state.
///
/// Evidence: the official versioned docs show `merge-tree
/// --allow-unrelated-histories` from 2.41.0 and `--attr-source` — without
/// which the merge would content-merge state records — from 2.42.0. 2.42 is
/// therefore the real floor, and it is the floor we require: the fleet is
/// pinned to Git 2.43, so a margin above 2.42 would exclude production for
/// no capability the merge actually uses.
pub const REQUIRED_GIT_VERSION: GitVersion = GitVersion {
    major: 2,
    minor: 42,
};

/// The attribute that forbids `merge-tree` from content-merging anything.
/// `-merge` marks every path unmergeable by text, so the merge takes a
/// one-sided change and reports a two-sided one as conflicted, keeping `ours`
/// in the written tree.
const NO_TEXTUAL_MERGE_ATTRIBUTE: &str = "* -merge\n";

/// Result of merging two commits from their common history: the merged tree
/// (conflicted paths carry the `ours` blob and are only meaningful once
/// replaced) and the paths that did not merge cleanly.
#[derive(Debug)]
pub struct TreeMerge {
    pub tree: String,
    pub conflicted_paths: Vec<String>,
}

/// A Git version, compared on `(major, minor)` only: Refine's requirements are
/// feature requirements, and features arrive in minor releases.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GitVersion {
    pub major: u32,
    pub minor: u32,
}

impl std::fmt::Display for GitVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

impl GitVersion {
    /// Read `major.minor` out of whatever `git --version` prints.
    ///
    /// Distributions decorate it freely — `git version 2.39.5 (Apple
    /// Git-154)`, `git version 2.42.0.windows.1`, release candidates like
    /// `2.43.0-rc1` — so this takes the first whitespace-separated field that
    /// starts with a digit and reads the leading integer of its first two
    /// dot-separated parts. Anything it cannot read is `None`, which the
    /// caller treats as unsupported rather than guessing.
    pub fn parse(output: &str) -> Option<Self> {
        let field = output
            .split_whitespace()
            .find(|field| field.starts_with(|character: char| character.is_ascii_digit()))?;
        let mut parts = field.split('.');
        let major = leading_number(parts.next()?)?;
        let minor = leading_number(parts.next().unwrap_or("0")).unwrap_or(0);
        Some(Self { major, minor })
    }
}

/// The leading run of digits of a version part, so `0-rc1` reads as 0 and
/// `windows` reads as nothing.
fn leading_number(part: &str) -> Option<u32> {
    let digits = part
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}

/// The host Git's version, probed once per process. Which `git` is on PATH is
/// a fact about the node, not about any repository, and a process cannot
/// change it — so this is cached and no merge pays for a version check.
pub fn host_git_version() -> Option<GitVersion> {
    static VERSION: OnceLock<Option<GitVersion>> = OnceLock::new();
    *VERSION.get_or_init(|| {
        let output = std::process::Command::new("git")
            .arg("--version")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        GitVersion::parse(&String::from_utf8_lossy(&output.stdout))
    })
}

/// Fail unless the host Git can run the state merge.
///
/// This is the ONE place the requirement is decided. It runs at daemon start
/// and at node bootstrap, so an operator meets the requirement as a clear
/// sentence rather than as a downstream `git merge-tree: unknown option`, and
/// again at the head of every synchronization pass, so a node whose Git is
/// downgraded underneath a running daemon fails the same typed way.
pub fn ensure_supported_git() -> RefineResult<()> {
    match host_git_version() {
        Some(version) if version >= REQUIRED_GIT_VERSION => Ok(()),
        Some(version) => Err(RefineError::UnsupportedGitVersion {
            required: REQUIRED_GIT_VERSION.to_string(),
            observed: version.to_string(),
        }),
        None => Err(RefineError::UnsupportedGitVersion {
            required: REQUIRED_GIT_VERSION.to_string(),
            observed: "no readable `git --version`".to_string(),
        }),
    }
}

/// Merge `ours` and `theirs`.
///
/// Git finds the merge base itself, including the recursive merge of several
/// bases in a criss-cross history, and `--allow-unrelated-histories` covers
/// the join of two nodes that bootstrapped the same branch independently — so
/// there is one call and no special case.
pub fn merge_commits(
    repo: &FileGitRepository,
    root: &std::path::Path,
    ours: &str,
    theirs: &str,
) -> RefineResult<TreeMerge> {
    ensure_supported_git()?;
    let attributes = no_textual_merge_attr_tree(repo, root)?;
    let output = repo.git_at(
        root,
        &[
            &format!("--attr-source={attributes}"),
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--allow-unrelated-histories",
            ours,
            theirs,
        ],
    )?;
    parse_merge_tree_output(output.success, &String::from_utf8_lossy(&output.stdout)).ok_or_else(
        || {
            command_failed(
                &format!("git merge-tree --write-tree {ours} {theirs}"),
                &output,
            )
        },
    )
}

/// The repository's empty tree object: the base the structural driver reads
/// against when two histories share no root, so an added-on-both-sides record
/// is still compared member by member.
pub fn empty_tree_id(repo: &FileGitRepository, root: &std::path::Path) -> RefineResult<String> {
    repo.git_at_stdout(root, &["hash-object", "-t", "tree", "/dev/null"])
}

/// A tree holding only `.gitattributes` with [`NO_TEXTUAL_MERGE_ATTRIBUTE`],
/// for `--attr-source`. Content-addressed, so writing it is idempotent and
/// every call in every process converges on the same id; it is referenced by
/// id and never becomes reachable from any branch, so it costs one loose
/// object that `git gc` reclaims.
fn no_textual_merge_attr_tree(
    repo: &FileGitRepository,
    root: &std::path::Path,
) -> RefineResult<String> {
    let blob = write_blob(repo, root, NO_TEXTUAL_MERGE_ATTRIBUTE.as_bytes())?;
    // A tree object body is `<mode> <name>\0<binary oid>` per entry. It is
    // assembled here and stored with `hash-object -t tree` rather than piped
    // to `mktree` because the repository plumbing runs Git without stdin; the
    // two produce the same object.
    let mut object = format!("{REGULAR_FILE_MODE} .gitattributes\0").into_bytes();
    let oid = blob.trim();
    if oid.is_empty() || oid.len() % 2 != 0 || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RefineError::Io(format!(
            "git hash-object returned an unreadable object id: {oid}"
        )));
    }
    object.extend(
        (0..oid.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&oid[index..index + 2], 16))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| RefineError::Io(format!("git hash-object returned {oid}: {error}")))?,
    );
    hash_object(repo, root, "tree", &object)
}

/// Parse `git merge-tree --write-tree --name-only` output: the merged tree id
/// on the first line, then — for a conflicted merge (non-zero exit) — the
/// conflicted names until a blank line separates the informational messages.
/// `None` means the output is not a merge result but a real command failure.
fn parse_merge_tree_output(success: bool, stdout: &str) -> Option<TreeMerge> {
    let mut lines = stdout.lines();
    let tree = lines.next().unwrap_or_default().trim().to_string();
    if tree.len() < 40 || !tree.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    if success {
        return Some(TreeMerge {
            tree,
            conflicted_paths: Vec::new(),
        });
    }
    let mut conflicted_paths = lines
        .take_while(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    // Sorted, because the order is load-bearing: the stable conflict report id
    // hashes these paths in sequence, so the same contention must always
    // produce the same id no matter what order Git happened to name them in.
    conflicted_paths.sort();
    Some(TreeMerge {
        tree,
        conflicted_paths,
    })
}

/// The mode a file Refine itself writes takes: state records and resolved
/// files are ordinary non-executable files.
pub const REGULAR_FILE_MODE: &str = "100644";

/// One path-level edit applied on top of a start tree.
#[derive(Debug)]
pub enum TreeOperation {
    Set {
        path: String,
        blob: String,
        mode: String,
    },
    Remove {
        path: String,
    },
}

impl TreeOperation {
    /// Set a path Refine produced the bytes for: a regular file.
    pub fn set(path: impl Into<String>, blob: impl Into<String>) -> Self {
        Self::Set {
            path: path.into(),
            blob: blob.into(),
            mode: REGULAR_FILE_MODE.to_string(),
        }
    }
}

/// Build a tree from `start_tree` plus path-level edits, using the given
/// worktree's index as scratch space and restoring it to HEAD afterwards. An
/// interruption leaves only a staged diff that the worktree recovery step
/// already clears.
pub fn build_tree(
    repo: &FileGitRepository,
    root: &std::path::Path,
    start_tree: &str,
    operations: &[TreeOperation],
) -> RefineResult<String> {
    let result = (|| {
        repo.git_at_checked(root, &["read-tree", start_tree])?;
        for operation in operations {
            match operation {
                TreeOperation::Set { path, blob, mode } => {
                    repo.git_at_checked(
                        root,
                        &[
                            "update-index",
                            "--add",
                            "--cacheinfo",
                            &format!("{mode},{blob},{path}"),
                        ],
                    )?;
                }
                TreeOperation::Remove { path } => {
                    repo.git_at_checked(root, &["update-index", "--force-remove", "--", path])?;
                }
            }
        }
        repo.git_at_stdout(root, &["write-tree"])
    })();
    let restore = repo.git_at_checked(root, &["read-tree", "HEAD"]);
    let tree = result?;
    restore?;
    Ok(tree)
}

/// Store bytes as a blob object and return its id.
pub fn write_blob(
    repo: &FileGitRepository,
    root: &std::path::Path,
    bytes: &[u8],
) -> RefineResult<String> {
    hash_object(repo, root, "blob", bytes)
}

/// Store bytes as a Git object of the given type and return its id. The bytes
/// are staged outside the repository so no worktree path is ever touched.
fn hash_object(
    repo: &FileGitRepository,
    root: &std::path::Path,
    kind: &str,
    bytes: &[u8],
) -> RefineResult<String> {
    let staging = std::env::temp_dir().join(format!(
        "refine-merge-{kind}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&staging, bytes).map_err(|error| {
        RefineError::Io(format!(
            "failed to stage merge {kind} {}: {error}",
            staging.display()
        ))
    })?;
    let staged = staging.display().to_string();
    let result = repo.git_at_stdout(root, &["hash-object", "-t", kind, "-w", "--", &staged]);
    let _ = std::fs::remove_file(&staging);
    result
}

/// Create a commit object for a tree with explicit parents; no ref moves.
pub fn commit_tree(
    repo: &FileGitRepository,
    root: &std::path::Path,
    tree: &str,
    parents: &[&str],
    message: &str,
) -> RefineResult<String> {
    let mut args = vec!["commit-tree".to_string(), tree.to_string()];
    for parent in parents {
        args.push("-p".to_string());
        args.push((*parent).to_string());
    }
    args.push("-m".to_string());
    args.push(message.to_string());
    let arguments = args.iter().map(String::as_str).collect::<Vec<_>>();
    repo.git_at_stdout(root, &arguments)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    /// A tiny repository the merge runs against directly, so every shape below
    /// is one real `merge-tree` call rather than an assertion about the sync
    /// pipeline that wraps it.
    struct MergeFixture {
        root: PathBuf,
    }

    impl MergeFixture {
        fn new(name: &str) -> Self {
            let root = unique_temp_dir(name);
            fs::create_dir_all(&root).unwrap();
            git(&root, &["init", "-q", "-b", "main"]);
            git(&root, &["config", "user.email", "merge@test"]);
            git(&root, &["config", "user.name", "Merge Test"]);
            Self { root }
        }

        fn repo(&self) -> FileGitRepository {
            FileGitRepository::new(&self.root, self.root.join("run"))
        }

        /// Commit the given file set as a whole tree state and return the
        /// commit id. `--allow-empty` so a side that deliberately changes
        /// nothing is still a commit.
        fn commit(&self, files: &[(&str, &str)]) -> String {
            for entry in fs::read_dir(&self.root).unwrap() {
                let path = entry.unwrap().path();
                if path.file_name().map(|name| name == ".git").unwrap_or(false)
                    || path.file_name().map(|name| name == "run").unwrap_or(false)
                {
                    continue;
                }
                if path.is_dir() {
                    fs::remove_dir_all(&path).unwrap();
                } else {
                    fs::remove_file(&path).unwrap();
                }
            }
            for (path, contents) in files {
                let full = self.root.join(path);
                fs::create_dir_all(full.parent().unwrap()).unwrap();
                fs::write(&full, contents).unwrap();
            }
            git(&self.root, &["add", "-A"]);
            git(
                &self.root,
                &["commit", "-q", "--allow-empty", "-m", "state"],
            );
            git(&self.root, &["rev-parse", "HEAD"])
        }

        fn checkout(&self, commit: &str) {
            git(&self.root, &["checkout", "-q", commit]);
        }

        fn merged_files(&self, tree: &str) -> BTreeMap<String, String> {
            let listing = git(&self.root, &["ls-tree", "-r", tree]);
            let mut files = BTreeMap::new();
            for line in listing.lines() {
                let (meta, path) = line.split_once('\t').unwrap();
                let oid = meta.split_whitespace().nth(2).unwrap();
                files.insert(
                    path.to_string(),
                    git(&self.root, &["cat-file", "-p", oid]).trim().to_string(),
                );
            }
            files
        }

        fn mode(&self, tree: &str, path: &str) -> String {
            let listing = git(&self.root, &["ls-tree", "-r", tree]);
            listing
                .lines()
                .find(|line| line.ends_with(&format!("\t{path}")))
                .map(|line| line.split_whitespace().next().unwrap().to_string())
                .unwrap_or_default()
        }

        /// Merge two sides built from the same base and return the result.
        fn merge(
            &self,
            base: &[(&str, &str)],
            ours: &[(&str, &str)],
            theirs: &[(&str, &str)],
        ) -> (TreeMerge, BTreeMap<String, String>) {
            let base_commit = self.commit(base);
            let our_commit = self.commit(ours);
            self.checkout(&base_commit);
            let their_commit = self.commit(theirs);
            self.checkout(&our_commit);
            let merged =
                merge_commits(&self.repo(), &self.root, &our_commit, &their_commit).unwrap();
            let files = self.merged_files(&merged.tree);
            (merged, files)
        }
    }

    impl Drop for MergeFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap_or_else(|error| panic!("git {args:?} failed to run: {error}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
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
            "refine-merge-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    const RECORD: &str = ".refine/goals/GO/ALA/goal.json";
    const OTHER: &str = ".refine/goals/GO/BEE/goal.json";

    /// A record whose two edited members are `gap` unchanged lines apart.
    fn spaced_record(status: &str, title: &str, gap: usize) -> String {
        let padding = (0..gap)
            .map(|index| format!("  \"pad_{index:02}\": \"xxxxxxxx\",\n"))
            .collect::<String>();
        format!("{{\n  \"status\": \"{status}\",\n{padding}  \"title\": \"{title}\"\n}}\n")
    }

    #[test]
    fn nothing_changed_on_either_side_merges_clean() {
        let fixture = MergeFixture::new("unchanged");
        let (merged, files) = fixture.merge(
            &[(RECORD, "base\n")],
            &[(RECORD, "base\n")],
            &[(RECORD, "base\n")],
        );
        assert!(merged.conflicted_paths.is_empty(), "{merged:?}");
        assert_eq!(files[RECORD], "base");
    }

    #[test]
    fn one_sided_changes_take_that_side_and_never_conflict() {
        let fixture = MergeFixture::new("one-sided");
        let (merged, files) = fixture.merge(
            &[(RECORD, "base\n"), (OTHER, "base\n")],
            &[(RECORD, "ours\n"), (OTHER, "base\n")],
            &[(RECORD, "base\n"), (OTHER, "theirs\n")],
        );
        assert!(merged.conflicted_paths.is_empty(), "{merged:?}");
        assert_eq!(files[RECORD], "ours");
        assert_eq!(files[OTHER], "theirs");
    }

    #[test]
    fn both_sides_making_the_same_change_is_not_a_conflict() {
        let fixture = MergeFixture::new("agreed");
        let (merged, files) = fixture.merge(
            &[(RECORD, "base\n")],
            &[(RECORD, "agreed\n")],
            &[(RECORD, "agreed\n")],
        );
        assert!(merged.conflicted_paths.is_empty(), "{merged:?}");
        assert_eq!(files[RECORD], "agreed");
    }

    /// THE property the whole merge is built to hold. Two nodes edit different
    /// members of one Goal, far enough apart that a textual three-way merge
    /// splices them together and calls it clean. It has to be reported
    /// contested so the structural driver — and then the ownership doctrine —
    /// decides it, because a spliced record is one neither node wrote.
    ///
    /// This is what `--attr-source` buys; without it this exact case returns a
    /// clean merge and this test fails.
    #[test]
    fn two_members_of_one_record_changed_far_apart_are_still_contested() {
        let fixture = MergeFixture::new("far-apart");
        let (merged, files) = fixture.merge(
            &[(RECORD, &spaced_record("todo", "base", 40))],
            &[(RECORD, &spaced_record("review", "base", 40))],
            &[(RECORD, &spaced_record("todo", "remote", 40))],
        );
        assert_eq!(merged.conflicted_paths, vec![RECORD.to_string()]);
        // A contested path keeps OURS in the written tree — never conflict
        // markers, and never a spliced record. The driver replaces it.
        assert_eq!(
            files[RECORD].trim(),
            spaced_record("review", "base", 40).trim()
        );
    }

    #[test]
    fn two_members_of_one_record_changed_adjacently_are_contested() {
        let fixture = MergeFixture::new("adjacent");
        let (merged, _) = fixture.merge(
            &[(RECORD, &spaced_record("todo", "base", 0))],
            &[(RECORD, &spaced_record("review", "base", 0))],
            &[(RECORD, &spaced_record("todo", "remote", 0))],
        );
        assert_eq!(merged.conflicted_paths, vec![RECORD.to_string()]);
    }

    #[test]
    fn add_add_conflicts_only_when_the_content_differs() {
        let differing = MergeFixture::new("add-add-differs");
        let (merged, _) = differing.merge(
            &[(OTHER, "base\n")],
            &[(OTHER, "base\n"), (RECORD, "ours\n")],
            &[(OTHER, "base\n"), (RECORD, "theirs\n")],
        );
        assert_eq!(merged.conflicted_paths, vec![RECORD.to_string()]);

        let agreeing = MergeFixture::new("add-add-agrees");
        let (merged, files) = agreeing.merge(
            &[(OTHER, "base\n")],
            &[(OTHER, "base\n"), (RECORD, "same\n")],
            &[(OTHER, "base\n"), (RECORD, "same\n")],
        );
        assert!(merged.conflicted_paths.is_empty(), "{merged:?}");
        assert_eq!(files[RECORD], "same");
    }

    #[test]
    fn a_delete_against_a_modification_is_contested_in_both_directions() {
        let delete_modify = MergeFixture::new("delete-modify");
        let (merged, _) = delete_modify.merge(
            &[(RECORD, "base\n"), (OTHER, "base\n")],
            &[(OTHER, "base\n")],
            &[(RECORD, "theirs\n"), (OTHER, "base\n")],
        );
        assert_eq!(merged.conflicted_paths, vec![RECORD.to_string()]);

        let modify_delete = MergeFixture::new("modify-delete");
        let (merged, _) = modify_delete.merge(
            &[(RECORD, "base\n"), (OTHER, "base\n")],
            &[(RECORD, "ours\n"), (OTHER, "base\n")],
            &[(OTHER, "base\n")],
        );
        assert_eq!(merged.conflicted_paths, vec![RECORD.to_string()]);
    }

    #[test]
    fn a_delete_on_one_side_removes_the_path_and_keeps_modes_otherwise() {
        let fixture = MergeFixture::new("delete-and-mode");
        let base = fixture.commit(&[("gone.txt", "base\n"), ("tool.sh", "base\n")]);
        let ours = base.clone();
        fixture.checkout(&base);
        let theirs = {
            use std::os::unix::fs::PermissionsExt;
            fs::remove_file(fixture.root.join("gone.txt")).unwrap();
            let tool = fixture.root.join("tool.sh");
            fs::write(&tool, "theirs\n").unwrap();
            fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
            git(&fixture.root, &["add", "-A"]);
            git(&fixture.root, &["commit", "-q", "-m", "theirs"]);
            git(&fixture.root, &["rev-parse", "HEAD"])
        };
        fixture.checkout(&ours);

        let merged = merge_commits(&fixture.repo(), &fixture.root, &ours, &theirs).unwrap();

        assert!(merged.conflicted_paths.is_empty(), "{merged:?}");
        let files = fixture.merged_files(&merged.tree);
        assert!(!files.contains_key("gone.txt"), "{files:?}");
        assert_eq!(files["tool.sh"], "theirs");
        assert_eq!(fixture.mode(&merged.tree, "tool.sh"), "100755");
    }

    #[test]
    fn both_sides_deleting_the_same_path_is_not_a_conflict() {
        let fixture = MergeFixture::new("delete-delete");
        let (merged, files) = fixture.merge(
            &[(RECORD, "base\n"), (OTHER, "base\n")],
            &[(OTHER, "base\n")],
            &[(OTHER, "base\n")],
        );
        assert!(merged.conflicted_paths.is_empty(), "{merged:?}");
        assert!(!files.contains_key(RECORD), "{files:?}");
    }

    #[test]
    fn large_and_binary_content_changed_on_both_sides_is_contested_not_spliced() {
        let big = |seed: u8| {
            (0..4096u32)
                .map(|index| format!("{}\n", (index as u8).wrapping_add(seed)))
                .collect::<String>()
        };
        let large = MergeFixture::new("large-blobs");
        let (merged, _) = large.merge(
            &[(RECORD, &big(0))],
            &[(RECORD, &big(1))],
            &[(RECORD, &big(2))],
        );
        assert_eq!(merged.conflicted_paths, vec![RECORD.to_string()]);

        let binaryish = |seed: char| format!("\u{0}\u{1}binary-{seed}\u{0}payload\u{2}\n");
        let binary = MergeFixture::new("binary-blobs");
        let (merged, _) = binary.merge(
            &[(RECORD, &binaryish('a'))],
            &[(RECORD, &binaryish('b'))],
            &[(RECORD, &binaryish('c'))],
        );
        assert_eq!(merged.conflicted_paths, vec![RECORD.to_string()]);
    }

    /// The stable conflict report id hashes the contested paths in order, so
    /// the merge must always name them in the same one.
    #[test]
    fn contested_paths_are_reported_in_sorted_order() {
        let fixture = MergeFixture::new("sorted-conflicts");
        let zebra = ".refine/goals/GO/ZED/goal.json";
        let alpha = ".refine/goals/GO/AAA/goal.json";
        let middle = ".refine/goals/GO/MID/goal.json";
        let (merged, _) = fixture.merge(
            &[(zebra, "base\n"), (alpha, "base\n"), (middle, "base\n")],
            &[(zebra, "ours\n"), (alpha, "ours\n"), (middle, "ours\n")],
            &[
                (zebra, "theirs\n"),
                (alpha, "theirs\n"),
                (middle, "theirs\n"),
            ],
        );
        assert_eq!(
            merged.conflicted_paths,
            vec![alpha.to_string(), middle.to_string(), zebra.to_string()]
        );
    }

    #[test]
    fn nested_records_changed_on_one_side_each_all_merge() {
        let fixture = MergeFixture::new("nested");
        let nodes = ".refine/nodes.json";
        let round = ".refine/goals/GO/ALA/rounds/0/round.json";
        let (merged, files) = fixture.merge(
            &[(nodes, "base\n"), (round, "base\n")],
            &[(nodes, "ours\n"), (round, "base\n")],
            &[(nodes, "base\n"), (round, "theirs\n")],
        );
        assert!(merged.conflicted_paths.is_empty(), "{merged:?}");
        assert_eq!(files[nodes], "ours");
        assert_eq!(files[round], "theirs");
    }

    /// Two nodes bootstrapping the same branch independently share no ancestor.
    /// `--allow-unrelated-histories` joins them in the same one call, with no
    /// empty-tree base and no second code path.
    #[test]
    fn unrelated_histories_join_with_only_the_shared_paths_contested() {
        let fixture = MergeFixture::new("unrelated");
        let ours = fixture.commit(&[("ours.txt", "ours\n"), ("shared.txt", "ours\n")]);
        git(&fixture.root, &["checkout", "-q", "--orphan", "other"]);
        let theirs = fixture.commit(&[("theirs.txt", "theirs\n"), ("shared.txt", "theirs\n")]);
        fixture.checkout(&ours);

        let merged = merge_commits(&fixture.repo(), &fixture.root, &ours, &theirs).unwrap();

        assert_eq!(merged.conflicted_paths, vec!["shared.txt".to_string()]);
        let files = fixture.merged_files(&merged.tree);
        assert_eq!(files["ours.txt"], "ours");
        assert_eq!(files["theirs.txt"], "theirs");
    }

    #[test]
    fn the_host_git_satisfies_the_requirement_the_merge_is_built_on() {
        ensure_supported_git()
            .expect("the test host must run a Git this build supports; see REQUIRED_GIT_VERSION");
    }

    #[test]
    fn a_git_version_is_read_from_whatever_the_distribution_decorates_it_with() {
        let cases = [
            ("git version 2.45.0", Some((2, 45))),
            ("git version 2.51.0\n", Some((2, 51))),
            ("git version 2.39.5 (Apple Git-154)", Some((2, 39))),
            ("git version 2.42.0.windows.1", Some((2, 42))),
            ("git version 2.45.0-rc1", Some((2, 45))),
            ("git version 2.45.0.rc2", Some((2, 45))),
            ("git version 3.0.0", Some((3, 0))),
            ("git version 2", Some((2, 0))),
            ("git version 2.", Some((2, 0))),
            // Nothing readable: the caller must treat these as unsupported
            // rather than assume a version.
            ("git version", None),
            ("", None),
            ("not a version at all", None),
            ("git version v2.45.0", None),
        ];
        for (output, expected) in cases {
            let parsed = GitVersion::parse(output).map(|version| (version.major, version.minor));
            assert_eq!(parsed, expected, "parsing {output:?}");
        }
    }

    #[test]
    fn versions_compare_on_major_then_minor() {
        let parse = |text: &str| GitVersion::parse(text).unwrap();
        // 2.42 is the floor: the version that added `--attr-source`, without
        // which the merge silently content-merges state records. 2.43 is what
        // the fleet is pinned to, so it must pass.
        assert!(parse("2.42.0") >= REQUIRED_GIT_VERSION);
        assert!(parse("2.43.0") >= REQUIRED_GIT_VERSION);
        assert!(parse("2.45.0") >= REQUIRED_GIT_VERSION);
        assert!(parse("2.51.1") >= REQUIRED_GIT_VERSION);
        assert!(parse("3.0.0") >= REQUIRED_GIT_VERSION);
        assert!(parse("2.41.9") < REQUIRED_GIT_VERSION);
        assert!(parse("2.34.1") < REQUIRED_GIT_VERSION);
        assert!(parse("1.9.9") < REQUIRED_GIT_VERSION);
    }

    /// The operator-facing sentence. It has to name both versions and say what
    /// to do, because this is the error a half-finished rollout produces.
    #[test]
    fn the_unsupported_git_error_names_both_versions_and_the_fix() {
        let error = RefineError::UnsupportedGitVersion {
            required: REQUIRED_GIT_VERSION.to_string(),
            observed: "2.34".to_string(),
        };
        let message = error.to_string();
        assert!(message.contains("2.42"), "{message}");
        assert!(message.contains("2.34"), "{message}");
        assert!(message.contains("Upgrade Git on this node"), "{message}");
        // The rest of the fleet is explicitly unaffected: this is one node's
        // condition, the same shape as a node still awaiting its upgrade.
        assert!(
            message.contains("every other node keeps syncing"),
            "{message}"
        );
        assert_eq!(error.category(), crate::error::ErrorCategory::Degraded);
    }
}

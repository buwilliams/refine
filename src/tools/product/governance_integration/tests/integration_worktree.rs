use super::*;
use crate::tools::product::governance_integration::integration_worktree::ensure_integration_worktree;
use crate::tools::product::worktree_cleanup::{FileWorktreeCleanupService, WorktreeCleanupOptions};

struct IntegrationWorktreeFixture {
    temp_root: PathBuf,
    repo: PathBuf,
    runtime_root: PathBuf,
    base_commit: String,
    delta_commit: String,
}

impl IntegrationWorktreeFixture {
    fn new(name: &str) -> Self {
        let temp_root = unique_temp_dir(name);
        let repo = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "base");
        let base_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
        commit_file(&repo, "delta.txt", "delta\n", "delta");
        let delta_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
        Self {
            temp_root,
            repo,
            runtime_root,
            base_commit,
            delta_commit,
        }
    }

    fn repo_git(&self) -> FileGitWorktreeService {
        FileGitWorktreeService::with_runtime_root(&self.repo, &self.runtime_root)
    }
}

#[test]
fn ensure_integration_worktree_creates_reuses_and_recreates() {
    let fixture = IntegrationWorktreeFixture::new("integration-worktree");
    let repo_git = fixture.repo_git();

    let created =
        ensure_integration_worktree(&repo_git, &fixture.runtime_root, &fixture.base_commit)
            .unwrap();
    assert_eq!(
        created.path,
        fixture.repo.join(".git/refine-integration/target")
    );
    assert_eq!(
        git_stdout(&created.path, &["rev-parse", "HEAD"]),
        fixture.base_commit
    );
    assert_eq!(
        git_stdout(&created.path, &["branch", "--show-current"]),
        "",
        "integration worktree must stay detached"
    );
    assert_eq!(
        created.git().symbolic_head_branch().unwrap(),
        None,
        "integration worktree service must observe detachment"
    );

    // A marker in the worktree's private admin directory survives reuse but
    // not a remove-and-recreate cycle.
    let admin = PathBuf::from(git_stdout(
        &created.path,
        &["rev-parse", "--absolute-git-dir"],
    ));
    let marker = admin.join("refine-test-reuse-marker");
    fs::write(&marker, "reused\n").unwrap();

    let reused =
        ensure_integration_worktree(&repo_git, &fixture.runtime_root, &fixture.delta_commit)
            .unwrap();
    assert_eq!(reused.path, created.path);
    assert_eq!(
        git_stdout(&reused.path, &["rev-parse", "HEAD"]),
        fixture.delta_commit
    );
    assert!(marker.exists(), "pristine worktree must be reused");

    fs::write(reused.path.join("residue.txt"), "residue\n").unwrap();
    let recreated =
        ensure_integration_worktree(&repo_git, &fixture.runtime_root, &fixture.base_commit)
            .unwrap();
    assert_eq!(recreated.path, created.path);
    assert!(!recreated.path.join("residue.txt").exists());
    assert_eq!(
        git_stdout(&recreated.path, &["rev-parse", "HEAD"]),
        fixture.base_commit
    );
    assert!(!marker.exists(), "residue must force recreation");
    assert!(
        !git_succeeds(
            &fixture.repo,
            &["worktree", "remove", recreated.path.to_str().unwrap()]
        ),
        "recreated worktree must be locked again"
    );

    fs::remove_dir_all(fixture.temp_root).unwrap();
}

#[test]
fn cleanup_sweep_does_not_touch_the_integration_worktree() {
    let fixture = IntegrationWorktreeFixture::new("integration-worktree-sweep");
    prepare_refine_dir(&fixture.repo).unwrap();
    let worktree = ensure_integration_worktree(
        &fixture.repo_git(),
        &fixture.runtime_root,
        &fixture.base_commit,
    )
    .unwrap();

    let report = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root)
        .run(WorktreeCleanupOptions {
            apply: true,
            older_than_seconds: 0,
        })
        .unwrap();

    assert_eq!(report.inspected, 0, "sweep must not even inspect the tree");
    assert_eq!(report.removed, 0);
    assert!(worktree.path.join("app.txt").exists());
    assert!(
        git_stdout(&fixture.repo, &["worktree", "list", "--porcelain"])
            .contains("refine-integration"),
        "registration must survive the sweep"
    );

    fs::remove_dir_all(fixture.temp_root).unwrap();
}

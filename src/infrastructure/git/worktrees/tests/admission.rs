use super::*;

struct Fixture {
    temp: PathBuf,
    repo: PathBuf,
    git: FileGitWorktreeService,
    workspace: ManagedWorktree,
}
impl Fixture {
    fn new() -> Self {
        let temp = unique_temp_dir("workspace-admission");
        let repo = temp.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "app.txt", "base\n", "base");
        let git = FileGitWorktreeService::new(&repo);
        let branch = "refine/GOAL/round-1".to_string();
        let path = git.managed_worktree_path(&branch).unwrap();
        let workspace = ManagedWorktree {
            repository: repo.clone(),
            path,
            branch,
            commit: None,
            allow_rebase: false,
            registration: None,
        };
        Self {
            temp,
            repo,
            git,
            workspace,
        }
    }
    fn create(&self) {
        self.git
            .ensure_worktree(&self.workspace.branch, &self.workspace.path)
            .unwrap();
    }
    fn primary_snapshot(&self) -> (Vec<u8>, Vec<u8>, String) {
        (
            fs::read(self.repo.join("app.txt")).unwrap(),
            fs::read(self.repo.join(".git/index")).unwrap(),
            git_stdout(&self.repo, &["rev-parse", "HEAD"]),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp);
    }
}

#[test]
fn matching_branch_in_primary_is_rejected_without_touching_files_or_index() {
    let f = Fixture::new();
    git(&f.repo, &["switch", "-c", &f.workspace.branch]).unwrap();
    fs::write(f.repo.join("app.txt"), "manual staged\n").unwrap();
    git(&f.repo, &["add", "app.txt"]).unwrap();
    fs::write(f.repo.join("app.txt"), "manual unstaged\n").unwrap();
    let before = f.primary_snapshot();
    let head = f.git.resolve_commit("HEAD").unwrap();
    for result in [
        f.git
            .ensure_worktree(&f.workspace.branch, &f.workspace.path),
        f.git
            .ensure_worktree_from_base(&f.workspace.branch, &f.workspace.path, &head),
        f.git
            .ensure_worktree_at_commit(&f.workspace.branch, &f.workspace.path, &head),
    ] {
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("existing work was preserved")
        );
    }
    assert_eq!(before, f.primary_snapshot());
    assert!(f.git.ensure_worktree(&f.workspace.branch, &f.repo).is_err());
}

#[test]
fn matching_branch_in_unrelated_linked_checkout_is_not_adopted_or_locked() {
    let f = Fixture::new();
    let elsewhere = f.temp.join("manual");
    git(
        &f.repo,
        &[
            "worktree",
            "add",
            "-b",
            &f.workspace.branch,
            elsewhere.to_str().unwrap(),
        ],
    )
    .unwrap();
    fs::write(elsewhere.join("app.txt"), "manual\n").unwrap();
    let before = git_stdout(&f.repo, &["worktree", "list", "--porcelain"]);
    assert!(
        f.git
            .ensure_worktree(&f.workspace.branch, &f.workspace.path)
            .is_err()
    );
    assert_eq!(
        before,
        git_stdout(&f.repo, &["worktree", "list", "--porcelain"])
    );
    assert_eq!(
        fs::read_to_string(elsewhere.join("app.txt")).unwrap(),
        "manual\n"
    );
}

#[test]
fn managed_workspace_revalidates_branch_before_staging() {
    let f = Fixture::new();
    f.create();
    let service = FileGitWorktreeService::new(&f.workspace.path)
        .with_managed_worktree(f.workspace.clone())
        .unwrap();
    let index = service.git_path("index").unwrap();
    git(&f.workspace.path, &["switch", "-c", "manual"]).unwrap();
    fs::write(f.workspace.path.join("app.txt"), "manual\n").unwrap();
    let before = fs::read(&index).unwrap();
    assert!(service.commit("must not commit", &[]).is_err());
    assert_eq!(before, fs::read(index).unwrap());
    assert_eq!(current_branch(&f.workspace.path), "manual");
}

#[test]
fn managed_workspace_checks_exact_commit_repository_and_registration() {
    let f = Fixture::new();
    f.create();
    let mut exact = f.workspace.clone();
    exact.commit = Some(f.git.resolve_commit("HEAD").unwrap());
    exact.validate().unwrap();
    commit_file(&f.workspace.path, "app.txt", "changed\n", "agent commit");
    assert!(exact.validate().is_err());
    f.workspace.validate().unwrap();
    let other = Fixture::new();
    let mut wrong_repo = f.workspace.clone();
    wrong_repo.repository = other.repo.clone();
    assert!(wrong_repo.validate().is_err());
    let git_dir = FileGitWorktreeService::new(&f.workspace.path)
        .git_path("gitdir")
        .unwrap();
    fs::write(
        git_dir,
        other.repo.join(".git").to_string_lossy().as_bytes(),
    )
    .unwrap();
    assert!(f.workspace.validate().is_err());
}

#[test]
fn missing_registered_managed_checkout_is_recreated_at_the_retained_commit() {
    let f = Fixture::new();
    f.create();
    commit_file(&f.workspace.path, "app.txt", "candidate\n", "candidate");
    let candidate = f.git.resolve_commit(&f.workspace.branch).unwrap();
    fs::remove_dir_all(&f.workspace.path).unwrap();
    f.git
        .ensure_worktree_at_commit(&f.workspace.branch, &f.workspace.path, &candidate)
        .unwrap();
    f.workspace.validate().unwrap();
    assert_eq!(
        fs::read_to_string(f.workspace.path.join("app.txt")).unwrap(),
        "candidate\n"
    );
}

#[test]
fn unregistered_directory_is_preserved() {
    let f = Fixture::new();
    fs::create_dir_all(&f.workspace.path).unwrap();
    fs::write(f.workspace.path.join("notes"), "retained").unwrap();
    assert!(
        f.git
            .ensure_worktree(&f.workspace.branch, &f.workspace.path)
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(f.workspace.path.join("notes")).unwrap(),
        "retained"
    );
}

#[cfg(unix)]
#[test]
fn symlinked_workspace_and_subpath_escapes_are_rejected() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.create();
    let root = &f.workspace.path;
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("file"), "content").unwrap();
    symlink(&f.repo, root.join("escape")).unwrap();
    symlink(root.join("src"), root.join("inside")).unwrap();
    assert_eq!(
        agent_worktree_cwd(root.to_str().unwrap(), "inside").unwrap(),
        root.join("src")
    );
    for subpath in ["escape", "missing", "file", "../..", "/tmp"] {
        assert!(
            agent_worktree_cwd(root.to_str().unwrap(), subpath).is_err(),
            "{subpath}"
        );
    }
    let alias = f.temp.join("alias");
    symlink(root, &alias).unwrap();
    let mut aliased = f.workspace.clone();
    aliased.path = alias;
    aliased.validate().unwrap();
    fs::remove_dir_all(root).unwrap();
    symlink(&f.repo, root).unwrap();
    assert!(f.workspace.validate().is_err());
}

#[test]
fn nested_repository_is_not_an_agent_cwd() {
    let f = Fixture::new();
    f.create();
    let nested = f.workspace.path.join("nested");
    fs::create_dir(&nested).unwrap();
    init_repo(&nested);
    assert!(f.workspace.validate_cwd(&nested).is_err());
}

#[test]
fn managed_launch_revalidates_even_when_provider_session_metadata_is_present() {
    let f = Fixture::new();
    f.create();
    let mut metadata = Map::new();
    metadata.insert(
        "managed_worktree".into(),
        json!(f.workspace.clone().pin().unwrap()),
    );
    metadata.insert("provider_session_id".into(), json!("resumed-session"));
    validate_workspace_launch(&metadata, Some(&f.workspace.path)).unwrap();
    git(&f.workspace.path, &["switch", "-c", "manual"]).unwrap();
    assert!(validate_workspace_launch(&metadata, Some(&f.workspace.path)).is_err());
    metadata.remove("provider_session_id");
    assert!(validate_workspace_launch(&metadata, Some(&f.workspace.path)).is_err());
}

#[test]
fn inherited_git_redirection_cannot_stage_or_commit_in_the_primary_checkout() {
    const CHILD: &str = "REFINE_WORKSPACE_ENV_TEST_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = PathBuf::from(root);
        let service = FileGitWorktreeService::new(&root);
        assert_eq!(
            service.head_ref().unwrap().branch.as_deref(),
            Some("refine/GOAL/round-1")
        );
        service.commit("isolated child", &[]).unwrap();
        // Deliberate temporary indexes still work after inherited overrides are removed.
        let index = root.parent().unwrap().join("explicit-index");
        service
            .git_output_with_env(
                &["read-tree", "HEAD"],
                &[("GIT_INDEX_FILE", index.to_str().unwrap())],
            )
            .unwrap();
        assert!(index.is_file());
        return;
    }
    let f = Fixture::new();
    f.create();
    fs::write(f.repo.join("app.txt"), "manual staged\n").unwrap();
    git(&f.repo, &["add", "app.txt"]).unwrap();
    fs::write(f.repo.join("app.txt"), "manual unstaged\n").unwrap();
    fs::write(f.workspace.path.join("app.txt"), "agent\n").unwrap();
    let before = f.primary_snapshot();
    let name = format!(
        "{}::inherited_git_redirection_cannot_stage_or_commit_in_the_primary_checkout",
        module_path!().split_once("::").unwrap().1
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--nocapture"])
        .env(CHILD, &f.workspace.path)
        .env("GIT_DIR", f.repo.join(".git"))
        .env("GIT_COMMON_DIR", f.repo.join(".git"))
        .env("GIT_WORK_TREE", &f.repo)
        .env("GIT_INDEX_FILE", f.repo.join(".git/index"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", &f.repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    assert_eq!(before, f.primary_snapshot());
    assert_eq!(
        git_stdout(&f.workspace.path, &["show", "HEAD:app.txt"]),
        "agent"
    );
}

#[test]
fn a_conflicting_worktree_lock_owner_is_preserved() {
    let f = Fixture::new();
    f.create();
    git(
        &f.repo,
        &["worktree", "unlock", f.workspace.path.to_str().unwrap()],
    )
    .unwrap();
    git(
        &f.repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "manual maintenance",
            f.workspace.path.to_str().unwrap(),
        ],
    )
    .unwrap();
    let before = git_stdout(&f.repo, &["worktree", "list", "--porcelain"]);
    assert!(f.workspace.validate().is_err());
    assert!(
        f.git
            .ensure_worktree(&f.workspace.branch, &f.workspace.path)
            .is_err()
    );
    assert_eq!(
        before,
        git_stdout(&f.repo, &["worktree", "list", "--porcelain"])
    );
}

#[test]
fn goal_agent_rejects_changed_workspace_before_resumed_or_fresh_provider_launch() {
    use crate::application::agents::sessions::{GoalAgentLaunch, run_goal_agent};
    use crate::infrastructure::agents::invocation::ProviderSessionContinuity;
    let f = Fixture::new();
    f.create();
    let workspace = f.workspace.clone().pin().unwrap();
    git(&f.workspace.path, &["switch", "-c", "manual"]).unwrap();
    let runtime = f.temp.join("never-launched");
    for provider_session in [
        Some(ProviderSessionContinuity::Resume("old-session".into())),
        None,
    ] {
        let error = run_goal_agent(
            GoalAgentLaunch {
                runtime_root: runtime.clone(),
                cwd: f.workspace.path.clone(),
                provider: "uninstalled-provider".into(),
                provider_session,
                prompt: "must not launch".into(),
                metadata: serde_json::from_value(json!({"managed_worktree": workspace})).unwrap(),
                completion_timeout: None,
                idle_timeout: None,
            },
            |_| {},
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("checkout branch or commit changed"),
            "{error}"
        );
        assert!(!runtime.exists());
    }
}

#[cfg(unix)]
#[test]
fn provider_resume_uses_explicit_admitted_cwd_and_rejects_lost_registration() {
    use crate::infrastructure::agents::invocation::HostAgentProviderService;
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.create();
    let workspace = f.workspace.clone().pin().unwrap();
    let bin = f.temp.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("codex"), "#!/bin/sh\npwd > resumed-cwd\n").unwrap();
    fs::set_permissions(bin.join("codex"), fs::Permissions::from_mode(0o755)).unwrap();
    let service = HostAgentProviderService {
        path_override: Some(bin.to_string_lossy().into()),
        runtime_root: Some(f.temp.join("runtime")),
    };
    let metadata = Map::from_iter([("managed_worktree".into(), json!(workspace))]);
    assert!(
        service
            .resume_detailed_with_output_and_metadata(
                "codex",
                "old-session",
                metadata.clone(),
                |_| {}
            )
            .is_err()
    );
    service
        .resume_detailed_at_cwd_with_output_and_metadata(
            "codex",
            "old-session",
            Some(&workspace.path),
            metadata.clone(),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(workspace.path.join("resumed-cwd"))
            .unwrap()
            .trim(),
        workspace.path.to_str().unwrap()
    );
    assert!(!f.repo.join("resumed-cwd").exists());
    fs::remove_dir_all(&workspace.path).unwrap();
    assert!(
        service
            .resume_detailed_at_cwd_with_output_and_metadata(
                "codex",
                "old-session",
                Some(&workspace.path),
                metadata,
                |_| {}
            )
            .is_err()
    );
}

#[test]
fn supervised_git_inspection_preserves_primary_index_bytes() {
    use crate::infrastructure::process::supervisor::operations::{
        FileOperationRegistry, OperationRegistry,
    };
    let f = Fixture::new();
    commit_file(&f.repo, "stable.txt", "stable\n", "stable file");
    fs::write(f.repo.join("app.txt"), "manual staged\n").unwrap();
    git(&f.repo, &["add", "app.txt"]).unwrap();
    fs::write(f.repo.join("app.txt"), "manual unstaged\n").unwrap();
    // Equal content with new stat data would otherwise refresh the shared index.
    fs::write(f.repo.join("stable.txt"), "stable\n").unwrap();
    let before = f.primary_snapshot();
    let runtime = f.temp.join("runtime");
    let operation = FileOperationRegistry::new(&runtime)
        .register("inspect:test")
        .unwrap();
    let status = FileGitWorktreeService::with_runtime_root(&f.repo, &runtime)
        .with_operation_id(operation.id)
        .inspect("")
        .unwrap();
    assert!(!status.is_pristine());
    assert_eq!(before, f.primary_snapshot());
}

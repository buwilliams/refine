use super::*;

#[test]
fn read_only_inspection_does_not_contend_for_the_repository_index() {
    let temp_root = unique_temp_dir("git-read-only-index");
    let repo = temp_root.join("repo");
    fs::create_dir_all(&repo).unwrap();
    init_repo(&repo);
    commit_file(&repo, "app.txt", "one\n", "initial");
    fs::write(repo.join("app.txt"), "two\n").unwrap();

    let index_lock = repo.join(".git/index.lock");
    fs::write(&index_lock, "integration owner holds the index\n").unwrap();
    let status = FileGitWorktreeService::new(&repo).inspect("").unwrap();
    assert!(status.dirty_user_changes);

    fs::remove_file(index_lock).unwrap();
    fs::remove_dir_all(temp_root).unwrap();
}

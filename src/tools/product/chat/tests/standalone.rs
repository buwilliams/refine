use super::*;

#[test]
fn file_chat_service_plan_prompt_drafts_software_specs() {
    let temp_root = unique_temp_dir("chat-plan-prompt");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("plan"))
        .unwrap();

    let prompt = service.chat_prompt(&session, "Plan authentication cleanup.");
    assert!(prompt.contains("Co-design software from the user's intent"));
    assert!(prompt.contains("material unknowns"));
    assert!(prompt.contains("ask the user when necessary"));
    assert!(prompt.contains("constraints"));
    assert!(prompt.contains("architecture"));
    assert!(prompt.contains("implementation order"));
    assert!(prompt.contains("verification"));
    assert!(prompt.contains("reviewable Features or Goals"));
    assert!(prompt.contains("Plan authentication cleanup."));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_starts_plan_mode_for_unborn_project_repo() {
    let temp_root = unique_temp_dir("chat-plan-unborn");
    init_unborn_git_app(&temp_root);
    fs::write(temp_root.join("draft.txt"), "local draft\n").unwrap();
    let refine_dir = temp_root.join(".refine");
    let service = FileChatService::new(&refine_dir);

    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("plan"))
        .unwrap();
    let worktree = PathBuf::from(session.worktree.as_ref().unwrap().path.clone());

    assert!(worktree.join(".git").exists());
    assert_eq!(
        git_stdout(&worktree, &["branch", "--show-current"]),
        session.worktree.as_ref().unwrap().branch
    );
    assert_eq!(
        git_stdout(&temp_root, &["log", "--pretty=%s", "-1"]),
        "Initialize Refine workspace"
    );
    assert!(!worktree.join("draft.txt").exists());
    assert!(temp_root.join("draft.txt").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_runs_standalone_provider_turns_in_attached_worktree() {
    let temp_root = unique_temp_dir("chat-standalone-worktree-cwd");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    write_cwd_provider(&refine_dir, "smoke-ai");
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();
    let worktree = PathBuf::from(session.worktree.as_ref().unwrap().path.clone());

    service
        .append_user_message(&session.id, "write cwd marker")
        .unwrap();
    wait_for_chat_line(&service, &session.id, "cwd provider response");
    assert_eq!(
        fs::read_to_string(worktree.join("provider-cwd.txt")).unwrap(),
        format!("{}\n", worktree.display())
    );
    assert!(!temp_root.join("provider-cwd.txt").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

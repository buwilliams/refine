use super::*;

#[test]
fn source_advancement_and_interrupted_process_reuse_pinned_dirty_lifecycle_work() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.backlog.exit", BindingMode::Blocking);
    f.request_todo();
    f.dispatch();
    let mut invocation = f.invocation("workflow.backlog.exit");
    let owner = invocation.context.lifecycle.clone().unwrap();
    let new_target = git(&f.primary, &["rev-parse", "manual"]);
    git(&f.primary, &["update-ref", "refs/heads/main", &new_target]);
    let before = f.snapshot();
    fs::write(owner.path.join("retained.txt"), "retained dirty work").unwrap();
    invocation.state = InvocationState::Running;
    f.service.save_invocation(&invocation).unwrap();
    f.dispatch();
    let resumed = f.invocation("workflow.backlog.exit");
    assert_eq!(resumed.context.lifecycle, invocation.context.lifecycle);
    assert_eq!(resumed.context.workspace, invocation.context.workspace);
    assert_eq!(
        resumed.context.lifecycle.as_ref().unwrap().source_commit,
        f.base
    );
    let result = f.execute(&resumed);
    assert_eq!(result.state, InvocationState::Succeeded, "{result:?}");
    assert_eq!(
        fs::read_to_string(owner.path.join("retained.txt")).unwrap(),
        "retained dirty work"
    );
    f.dispatch();
    f.assert_no_candidate();
    assert_eq!(before, f.snapshot());
}

#[test]
fn pending_and_cached_lifecycle_invocations_reject_missing_replaced_or_wrong_registrations() {
    for completed in [false, true] {
        for changed in ["missing", "replaced", "branch", "repository", "symlink"] {
            let f = Fixture::new();
            let _smoke = SmokeSkill::install(&f.service, &f.temp);
            f.gate("workflow.backlog.exit", BindingMode::Blocking);
            let before = f.snapshot();
            f.request_todo();
            f.dispatch();
            let invocation = f.invocation("workflow.backlog.exit");
            if completed {
                assert_eq!(f.execute(&invocation).state, InvocationState::Succeeded);
            }
            let retained = f.service.invocation(&invocation.id).unwrap();
            let workspace = invocation.context.workspace.as_ref().unwrap();
            match changed {
                "branch" => {
                    git(&workspace.path, &["switch", "-qc", "wrong-branch"]);
                }
                "repository" => {
                    fs::rename(
                        workspace.path.join(".git"),
                        workspace.path.join("retained-git-link"),
                    )
                    .unwrap();
                    git(&workspace.path, &["init", "-q"]);
                }
                "symlink" => {
                    fs::rename(&workspace.path, f.temp.join("retained-workspace")).unwrap();
                    std::os::unix::fs::symlink(&f.primary, &workspace.path).unwrap();
                }
                _ => {
                    fs::rename(&workspace.path, f.temp.join("retained-workspace")).unwrap();
                    if changed == "replaced" {
                        FileGitWorktreeService::new(&f.primary)
                            .ensure_worktree_at_commit(&workspace.branch, &workspace.path, &f.base)
                            .unwrap();
                    }
                }
            }
            assert!(
                f.service.execute(&invocation.id, || Ok(())).is_err(),
                "{completed} {changed}"
            );
            f.dispatch();
            assert_eq!(
                f.work().show_goal_detail("FRESH").unwrap()["status"],
                "failed"
            );
            if completed {
                assert_eq!(f.service.invocation(&invocation.id).unwrap(), retained);
            }
            assert_eq!(before, f.snapshot(), "{changed}");
        }
    }
}

#[test]
fn nested_repository_and_subpath_symlink_are_rejected_before_launch() {
    for variant in ["nested", "symlink"] {
        let f = Fixture::new();
        let _smoke = SmokeSkill::install(&f.service, &f.temp);
        crate::infrastructure::process::supervisor::config::FileSettingsService::for_node(
            &f.service.refine_dir,
            "default",
        )
        .update(&json!({"agent_subpath":"app"}))
        .unwrap();
        f.gate("workflow.backlog.exit", BindingMode::Blocking);
        let before = f.snapshot();
        f.request_todo();
        f.dispatch();
        let invocation = f.invocation("workflow.backlog.exit");
        if variant == "nested" {
            git(&invocation.context.cwd, &["init", "-q"]);
        } else {
            fs::rename(
                &invocation.context.cwd,
                invocation.context.cwd.with_extension("retained"),
            )
            .unwrap();
            std::os::unix::fs::symlink(&f.primary, &invocation.context.cwd).unwrap();
        }
        assert!(f.service.execute(&invocation.id, || Ok(())).is_err());
        assert_eq!(before, f.snapshot());
    }
}

#[test]
fn inherited_git_redirection_cannot_redirect_lifecycle_launches_or_writes() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.backlog.exit", BindingMode::Blocking);
    f.request_todo();
    f.dispatch();
    let invocation = f.invocation("workflow.backlog.exit");
    let before = f.snapshot();
    struct Restore(Vec<(String, Option<std::ffi::OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(key, v),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }
    let keys = [
        ("GIT_DIR", f.primary.join(".git")),
        ("GIT_WORK_TREE", f.primary.clone()),
        ("GIT_INDEX_FILE", f.primary.join(".git/index")),
    ];
    let restore = Restore(
        keys.iter()
            .map(|(k, _)| (k.to_string(), std::env::var_os(k)))
            .collect(),
    );
    for (key, path) in &keys {
        unsafe {
            std::env::set_var(key, path);
        }
    }
    let result = f.execute(&invocation);
    drop(restore);
    assert_eq!(result.state, InvocationState::Succeeded, "{result:?}");
    assert_eq!(before, f.snapshot());
}

fn persist_admission_intent(f: &Fixture) -> EventInvocation {
    let config = f.service.config().unwrap();
    let event = &config.events["workflow.backlog.exit"];
    let mut context = f
        .service
        .manual_context(&f.primary, &json!({"goal_id":"FRESH"}))
        .unwrap();
    let goal = f.work().show_goal_detail("FRESH").unwrap();
    context.data["lifecycle_transition"] = goal["pending_event_transition"].clone();
    let occurrence = goal["pending_event_transition"]["id"].as_str().unwrap();
    let id = stable_id(&format!("{occurrence}:{}", event.id));
    let bindings = f
        .service
        .resolve_bindings(&config, event, &mut context, &BTreeMap::new())
        .unwrap();
    f.service
        .pin_lifecycle_intent(event, &mut context, &id, occurrence)
        .unwrap();
    let invocation = EventInvocation {
        id,
        event: event.clone(),
        config_revision: config.revision,
        context,
        bindings,
        state: InvocationState::Pending,
        results: BTreeMap::new(),
        attempts: Vec::new(),
        created_at: now(),
        completed_at: None,
        error: None,
        action_applied: false,
    };
    f.service.save_invocation(&invocation).unwrap();
    invocation
}

#[test]
fn interrupted_admission_recovers_only_unambiguous_persisted_ownership() {
    for interruption in [
        "before_creation",
        "after_creation",
        "after_registration",
        "cancelled",
        "request_changed",
    ] {
        let f = Fixture::new();
        let _smoke = SmokeSkill::install(&f.service, &f.temp);
        f.gate("workflow.backlog.exit", BindingMode::Blocking);
        f.request_todo();
        let mut invocation = persist_admission_intent(&f);
        let owner = invocation.context.lifecycle.clone().unwrap();
        let repository = FileGitWorktreeService::new(&f.primary);
        match interruption {
            "after_creation" | "after_registration" => {
                repository
                    .create_worktree_from_base(&owner.branch, &owner.path, &owner.source_commit)
                    .unwrap();
                fs::write(owner.path.join("retained.txt"), "interrupted contents").unwrap();
                if interruption == "after_registration" {
                    invocation.context.workspace =
                        Some(invocation.context.workspace.clone().unwrap().pin().unwrap());
                    f.service.save_invocation(&invocation).unwrap();
                }
            }
            "cancelled" => {
                f.work().cancel_goal_summary("FRESH").unwrap();
            }
            "request_changed" => {
                f.work()
                    .update_goal_assignee_summary("FRESH", "Changed author")
                    .unwrap();
            }
            _ => {}
        }
        // A replay must use the source already persisted, even if target advances.
        let target = git(&f.primary, &["rev-parse", "manual"]);
        git(&f.primary, &["update-ref", "refs/heads/main", &target]);
        let before = f.snapshot();
        f.dispatch();
        let retained = f.service.invocation(&invocation.id).unwrap();
        if ["before_creation", "after_registration"].contains(&interruption) {
            assert_eq!(
                retained.state,
                InvocationState::Pending,
                "{interruption}: {retained:?}"
            );
            assert_eq!(
                retained.context.lifecycle.as_ref().unwrap().source_commit,
                f.base
            );
            assert_eq!(f.execute(&retained).state, InvocationState::Succeeded);
        } else if interruption == "after_creation" {
            assert_eq!(retained.state, InvocationState::Error);
            assert!(retained.error.unwrap().contains("interrupted"));
            assert_eq!(
                fs::read_to_string(owner.path.join("retained.txt")).unwrap(),
                "interrupted contents"
            );
        } else {
            assert!(!owner.path.exists());
        }
        assert_eq!(before, f.snapshot());
    }
}

#[test]
fn queued_lifecycle_dispatch_restarts_without_duplicate_processes() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.backlog.exit", BindingMode::Blocking);
    let before = f.snapshot();
    f.request_todo();
    f.dispatch();
    let invocation = f.invocation("workflow.backlog.exit");
    let restarted =
        FileEventService::with_runtime_root(&f.service.refine_dir, f.temp.join("runtime"));
    assert_eq!(restarted.dispatch_pending(&f.primary).unwrap(), 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        restarted.dispatch_pending(&f.primary).unwrap();
        let result = restarted.invocation(&invocation.id).unwrap();
        if result.state.terminal() {
            assert_eq!(result.state, InvocationState::Succeeded, "{result:?}");
            assert_eq!(result.attempts.len(), 1);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "dispatcher did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // Wait for the dispatch reservation to settle before tearing down the fixture.
    while crate::application::workflow::engine::admission::reservations(&f.temp.join("runtime"))
        .iter()
        .any(|r| r.invocation_id.as_deref() == Some(invocation.id.as_str()))
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    f.dispatch();
    f.assert_no_candidate();
    assert_eq!(before, f.snapshot());
}

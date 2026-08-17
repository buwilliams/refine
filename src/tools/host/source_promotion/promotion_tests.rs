use super::*;

#[test]
fn candidate_build_spawn_failure_cleans_worktree_and_allows_retry() {
    let root = test_directory("source-candidate-retry");
    let checkout = root.join("checkout");
    let commit = initialize_git_repository(&checkout);
    let service = FileSourcePromotionService::new(&checkout, root.join("runtime/8080"), 8080);
    let mut host = FileSourcePromotionHost::new(service.clone());
    host.candidate_builder = root.join("missing-candidate-builder");

    for _ in 0..2 {
        let error = host.build_candidate(&commit).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to launch candidate build"),
            "{error}"
        );
        let worktrees = git_text(&checkout, &["worktree", "list", "--porcelain"]).unwrap();
        assert_eq!(
            worktrees
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count(),
            1,
            "{worktrees}"
        );
    }
    let artifact_root = service.port_runtime_root.join("source-promotion");
    assert_eq!(fs::read_dir(artifact_root).unwrap().count(), 0);

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn candidate_build_reports_primary_and_unrecovered_cleanup_failures() {
    use std::os::unix::fs::PermissionsExt;

    let root = test_directory("source-candidate-cleanup-failure");
    let checkout = root.join("checkout");
    let commit = initialize_git_repository(&checkout);
    let service = FileSourcePromotionService::new(&checkout, root.join("runtime/8080"), 8080);
    let builder = root.join("fail-build");
    fs::write(
        &builder,
        "#!/bin/sh\nchmod 0555 ..\necho 'mock primary build failure' >&2\nexit 42\n",
    )
    .unwrap();
    fs::set_permissions(&builder, fs::Permissions::from_mode(0o755)).unwrap();
    let mut host = FileSourcePromotionHost::new(service.clone());
    host.candidate_builder = builder;

    let error = host.build_candidate(&commit).unwrap_err();
    let artifact_root = service.port_runtime_root.join("source-promotion");
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o755)).unwrap();
    let candidate = fs::read_dir(&artifact_root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .unwrap();
    assert!(
        cleanup_candidate_worktree(&checkout, &candidate, true).is_empty(),
        "test cleanup should recover after restoring permissions"
    );

    let message = error.to_string();
    assert!(message.contains("mock primary build failure"), "{message}");
    assert!(
        message.contains("candidate cleanup also failed"),
        "{message}"
    );
    assert!(message.contains("git worktree prune"), "{message}");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_activation_advances_ref_index_and_worktree_together() {
    let repository = initialize_promotion_repository("source-activation");
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);

    host.activate(&repository.from_commit, &repository.to_commit)
        .unwrap();

    assert_checked_out_commit(
        &repository.checkout,
        &repository.to_commit,
        "promoted fixture\n",
    );
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_activation_ref_failure_restores_prior_index_and_worktree() {
    let repository = initialize_promotion_repository("source-activation-ref-failure");
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);
    let lock = repository.checkout.join(".git/refs/heads/main.lock");
    fs::write(&lock, "locked for source-promotion test\n").unwrap();

    let error = host
        .activate(&repository.from_commit, &repository.to_commit)
        .unwrap_err();

    assert!(error.to_string().contains("update-ref"), "{error}");
    fs::remove_file(lock).unwrap();
    assert_checked_out_commit(
        &repository.checkout,
        &repository.from_commit,
        "prior fixture\n",
    );
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_promotion_rollback_restores_prior_ref_index_and_worktree() {
    let repository = initialize_promotion_repository("source-rollback");
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);
    host.activate(&repository.from_commit, &repository.to_commit)
        .unwrap();

    host.rollback(&repository.from_commit, &repository.to_commit)
        .unwrap();

    assert_checked_out_commit(
        &repository.checkout,
        &repository.from_commit,
        "prior fixture\n",
    );
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_activation_leaves_dirty_checkout_untouched() {
    let repository = initialize_promotion_repository("source-activation-dirty");
    fs::write(repository.checkout.join("fixture.txt"), "local work\n").unwrap();
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);

    let error = host
        .activate(&repository.from_commit, &repository.to_commit)
        .unwrap_err();

    assert!(error.to_string().contains("checkout changed"), "{error}");
    assert_eq!(
        git_text(&repository.checkout, &["rev-parse", "HEAD"]).unwrap(),
        repository.from_commit
    );
    assert_eq!(
        git_text(&repository.checkout, &["write-tree"]).unwrap(),
        git_text(
            &repository.checkout,
            &["rev-parse", &format!("{}^{{tree}}", repository.from_commit)]
        )
        .unwrap()
    );
    assert_eq!(
        fs::read_to_string(repository.checkout.join("fixture.txt")).unwrap(),
        "local work\n"
    );
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_activation_leaves_diverged_checkout_untouched() {
    let repository = initialize_promotion_repository("source-activation-diverged");
    fs::write(
        repository.checkout.join("fixture.txt"),
        "diverged fixture\n",
    )
    .unwrap();
    git_ok(&repository.checkout, &["add", "fixture.txt"]).unwrap();
    git_ok(
        &repository.checkout,
        &["commit", "--quiet", "-m", "diverged source"],
    )
    .unwrap();
    let diverged_commit = git_text(&repository.checkout, &["rev-parse", "HEAD"]).unwrap();
    let service = FileSourcePromotionService::new(
        &repository.checkout,
        repository.root.join("runtime/8080"),
        8080,
    );
    let mut host = FileSourcePromotionHost::new(service);

    let error = host
        .activate(&diverged_commit, &repository.to_commit)
        .unwrap_err();

    assert!(error.to_string().contains("fast-forward"), "{error}");
    assert_checked_out_commit(&repository.checkout, &diverged_commit, "diverged fixture\n");
    fs::remove_dir_all(repository.root).unwrap();
}

#[test]
fn source_promotion_builds_before_stopping_and_verifies_restart() {
    let mut host = FakeHost::default();
    let mut operation = operation();
    let mut states = Vec::new();
    run_source_promotion(&mut host, &mut operation, |state| {
        states.push((state.status.clone(), state.stage.clone()));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        host.calls,
        [
            "build",
            "preflight",
            "register",
            "stop",
            "activate_binary",
            "activate",
            "restart",
            "verify",
            "complete_registration"
        ]
    );
    assert_eq!(operation.status, "succeeded");
    assert_eq!(operation.stage, "complete");
    assert_eq!(states.first().unwrap().1, "build_candidate");
}

#[test]
fn source_promotion_build_failure_never_stops_or_activates() {
    let mut host = FakeHost {
        fail: Some("build"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(host.calls, ["build"]);
    assert_eq!(operation.stage, "build_candidate");
    assert_eq!(operation.status, "failed");
}

#[test]
fn source_promotion_restart_failure_rolls_back_and_recovers_previous_daemon() {
    let mut host = FakeHost {
        fail: Some("restart"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(
        host.calls,
        [
            "build",
            "preflight",
            "register",
            "stop",
            "activate_binary",
            "activate",
            "restart",
            "rollback",
            "restore_binary",
            "restore_registration",
            "verify_registration",
            "restart_previous",
            "observe_previous",
            "complete_registration"
        ]
    );
    assert_eq!(operation.rollback_succeeded, Some(true));
    assert!(operation.recovery.as_deref().unwrap().contains("restored"));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.registration_verified, Some(true));
    assert_eq!(evidence.replacement_succeeded, Some(true));
    assert_eq!(evidence.identity_matches, Some(true));
    assert_eq!(
        evidence.observed_executable.as_deref(),
        Some("/previous/refine")
    );
}

#[test]
fn source_promotion_rollback_restart_failure_never_reports_success() {
    let mut host = FakeHost {
        fail: Some("verify"),
        rollback_fail: Some("restart_previous"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.rollback_succeeded, Some(false));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert!(evidence.replacement_attempted);
    assert_eq!(evidence.replacement_succeeded, Some(false));
    assert!(
        evidence
            .errors
            .iter()
            .any(|error| error.contains("forced prior daemon replacement failed"))
    );
}

#[test]
fn source_promotion_rollback_indeterminate_identity_never_reports_success() {
    let mut host = FakeHost {
        fail: Some("verify"),
        rollback_fail: Some("observe_unknown"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.rollback_succeeded, Some(false));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.reachability, "reachable");
    assert_eq!(evidence.identity_matches, None);
    assert_eq!(evidence.observed_executable, None);
    assert!(
        operation
            .recovery
            .as_deref()
            .unwrap()
            .contains("partial or indeterminate")
    );
}

#[test]
fn source_promotion_rollback_registration_verification_failure_withholds_restart() {
    let mut host = FakeHost {
        fail: Some("verify"),
        rollback_fail: Some("verify_registration"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.rollback_succeeded, Some(false));
    assert!(!host.calls.contains(&"restart_previous".to_string()));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.registration_restored, Some(true));
    assert_eq!(evidence.registration_verified, Some(false));
    assert!(!evidence.replacement_attempted);
}

#[test]
fn source_promotion_rollback_observed_candidate_never_reports_success() {
    let mut host = FakeHost {
        fail: Some("verify"),
        rollback_fail: Some("observe_candidate"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.rollback_succeeded, Some(false));
    let evidence = operation.rollback_evidence.as_ref().unwrap();
    assert_eq!(evidence.reachability, "reachable");
    assert_eq!(evidence.identity_matches, Some(false));
    assert_eq!(
        evidence.observed_executable.as_deref(),
        Some("/candidate/refine")
    );
}

#[test]
fn source_activation_failure_verifies_prior_source_before_forced_restart() {
    let mut host = FakeHost {
        fail: Some("activate"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());

    assert_eq!(operation.stage, "activate_source");
    assert_eq!(operation.rollback_succeeded, Some(true));
    assert!(host.calls.contains(&"verify_previous_source".to_string()));
    assert!(host.calls.contains(&"restart_previous".to_string()));
}

#[test]
fn source_promotion_stop_failure_restores_prepared_service_registration() {
    let mut host = FakeHost {
        fail: Some("stop"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(
        host.calls,
        [
            "build",
            "preflight",
            "register",
            "stop",
            "restore_binary",
            "restore_registration",
            "verify_registration",
            "complete_registration"
        ]
    );
    assert_eq!(operation.stage, "stop_daemon");
    assert_eq!(operation.registration_rollback_succeeded, Some(true));
    assert!(
        operation
            .recovery
            .as_deref()
            .unwrap()
            .contains("fresh lifecycle evidence")
    );
}

#[test]
fn source_promotion_registration_failure_never_stops_or_activates() {
    let mut host = FakeHost {
        fail: Some("register"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(host.calls, ["build", "preflight", "register"]);
    assert_eq!(operation.stage, "prepare_service_registration");
}

#[test]
fn source_promotion_active_work_after_build_never_stops_or_activates() {
    let mut host = FakeHost {
        fail: Some("preflight"),
        ..Default::default()
    };
    let mut operation = operation();
    assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
    assert_eq!(host.calls, ["build", "preflight"]);
    assert_eq!(operation.stage, "verify_idle");
}

#[test]
fn validation_rejects_dirty_diverged_active_and_current_snapshots() {
    let base = SourcePromotionSnapshot {
        checkout_path: "/refine".to_string(),
        current_commit: "aaa".to_string(),
        remote: "origin".to_string(),
        local_branch: "main".to_string(),
        branch: "main".to_string(),
        available_commit: "bbb".to_string(),
        relationship: "behind".to_string(),
        clean: true,
        fast_forward: true,
        update_available: true,
        active_work: Vec::new(),
        operation: None,
    };
    let mut dirty = base.clone();
    dirty.clean = false;
    assert!(
        validate_promotion(&dirty)
            .unwrap_err()
            .to_string()
            .contains("clean")
    );
    let mut diverged = base.clone();
    diverged.fast_forward = false;
    assert!(
        validate_promotion(&diverged)
            .unwrap_err()
            .to_string()
            .contains("fast-forward")
    );
    let mut active = base.clone();
    active.active_work.push("active Goal claim G1".to_string());
    assert!(
        validate_promotion(&active)
            .unwrap_err()
            .to_string()
            .contains("idle")
    );
    let mut current = base;
    current.update_available = false;
    assert!(
        validate_promotion(&current)
            .unwrap_err()
            .to_string()
            .contains("already")
    );
}

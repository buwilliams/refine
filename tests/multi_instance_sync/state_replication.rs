use super::*;

#[test]
#[ignore = "daemon-backed multi-instance test; run through `cargo run --manifest-path xtask/Cargo.toml -- test-multi-instance-sync`"]
fn two_daemons_sync_refine_state_through_shared_git_remote() {
    let root = temp_root("multi-instance-sync");
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    let app_a = root.join("app-a");
    let app_b = root.join("app-b");
    let runtime_a = root.join("run-a");
    let runtime_b = root.join("run-b");
    let artifacts = root.join("artifacts");

    seed_remote(&remote, &seed);
    git(
        &root,
        &["clone", remote.to_str().unwrap(), app_a.to_str().unwrap()],
    );
    git(
        &root,
        &["clone", remote.to_str().unwrap(), app_b.to_str().unwrap()],
    );
    configure_repo(&app_a);
    configure_repo(&app_b);

    let mut instance_a = RefineInstance::start("a", &runtime_a, &app_a, &artifacts);
    let published_bootstrap = FileGitSyncService::new(&app_a, &runtime_a).sync().unwrap();
    assert!(
        published_bootstrap.pushed,
        "first node did not publish bootstrap state: {published_bootstrap:?}"
    );
    let observed_bootstrap = FileGitSyncService::new(&app_b, &runtime_b).sync().unwrap();
    assert!(
        observed_bootstrap.pulled,
        "second node did not observe bootstrap state: {observed_bootstrap:?}"
    );
    let mut instance_b = RefineInstance::start("b", &runtime_b, &app_b, &artifacts);

    let first_label = format!("multi-instance first {}", now_millis());
    let first_goal = instance_a.create_goal(&first_label);
    assert!(!first_goal.is_empty());
    instance_b.wait_for_goal(&first_label);

    let second_label = format!("multi-instance second {}", now_millis());
    let second_goal = instance_b.create_goal(&second_label);
    assert!(!second_goal.is_empty());
    instance_a.wait_for_goal(&first_label);
    instance_a.wait_for_goal(&second_label);

    instance_b.stop();
    instance_a.stop();
    let _ = fs::remove_dir_all(root);
}

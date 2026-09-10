use super::*;
use crate::infrastructure::process::subprocess::{ManagedProcessSpec, ProcessSupervisor};

#[test]
fn missing_primary_registration_keeps_owned_worktree_until_group_exit() {
    let fixture = Fixture::new("retained-process-worktree");
    fixture.create_goal("GOAL1", "refine/GOAL1/round-1", true);
    let worktree = fixture.add_worktree("refine/GOAL1/round-1");
    let supervisor = FileProcessSupervisor::new(&fixture.runtime_root);
    let process = supervisor
        .launch(ManagedProcessSpec {
            owner: ProcessOwner::Maintenance,
            command: "/bin/sleep".into(),
            args: vec!["60".into()],
            cwd: Some(worktree.display().to_string()),
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: serde_json::from_value(json!({"isolated_process_group":true,
            "workflow_incarnation":uuid::Uuid::new_v4().to_string(), "goal_id":"GOAL1"}))
            .unwrap(),
        })
        .unwrap();
    let group = supervisor.owned_groups().unwrap().remove(0);
    fs::remove_file(
        supervisor
            .processes_dir()
            .join(format!("{}.json", process.id)),
    )
    .unwrap();
    let cleanup = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root);
    let run = || {
        cleanup
            .run(WorktreeCleanupOptions {
                apply: true,
                older_than_seconds: 0,
            })
            .unwrap()
    };
    assert_eq!(run().removed, 0);
    assert!(worktree.exists());
    supervisor
        .stop_owned_group(&group, std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(run().removed, 1);
    assert!(!worktree.exists());
    assert!(git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "refs/heads/refine/GOAL1/round-1"]
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn cleanup_preserves_unobserved_descendants_and_only_releases_complete_lifetime_proof() {
    use crate::infrastructure::process::subprocess::owned_groups::test_fixture::UnobservedChild;
    for tracked in [false, true] {
        for remove_primary in [false, true] {
            let fixture = Fixture::new("unobserved-worktree");
            fixture.create_goal("GOAL1", "refine/GOAL1/round-1", true);
            let worktree = fixture.add_worktree("refine/GOAL1/round-1");
            let supervisor = FileProcessSupervisor::new(&fixture.runtime_root);
            let child = UnobservedChild::launch(
                &supervisor,
                tracked,
                json!({
                    "workflow_incarnation":uuid::Uuid::new_v4().to_string(), "goal_id":"GOAL1", "worktree":worktree
                }),
            );
            assert!(child.child_alive());
            assert!(!child.group.witnesses.contains_key(&child.child_pid));
            if remove_primary {
                fs::remove_file(
                    supervisor
                        .processes_dir()
                        .join(format!("{}.json", child.group.process.id)),
                )
                .unwrap();
            }
            let cleanup = FileWorktreeCleanupService::new(&fixture.repo, &fixture.runtime_root);
            let run = || {
                cleanup
                    .run(WorktreeCleanupOptions {
                        apply: true,
                        older_than_seconds: 0,
                    })
                    .unwrap()
            };
            assert_eq!(run().removed, 0);
            assert!(worktree.exists());
            let stopped =
                supervisor.stop_owned_group(&child.group, std::time::Duration::from_secs(2));
            if tracked {
                assert!(stopped.unwrap().confirmed_exit);
                assert!(!child.child_alive());
                assert_eq!(run().removed, 1);
                assert!(!worktree.exists());
            } else {
                assert!(stopped.is_err());
                assert!(child.child_alive());
                child.kill_child();
                assert_eq!(
                    run().removed,
                    0,
                    "later empty scans cannot repair lost lifetime evidence"
                );
                assert!(worktree.exists());
            }
            assert!(git_succeeds(
                &fixture.repo,
                &["rev-parse", "--verify", "refs/heads/refine/GOAL1/round-1"]
            ));
        }
    }
}

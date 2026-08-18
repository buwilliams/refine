//! Regression tests for crash-window and first-contact sync bugs found in
//! adversarial review: independent bootstraps joining, interrupted adoption
//! hydration, interrupted contested-join recovery, and remote-authority join
//! settlement. Each scenario drives real `FileGitSyncService` nodes against a
//! local bare remote.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use refine::tools::host::git_sync::{
    FileGitSyncService, StateRecoveryAuthority, StateRecoveryDecision,
    latest_state_sync_conflict_report,
};
use refine::tools::host::project_layout::refine_dir_for_target_root;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed in {}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_out(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "refine-sync-regression-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn configure(root: &Path) {
    git(root, &["config", "user.email", "regression@test"]);
    git(root, &["config", "user.name", "Regression"]);
}

fn node(root: &Path, remote: &Path, id: &str) -> PathBuf {
    let target = root.join(id);
    git(
        root,
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            target.to_str().unwrap(),
        ],
    );
    configure(&target);
    let live = refine_dir_for_target_root(&target).unwrap();
    let runtime_local = live.join("runtime");
    fs::create_dir_all(&runtime_local).unwrap();
    fs::write(
        runtime_local.join("active-node.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "active_node_id": id,
            "refine_dir": live.display().to_string(),
            "updated_at": "2026-08-17T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
    target
}

fn service(target: &Path) -> FileGitSyncService {
    FileGitSyncService::new(target, target.join("run"))
}

fn write_goal(target: &Path, goal_id: &str, name: &str) -> String {
    let relative = format!("goals/{}/{}/goal.json", &goal_id[..2], &goal_id[2..]);
    let path = refine_dir_for_target_root(target).unwrap().join(&relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(r#"{{"id":"{goal_id}","name":"{name}","status":"todo","rounds":[]}}"#),
    )
    .unwrap();
    relative
}

fn fixture(name: &str) -> (PathBuf, PathBuf) {
    let root = unique_temp_dir(name);
    let remote = root.join("remote.git");
    let seed = root.join("seed");
    fs::create_dir_all(&seed).unwrap();
    git(&root, &["init", "--bare", remote.to_str().unwrap()]);
    git(&seed, &["init", "-q"]);
    configure(&seed);
    fs::write(seed.join("app.txt"), "base\n").unwrap();
    git(&seed, &["add", "app.txt"]);
    git(&seed, &["commit", "-q", "-m", "initial"]);
    git(&seed, &["branch", "-M", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-q", "-u", "origin", "main"]);
    (root, remote)
}

/// Two nodes bootstrap the orphan state branch independently (push race or
/// standalone-then-join). The histories share no root, so sync joins them
/// with an empty-tree-base merge instead of wedging on unrelated histories.
#[test]
fn independent_bootstraps_join_through_an_empty_tree_merge() {
    let (root, remote) = fixture("bootstrap-race");
    let a = node(&root, &remote, "node-a");
    let b = node(&root, &remote, "node-b");

    // Block state pushes so node B creates its orphan branch but cannot publish.
    let hook = remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do\n  if test \"$ref\" = refs/heads/refine/state; then exit 1; fi\ndone\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let goal_b = write_goal(&b, "BBGOAL", "b1");
    let blocked = service(&b).sync();
    assert!(blocked.is_err(), "push was blocked: {blocked:?}");
    assert!(
        git_out(&b, &["rev-parse", "refine/state"]).is_some(),
        "B created its local orphan state branch"
    );

    // Node A wins the bootstrap race.
    fs::remove_file(&hook).unwrap();
    let goal_a = write_goal(&a, "AAGOAL", "a1");
    let published = service(&a).sync().unwrap();
    assert!(published.pushed);

    // B converges by merging the unrelated histories from the empty tree.
    let joined = service(&b).sync().unwrap();
    assert!(joined.ok && joined.pushed, "{joined:?}");
    for goal in [&goal_a, &goal_b] {
        assert!(
            git_out(&remote, &["show", &format!("refine/state:.refine/{goal}")]).is_some(),
            "converged head carries {goal}"
        );
    }
    let live_b = refine_dir_for_target_root(&b).unwrap();
    assert!(live_b.join(&goal_a).exists(), "B hydrated A's record");
    let head = git_out(&remote, &["rev-parse", "refine/state"]).unwrap();
    let parents = git_out(&remote, &["rev-list", "--parents", "-n", "1", &head]).unwrap();
    assert_eq!(
        parents.split_whitespace().count(),
        3,
        "the join is a two-parent merge commit: {parents}"
    );

    // Reruns are no-ops and A picks up B's record.
    let rerun = service(&b).sync().unwrap();
    assert!(!rerun.pushed && !rerun.pulled, "{rerun:?}");
    service(&a).sync().unwrap();
    let live_a = refine_dir_for_target_root(&a).unwrap();
    assert!(live_a.join(&goal_b).exists(), "A hydrated B's record");
}

/// Live store wiped (the hibernation class), adoption hydration interrupted
/// partway: the rerun must resume the adoption from its durable marker rather
/// than commit deletions of every record hydration had not yet copied.
#[test]
#[cfg(unix)]
fn interrupted_adoption_resumes_without_inventing_deletions() {
    use std::os::unix::fs::PermissionsExt;

    let (root, remote) = fixture("adopt-crash");
    let a = node(&root, &remote, "node-a");
    let b = node(&root, &remote, "node-b");

    let g1 = write_goal(&a, "AAAGOAL", "v1");
    let g2 = write_goal(&a, "ZZGOAL", "v1");
    assert!(service(&a).sync().unwrap().pushed);
    service(&b).sync().unwrap();
    let live_b = refine_dir_for_target_root(&b).unwrap();
    assert!(live_b.join(&g2).exists(), "B adopted both records");

    // Live store wiped: only machine-local runtime identity survives.
    fs::remove_dir_all(live_b.join("goals")).unwrap();

    // Interrupt adoption hydration between records: G2's destination
    // directory is unwritable, exactly like a crash after G1 hydrated.
    let g2_dir = live_b.join("goals/ZZ/GOAL");
    fs::create_dir_all(&g2_dir).unwrap();
    fs::set_permissions(&g2_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let interrupted = service(&b).sync();
    assert!(interrupted.is_err(), "hydration was blocked");
    assert!(live_b.join(&g1).exists(), "first record was hydrated");
    assert!(!live_b.join(&g2).exists(), "second record was not");

    // The "crash" clears; rerun resumes the marked adoption.
    fs::set_permissions(&g2_dir, fs::Permissions::from_mode(0o755)).unwrap();
    let rerun = service(&b).sync().unwrap();
    assert!(rerun.ok, "{rerun:?}");
    assert!(
        git_out(&remote, &["show", &format!("refine/state:.refine/{g2}")]).is_some(),
        "rerun after interrupted adoption must not delete {g2} from the branch"
    );
    assert!(
        live_b.join(&g2).exists(),
        "rerun hydrated the missing record"
    );
    assert!(
        git_out(&b, &["rev-parse", "refs/refine/state-adoption"]).is_none(),
        "the adoption marker is cleared once the pass converges"
    );

    // A converged pass keeps every record everywhere.
    let settled = service(&b).sync().unwrap();
    assert!(settled.ok, "{settled:?}");
    assert!(git_out(&remote, &["show", &format!("refine/state:.refine/{g1}")]).is_some());
}

/// Contested-join recovery interrupted after the live store was retained by
/// ref: the rerun must converge, and the first retention must survive intact
/// rather than being clobbered or wedging the retention fetch.
#[test]
#[cfg(unix)]
fn contested_join_recovery_rerun_converges_and_preserves_prior_retention() {
    let (root, remote) = fixture("join-retain-rerun");
    let a = node(&root, &remote, "node-a");
    let b = node(&root, &remote, "node-b");

    fs::write(
        refine_dir_for_target_root(&a).unwrap().join("shared.json"),
        b"{\"node\":\"a\"}\n",
    )
    .unwrap();
    write_goal(&a, "AAAGOAL", "v1");
    assert!(service(&a).sync().unwrap().pushed);

    // Joining node contests shared.json; a file blocking the goal-record
    // directory interrupts the recovery's hydration after retention.
    let live_b = refine_dir_for_target_root(&b).unwrap();
    fs::create_dir_all(&live_b).unwrap();
    fs::write(live_b.join("shared.json"), b"{\"node\":\"b\"}\n").unwrap();
    service(&b).sync().unwrap_err();

    fs::create_dir_all(live_b.join("goals")).unwrap();
    fs::write(live_b.join("goals/AA"), b"blocker").unwrap();
    let remote_head = git_out(&b, &["rev-parse", "origin/refine/state"]).unwrap();
    let first_retained = format!("refs/refine/retained/live-{}", &remote_head[..12]);
    let first = service(&b).run_state_recovery(StateRecoveryDecision::uniform(
        StateRecoveryAuthority::Remote,
    ));
    assert!(first.is_err(), "hydration was blocked: {first:?}");
    assert_eq!(
        git_out(&b, &["show", &format!("{first_retained}:.refine/goals/AA")]).as_deref(),
        Some("blocker"),
        "the interrupted attempt retained the pre-recovery live store"
    );

    fs::remove_file(live_b.join("goals/AA")).unwrap();
    let rerun = service(&b)
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap();
    assert!(rerun.ok && rerun.recovered, "{rerun:?}");
    assert_eq!(
        fs::read_to_string(live_b.join("shared.json")).unwrap(),
        "{\"node\":\"a\"}\n"
    );
    // The first retention still holds the displaced bytes exactly; the rerun
    // retained its own (changed) live snapshot under a fresh name.
    assert_eq!(
        git_out(
            &b,
            &["show", &format!("{first_retained}:.refine/shared.json")]
        )
        .as_deref(),
        Some("{\"node\":\"b\"}")
    );
    let rerun_retained = &rerun.recovery.as_ref().unwrap().retained_refs[0];
    assert_ne!(rerun_retained, &first_retained);
    assert!(rerun_retained.starts_with("refs/refine/retained/live-"));
}

/// A node joins with one contested path and an uncontested live-only goal:
/// uniform remote authority settles only the contested path — the local-only
/// record stays in live and is published with the join.
#[test]
fn remote_authority_join_keeps_and_publishes_uncontested_local_records() {
    let (root, remote) = fixture("join-remote-keeps-local");
    let a = node(&root, &remote, "node-a");
    let b = node(&root, &remote, "node-b");

    fs::write(
        refine_dir_for_target_root(&a).unwrap().join("shared.json"),
        b"{\"node\":\"a\"}\n",
    )
    .unwrap();
    assert!(service(&a).sync().unwrap().pushed);

    let live_b = refine_dir_for_target_root(&b).unwrap();
    fs::create_dir_all(&live_b).unwrap();
    fs::write(live_b.join("shared.json"), b"{\"node\":\"b\"}\n").unwrap();
    let local_only = write_goal(&b, "BBONLY", "local-work");
    service(&b).sync().unwrap_err();

    let run = service(&b)
        .run_state_recovery(StateRecoveryDecision::uniform(
            StateRecoveryAuthority::Remote,
        ))
        .unwrap();
    assert!(run.ok && run.recovered, "{run:?}");
    assert_eq!(
        run.recovery.as_ref().unwrap().settled_paths,
        vec!["shared.json".to_string()]
    );
    assert_eq!(
        fs::read_to_string(live_b.join("shared.json")).unwrap(),
        "{\"node\":\"a\"}\n"
    );
    assert!(
        live_b.join(&local_only).exists(),
        "the uncontested local-only record stays in live"
    );
    assert!(
        git_out(
            &remote,
            &["show", &format!("refine/state:.refine/{local_only}")]
        )
        .is_some(),
        "the uncontested local-only record is published with the join"
    );
}

/// An agent resolution whose publication is interrupted (crash window between
/// the gated result commit and the push) must complete on rerun from its
/// durable refs alone: the surviving result publishes WITHOUT re-invoking the
/// resolver, and the retired refs plus the cleared conflict report prove the
/// resolution left nothing behind.
#[test]
#[cfg(unix)]
fn interrupted_resolution_publish_completes_on_rerun_without_reinvoking_the_resolver() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use refine::tools::git::resolve::{
        ResolutionRequest, ResolverOutcome, ScriptedResolver, ScriptedStep,
        install_resolver_override,
    };

    let (root, remote) = fixture("resolve-publish-crash");
    let a = node(&root, &remote, "node-a");
    let b = node(&root, &remote, "node-b");
    let shared = |target: &Path| {
        refine_dir_for_target_root(target)
            .unwrap()
            .join("shared.json")
    };

    fs::write(shared(&a), b"{\"node\":\"base\"}\n").unwrap();
    assert!(service(&a).sync().unwrap().pushed);
    service(&b).sync().unwrap();
    fs::write(shared(&a), b"{\"node\":\"a\"}\n").unwrap();
    assert!(service(&a).sync().unwrap().pushed);
    fs::write(shared(&b), b"{\"node\":\"b\"}\n").unwrap();

    // Block state pushes: the resolution succeeds and records its result ref,
    // but publication fails — the crash window this test exists for.
    let hook = remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        "#!/bin/sh\nwhile read old new ref; do\n  if test \"$ref\" = refs/heads/refine/state; then exit 1; fi\ndone\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();

    {
        let _guard = install_resolver_override(
            &b,
            Arc::new(ScriptedResolver::new(vec![
                Box::new(|request: &ResolutionRequest<'_>| {
                    fs::write(
                        request.workspace_dir.join(".refine/shared.json"),
                        "{\"node\":\"merged\"}\n",
                    )
                    .unwrap();
                    Ok(ResolverOutcome::Completed)
                }) as ScriptedStep,
            ])),
        );
        let interrupted = service(&b).sync();
        assert!(
            interrupted.is_err(),
            "publication was blocked: {interrupted:?}"
        );
    }
    let result_refs = git_out(
        &b,
        &["for-each-ref", "--format=%(refname)", "refs/refine/resolve"],
    )
    .unwrap();
    assert!(
        result_refs.lines().any(|line| line.ends_with("/result")),
        "the gated result survives the interrupted publication as a ref: {result_refs}"
    );

    // The "crash" clears. The rerun must publish the surviving result without
    // asking the resolver anything.
    fs::remove_file(&hook).unwrap();
    let _guard = install_resolver_override(
        &b,
        Arc::new(ScriptedResolver::new(vec![Box::new(
            |_request: &ResolutionRequest<'_>| -> refine::process::supervisor::errors::RefineResult<
                ResolverOutcome,
            > { panic!("a surviving gated result must publish without re-invoking the resolver") },
        ) as ScriptedStep])),
    );
    let published = service(&b).sync().unwrap();
    assert!(published.ok && published.pushed, "{published:?}");
    assert_eq!(
        git_out(&remote, &["show", "refine/state:.refine/shared.json"]).as_deref(),
        Some("{\"node\":\"merged\"}")
    );
    assert_eq!(
        fs::read_to_string(shared(&b)).unwrap(),
        "{\"node\":\"merged\"}\n"
    );
    assert_eq!(
        git_out(&b, &["for-each-ref", "refs/refine/resolve"]).as_deref(),
        Some(""),
        "the bounded resolve namespace is retired after publication"
    );
    assert!(
        latest_state_sync_conflict_report(&b.join("run"))
            .unwrap()
            .is_none(),
        "the settled conflict report is cleared"
    );

    // The other node converges onto the resolved content and reruns are no-ops.
    service(&a).sync().unwrap();
    assert_eq!(
        fs::read_to_string(shared(&a)).unwrap(),
        "{\"node\":\"merged\"}\n"
    );
    let rerun = service(&b).sync().unwrap();
    assert!(!rerun.pushed, "{rerun:?}");
}

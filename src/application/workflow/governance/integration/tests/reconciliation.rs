use super::*;

struct ReconciliationFixture {
    temp_root: PathBuf,
    repo: PathBuf,
    refine_dir: PathBuf,
    runtime_root: PathBuf,
    base_commit: String,
    candidate_commit: String,
    integration_target: String,
    descendant_target: String,
    integration: RoundIntegration,
}

impl ReconciliationFixture {
    fn new() -> Self {
        let temp_root = unique_temp_dir("already-merged-resolution");
        let repo = temp_root.join("repo");
        let runtime_root = temp_root.join("run/8080");
        let worktree = temp_root.join("candidate");
        let remote = temp_root.join("remote.git");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        let refine_dir = prepare_refine_dir(&repo).unwrap();
        commit_file(&repo, "app.txt", "base\n", "base");
        let base_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
        git(
            &temp_root,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        )
        .unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&repo, &["push", "-u", "origin", "main"]).unwrap();
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "refine/GOAL/round-1",
                worktree.to_str().unwrap(),
            ],
        )
        .unwrap();
        commit_file(&worktree, "candidate.txt", "candidate\n", "candidate");
        let candidate_commit = git_stdout(&worktree, &["rev-parse", "HEAD"]);
        git(&repo, &["merge", "--no-ff", "--no-edit", &candidate_commit]).unwrap();
        let integration_target = git_stdout(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["push", "origin", "main"]).unwrap();
        commit_file(&repo, "sibling.txt", "sibling\n", "sibling descendant");
        let descendant_target = git_stdout(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["push", "origin", "main"]).unwrap();
        let integration = RoundIntegration {
            candidate_commit: candidate_commit.clone(),
            target_branch: "main".to_string(),
            target_commit: integration_target.clone(),
            remote: "origin".to_string(),
            pushed: true,
            integrated_at: "2026-08-15T00:00:00Z".to_string(),
            merge: MergeResult {
                ok: true,
                conflicts: Vec::new(),
                message: Some("candidate integration".to_string()),
            },
        };
        Self {
            temp_root,
            repo,
            refine_dir,
            runtime_root,
            base_commit,
            candidate_commit,
            integration_target,
            descendant_target,
            integration,
        }
    }

    fn work_items(&self) -> FileWorkItemService {
        FileWorkItemService::new(&self.refine_dir)
    }

    fn service(&self) -> FileGovernanceIntegrationService {
        FileGovernanceIntegrationService::with_target_root(
            &self.runtime_root,
            &self.refine_dir,
            &self.repo,
        )
    }

    fn create_goal(&self, goal_id: &str, patch: Value, include_integration: bool) {
        let work_items = self.work_items();
        work_items
            .create_goal_summary(goal_id, Some(goal_id))
            .unwrap();
        work_items
            .append_goal_round_summary(goal_id, "Buddy", "Implement")
            .unwrap();
        work_items
            .transition_goal_status(goal_id, GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status(goal_id, GoalStatus::Plan)
            .unwrap();
        work_items
            .update_goal_git_refs(
                goal_id,
                &format!("refine/{goal_id}/round-1"),
                "main",
                &self.base_commit,
                Some(&self.candidate_commit),
            )
            .unwrap();
        work_items
            .advance_automated_goal_status(goal_id, GoalStatus::Implement)
            .unwrap();
        work_items
            .advance_automated_goal_status(goal_id, GoalStatus::Quality)
            .unwrap();
        let mut evidence = json!({
            "quality_state": "passed",
            "quality_candidate_commit": self.candidate_commit,
            "quality_checked_at": "2026-08-15T00:01:00Z",
            "quality_details": {
                "operation_id": "quality-operation-1",
                "candidate_commit": self.candidate_commit,
                "source_candidate_commit": self.candidate_commit,
                "evaluation_scope": "isolated_candidate",
                "results": []
            },
            "rule_state": "passed",
            "meta_rule_state": "passed",
            "product_state": "passed",
            "constitution_state": "passed",
            "governance_candidate_commit": self.candidate_commit,
            "governance_checked_at": "2026-08-15T00:02:00Z"
        });
        if include_integration {
            evidence["workflow_integration"] = json!(self.integration);
        }
        for (key, value) in patch.as_object().cloned().unwrap_or_default() {
            evidence[key] = value;
        }
        work_items
            .update_goal_round_evaluation_summary(goal_id, 0, &evidence)
            .unwrap();
        let (round_idx, revision, request) = work_items.authored_goal_commitment(goal_id).unwrap();
        work_items
            .claim_workflow_attempt(goal_id, GoalStatus::Quality, round_idx, revision, &request)
            .unwrap();
    }
}

impl Drop for ReconciliationFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_root);
    }
}

/// Rewind local and published `main` to the recorded integration merge, the
/// exact snapshot an already-merged revert expects.
fn park_target_at_integration(fixture: &ReconciliationFixture) {
    git(
        &fixture.repo,
        &["reset", "--hard", &fixture.integration_target],
    )
    .unwrap();
    git(&fixture.repo, &["push", "--force", "origin", "main"]).unwrap();
}

fn install_hook(path: &Path, script: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn revert(
    fixture: &ReconciliationFixture,
    goal_id: &str,
    expected_target_commit: &str,
) -> RefineResult<ReconciliationRevert> {
    fixture
        .service()
        .revert_reconciled_candidate_and_settle(
            ReconciliationRequest {
                goal_id,
                round_idx: 0,
                node_id: "default",
                integration: &fixture.integration,
                expected_target_commit,
            },
            |_| Ok(()),
        )
        .map(|(reverted, ())| reverted)
}

#[test]
fn already_merged_resolution_allows_shared_target_descendants_and_is_concurrently_idempotent() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-RESOLVE", json!({}), true);
    let first = fixture.service();
    let second = fixture.service();
    let (first, second) = thread::scope(|scope| {
        let first = scope.spawn(|| first.resolve_already_merged_goal("GOAL-RESOLVE"));
        let second = scope.spawn(|| second.resolve_already_merged_goal("GOAL-RESOLVE"));
        (
            first.join().unwrap().unwrap(),
            second.join().unwrap().unwrap(),
        )
    });
    let dispositions = [first.disposition, second.disposition];
    assert!(dispositions.contains(&AlreadyMergedResolutionDisposition::Resolved));
    assert!(
        dispositions.contains(&AlreadyMergedResolutionDisposition::AlreadyResolved),
        "{dispositions:?}"
    );
    let repeated = fixture
        .service()
        .resolve_already_merged_goal("GOAL-RESOLVE")
        .unwrap();
    assert_eq!(
        repeated.disposition,
        AlreadyMergedResolutionDisposition::AlreadyResolved
    );
    assert_eq!(repeated.goal.goal.status, GoalStatus::Review);
    assert_eq!(
        repeated.evidence["integration_target_commit"],
        fixture.integration_target
    );
    assert_eq!(
        repeated.evidence["local_target_commit"],
        fixture.descendant_target
    );
    assert_eq!(
        repeated.evidence["published_target_commit"],
        fixture.descendant_target
    );
    assert_eq!(repeated.evidence["quality_proof_mode"], "normalized");
}

#[test]
fn candidate_reachability_without_current_round_integration_stays_on_isolated_quality_path() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-SIBLING", json!({}), false);
    let error = fixture
        .service()
        .resolve_already_merged_goal("GOAL-SIBLING")
        .unwrap_err();
    assert!(
        error.to_string().contains("candidate ancestry alone"),
        "{error}"
    );
    assert_eq!(
        fixture
            .work_items()
            .show_goal_summary("GOAL-SIBLING")
            .unwrap()
            .goal
            .status,
        GoalStatus::Quality
    );
}

#[test]
fn missing_quality_proof_is_regenerated_for_the_exact_candidate() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal(
        "GOAL-QUALITY-MISMATCH",
        json!({"quality_candidate_commit": "wrong"}),
        true,
    );
    let resolution = fixture
        .service()
        .resolve_already_merged_goal("GOAL-QUALITY-MISMATCH")
        .unwrap();
    assert_eq!(
        resolution.disposition,
        AlreadyMergedResolutionDisposition::Resolved
    );
    assert_eq!(resolution.goal.goal.status, GoalStatus::Review);
    assert_eq!(resolution.evidence["quality_proof_mode"], "regenerated");
    assert_eq!(
        resolution.evidence["quality_proof"]["checked_candidate_commit"],
        fixture.candidate_commit
    );
    assert_eq!(
        resolution.evidence["quality_checkout"]["candidate_commit"],
        fixture.candidate_commit
    );
}

#[test]
fn mismatched_nested_quality_proof_is_regenerated_instead_of_accepted() {
    let fixture = ReconciliationFixture::new();
    let goal_id = "GOAL-QUALITY-PROOF-MISMATCH";
    fixture.create_goal(
        goal_id,
        json!({
            "quality_details": {
                "operation_id": "quality-operation-1",
                "candidate_commit": fixture.candidate_commit,
                "source_candidate_commit": fixture.candidate_commit,
                "evaluation_scope": "isolated_candidate",
                "results": [],
                "quality_proof": {
                    "schema_version": 1,
                    "goal_id": goal_id,
                    "round_idx": 0,
                    "evaluation_scope": "isolated_candidate",
                    "operation_id": "different-operation",
                    "checked_candidate_commit": fixture.candidate_commit,
                    "source_candidate_commit": fixture.candidate_commit,
                    "state": "passed",
                    "checked_at": "2026-08-15T00:01:00Z",
                    "results": []
                }
            }
        }),
        true,
    );

    let resolution = fixture
        .service()
        .resolve_already_merged_goal(goal_id)
        .unwrap();
    assert_eq!(
        resolution.disposition,
        AlreadyMergedResolutionDisposition::Resolved
    );
    assert_eq!(resolution.evidence["quality_proof_mode"], "regenerated");
    assert_ne!(
        resolution.evidence["quality_proof"]["operation_id"],
        "different-operation"
    );
}

#[test]
fn quality_regeneration_rejects_a_mismatched_managed_checkout() {
    let fixture = ReconciliationFixture::new();
    let goal_id = "GOAL-QUALITY-CHECKOUT-MISMATCH";
    fixture.create_goal(goal_id, json!({"quality_candidate_commit": "wrong"}), true);
    let short_candidate = fixture
        .candidate_commit
        .chars()
        .take(12)
        .collect::<String>();
    let reconciliation_branch =
        format!("refine/reconciliation/{goal_id}/round-1/{short_candidate}");
    git(
        &fixture.repo,
        &["branch", &reconciliation_branch, &fixture.base_commit],
    )
    .unwrap();

    let resolution = fixture
        .service()
        .resolve_already_merged_goal(goal_id)
        .unwrap();
    assert_eq!(
        resolution.disposition,
        AlreadyMergedResolutionDisposition::Failed
    );
    assert_eq!(resolution.goal.goal.status, GoalStatus::Failed);
    assert!(
        resolution.evidence["error"]
            .as_str()
            .unwrap()
            .contains("the branch was not moved")
    );
    assert!(resolution.evidence["quality_proof"].is_null());
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", &reconciliation_branch]),
        fixture.base_commit
    );
}

#[test]
fn mismatched_or_missing_non_quality_gate_evidence_settles_failed_once() {
    let fixture = ReconciliationFixture::new();
    for (goal_id, patch) in [
        (
            "GOAL-GOVERNANCE-MISSING",
            json!({"governance_candidate_commit": ""}),
        ),
        (
            "GOAL-INTEGRATION-MISMATCH",
            json!({"workflow_integration": {
                "candidate_commit": fixture.base_commit,
                "target_branch": "main",
                "target_commit": fixture.integration_target,
                "remote": "origin",
                "pushed": true,
                "integrated_at": "2026-08-15T00:00:00Z",
                "merge": {"ok": true, "conflicts": [], "message": "wrong candidate"}
            }}),
        ),
        (
            "GOAL-INTEGRATION-UNSUCCESSFUL",
            json!({"workflow_integration": {
                "candidate_commit": fixture.candidate_commit,
                "target_branch": "main",
                "target_commit": fixture.integration_target,
                "remote": "origin",
                "pushed": true,
                "integrated_at": "2026-08-15T00:00:00Z",
                "merge": {"ok": false, "conflicts": ["app.txt"], "message": "not integrated"}
            }}),
        ),
    ] {
        fixture.create_goal(goal_id, patch, true);
        let first = fixture
            .service()
            .resolve_already_merged_goal(goal_id)
            .unwrap();
        assert_eq!(
            first.disposition,
            AlreadyMergedResolutionDisposition::Failed
        );
        assert_eq!(first.goal.goal.status, GoalStatus::Failed);
        assert!(
            first.evidence["error"]
                .as_str()
                .is_some_and(|v| !v.is_empty())
        );
        let repeated = fixture
            .service()
            .resolve_already_merged_goal(goal_id)
            .unwrap();
        assert_eq!(
            repeated.disposition,
            AlreadyMergedResolutionDisposition::AlreadyFailed
        );
    }
}

#[test]
fn missing_local_ancestry_fails_closed_without_reverting_shared_history() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-GIT-UNSAFE", json!({}), true);
    git(&fixture.repo, &["reset", "--hard", &fixture.base_commit]).unwrap();
    let before = git_stdout(&fixture.repo, &["rev-parse", "HEAD"]);
    let resolution = fixture
        .service()
        .resolve_already_merged_goal("GOAL-GIT-UNSAFE")
        .unwrap();
    assert_eq!(
        resolution.disposition,
        AlreadyMergedResolutionDisposition::Failed
    );
    assert!(
        resolution.evidence["error"]
            .as_str()
            .unwrap()
            .contains("no longer contains")
    );
    assert_eq!(git_stdout(&fixture.repo, &["rev-parse", "HEAD"]), before);
}

#[test]
fn missing_published_ancestry_records_both_snapshots_and_fails_closed() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-PUBLISHED-UNSAFE", json!({}), true);
    git(
        &fixture.repo,
        &[
            "push",
            "--force",
            "origin",
            &format!("{}:main", fixture.base_commit),
        ],
    )
    .unwrap();
    let resolution = fixture
        .service()
        .resolve_already_merged_goal("GOAL-PUBLISHED-UNSAFE")
        .unwrap();
    assert_eq!(
        resolution.disposition,
        AlreadyMergedResolutionDisposition::Failed
    );
    assert_eq!(
        resolution.evidence["local_target_commit"],
        fixture.descendant_target
    );
    assert_eq!(
        resolution.evidence["published_target_commit"],
        fixture.base_commit
    );
    assert!(
        resolution.evidence["error"]
            .as_str()
            .unwrap()
            .contains("Published target")
    );
}

#[test]
fn revert_runs_in_the_integration_worktree_and_syncs_the_human_checkout() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-REVERT-OK", json!({}), true);
    park_target_at_integration(&fixture);

    let reverted = revert(&fixture, "GOAL-REVERT-OK", &fixture.integration_target).unwrap();

    assert_eq!(reverted.merge_commit, fixture.integration_target);
    assert!(reverted.result.ok);
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        reverted.revert_commit
    );
    assert!(
        git_stdout(&fixture.repo, &["ls-remote", "origin", "refs/heads/main"])
            .starts_with(&reverted.revert_commit),
        "the revert must be published"
    );
    // The human checkout was synced in place: still on main, clean, with the
    // candidate delta unwound and no revert porcelain state.
    assert_eq!(
        git_stdout(&fixture.repo, &["branch", "--show-current"]),
        "main"
    );
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "HEAD"]),
        reverted.revert_commit
    );
    assert!(!fixture.repo.join("candidate.txt").exists());
    assert!(git_stdout(&fixture.repo, &["status", "--porcelain"]).is_empty());
    assert!(!git_succeeds(
        &fixture.repo,
        &["rev-parse", "--verify", "REVERT_HEAD"]
    ));
    // The porcelain ran in the detached integration worktree.
    let worktree = fixture.repo.join(".git/refine-integration/target");
    assert!(worktree.exists());
    assert_eq!(
        git_stdout(&worktree, &["rev-parse", "HEAD"]),
        reverted.revert_commit
    );
    assert_eq!(git_stdout(&worktree, &["branch", "--show-current"]), "");
}

#[test]
fn revert_succeeds_with_the_human_checkout_parked_on_another_branch() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-REVERT-PARKED", json!({}), true);
    park_target_at_integration(&fixture);
    git(&fixture.repo, &["switch", "-c", "parked"]).unwrap();

    // The revert no longer asserts checkout position: the ref moves by CAS
    // while the parked checkout stays byte-identical.
    let reverted = revert(&fixture, "GOAL-REVERT-PARKED", &fixture.integration_target).unwrap();

    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        reverted.revert_commit
    );
    assert_eq!(
        git_stdout(&fixture.repo, &["branch", "--show-current"]),
        "parked"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("candidate.txt")).unwrap(),
        "candidate\n"
    );
    assert!(git_stdout(&fixture.repo, &["status", "--porcelain"]).is_empty());
}

#[test]
fn conflicted_revert_recovers_the_integration_worktree_and_moves_nothing() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-REVERT-CONFLICT", json!({}), true);
    park_target_at_integration(&fixture);
    // A later commit rewrote the candidate's file, so unwinding the merge
    // conflicts instead of applying cleanly.
    commit_file(
        &fixture.repo,
        "candidate.txt",
        "mutated after integration\n",
        "mutate candidate",
    );
    git(&fixture.repo, &["push", "origin", "main"]).unwrap();
    let expected = git_stdout(&fixture.repo, &["rev-parse", "main"]);

    let error = revert(&fixture, "GOAL-REVERT-CONFLICT", &expected).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("already-merged reconciliation revert failed"),
        "{error}"
    );
    assert_eq!(git_stdout(&fixture.repo, &["rev-parse", "main"]), expected);
    assert_eq!(
        fs::read_to_string(fixture.repo.join("candidate.txt")).unwrap(),
        "mutated after integration\n"
    );
    let worktree = fixture.repo.join(".git/refine-integration/target");
    assert!(
        git_stdout(&worktree, &["status", "--porcelain"]).is_empty(),
        "the integration worktree must be recovered after a revert conflict"
    );
    assert!(!git_succeeds(
        &worktree,
        &["rev-parse", "--verify", "REVERT_HEAD"]
    ));
}

#[test]
fn revert_push_failure_rolls_the_target_back_by_counter_cas() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-REVERT-PUSH", json!({}), true);
    park_target_at_integration(&fixture);
    install_hook(
        &fixture.temp_root.join("remote.git/hooks/pre-receive"),
        "#!/bin/sh\nexit 1\n",
    );

    let error = revert(&fixture, "GOAL-REVERT-PUSH", &fixture.integration_target).unwrap_err();

    assert!(error.to_string().contains("could not be pushed"), "{error}");
    assert!(
        error
            .to_string()
            .contains("the clean local target was restored to"),
        "{error}"
    );
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        fixture.integration_target,
        "the counter-CAS must restore the pre-revert target"
    );
    // The checkout sync was mirrored back too: the candidate delta returned.
    assert_eq!(
        fs::read_to_string(fixture.repo.join("candidate.txt")).unwrap(),
        "candidate\n"
    );
    assert!(git_stdout(&fixture.repo, &["status", "--porcelain"]).is_empty());
    assert!(
        git_stdout(&fixture.repo, &["ls-remote", "origin", "refs/heads/main"])
            .starts_with(&fixture.integration_target)
    );
}

#[test]
fn revert_push_failure_with_an_externally_moved_ref_reports_the_unrestored_leg() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-REVERT-PUSH-MOVED", json!({}), true);
    park_target_at_integration(&fixture);
    // The hook both fails the push and yanks the ref elsewhere, so the
    // rollback counter-CAS must lose and say so.
    install_hook(
        &fixture.repo.join(".git/hooks/pre-push"),
        &format!(
            "#!/bin/sh\ngit update-ref refs/heads/main {}\nexit 1\n",
            fixture.base_commit
        ),
    );

    let error = revert(
        &fixture,
        "GOAL-REVERT-PUSH-MOVED",
        &fixture.integration_target,
    )
    .unwrap_err();

    assert!(error.to_string().contains("could not be pushed"), "{error}");
    assert!(
        error.to_string().contains("could not be restored to"),
        "{error}"
    );
    assert_eq!(
        git_stdout(&fixture.repo, &["rev-parse", "main"]),
        fixture.base_commit,
        "a lost rollback counter-CAS must leave the externally moved ref alone"
    );
}

#[test]
fn destructive_revert_retains_exact_target_snapshot_fence() {
    let fixture = ReconciliationFixture::new();
    fixture.create_goal("GOAL-REVERT-FENCE", json!({}), true);
    let before = git_stdout(&fixture.repo, &["rev-parse", "HEAD"]);
    let error = fixture
        .service()
        .revert_reconciled_candidate_and_settle(
            ReconciliationRequest {
                goal_id: "GOAL-REVERT-FENCE",
                round_idx: 0,
                node_id: "default",
                integration: &fixture.integration,
                expected_target_commit: &fixture.integration_target,
            },
            |_| Ok(()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("target changed"), "{error}");
    assert_eq!(git_stdout(&fixture.repo, &["rev-parse", "HEAD"]), before);
}

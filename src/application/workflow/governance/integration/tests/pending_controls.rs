use super::*;
use crate::application::work_items::WorkflowControl;
use crate::application::workflow::WorkflowEngine;

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    work: FileWorkItemService,
    service: FileGovernanceIntegrationService,
    request: WorkflowControl,
    candidate: String,
}

impl Fixture {
    fn new() -> Self {
        let root = unique_temp_dir("pending-integration-control");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_repo(&repo);
        commit_file(&repo, "base.txt", "base\n", "base");
        let base = git_stdout(&repo, &["rev-parse", "HEAD"]);
        let remote = root.join("remote.git");
        git(
            &root,
            &["init", "--bare", "-b", "main", remote.to_str().unwrap()],
        )
        .unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&repo, &["push", "-u", "origin", "main"]).unwrap();
        let branch = "refine/PENDING/round-1";
        let checkout = root.join("candidate");
        git(
            &repo,
            &["worktree", "add", "-b", branch, checkout.to_str().unwrap()],
        )
        .unwrap();
        commit_file(&checkout, "feature.txt", "candidate\n", "candidate");
        let candidate = git_stdout(&checkout, &["rev-parse", "HEAD"]);
        git(&checkout, &["push", "-u", "origin", branch]).unwrap();
        let state = prepare_refine_dir(&repo).unwrap();
        let work = FileWorkItemService::new(&state);
        work.create_goal_summary("Pending integration", Some("PENDING"))
            .unwrap();
        work.append_goal_round_summary("PENDING", "operator", "Implement the request")
            .unwrap();
        work.update_goal_git_refs("PENDING", branch, "main", &base, Some(&candidate))
            .unwrap();
        work.update_goal_round_evaluation_summary(
            "PENDING",
            0,
            &json!({"workflow_git_remote":"origin"}),
        )
        .unwrap();
        let request = WorkflowControl {
            to: GoalStatus::Governance,
            reason: "Integrate this candidate".into(),
            context: String::new(),
            expected_revision: work.show_goal_detail("PENDING").unwrap()["workflow_revision"]
                .as_u64()
                .unwrap(),
            request_id: "integration-request".into(),
            force: true,
            actor: "operator".into(),
            invocation_id: None,
        };
        let service =
            FileGovernanceIntegrationService::with_target_root(root.join("runtime"), &state, &repo);
        Self {
            root,
            repo,
            work,
            service,
            request,
            candidate,
        }
    }

    fn accept(&self) -> Value {
        self.work
            .control_workflow_operation("PENDING", &self.request, true)
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn daemon_resumes_a_request_lost_after_acceptance_and_duplicate_does_not_merge_again() {
    let f = Fixture::new();
    let before = git_stdout(&f.repo, &["rev-parse", "main"]);
    f.accept();
    assert_eq!(git_stdout(&f.repo, &["rev-parse", "main"]), before);
    // Drop the request continuation. A fresh ordinary engine discovers and runs
    // the pending operation through its existing Goal scheduling path.
    let engine = WorkflowEngine::with_target_root(&f.service.runtime_root, &f.repo);
    engine
        .recover_interrupted_goals("restart after accepted integration")
        .unwrap();
    let pass = engine.evaluate_workflow().unwrap();
    assert_eq!(pass.steps.len(), 1);
    assert_eq!(pass.steps[0].final_status, "review");
    let after = f.work.show_goal_detail("PENDING").unwrap();
    assert_eq!(after["workflow_integration_control"]["state"], "succeeded");
    let integrated = git_stdout(&f.repo, &["rev-parse", "main"]);
    assert_ne!(integrated, before);
    let repeated = f.service.force_integrate("PENDING", &f.request).unwrap();
    assert_eq!(repeated["decision"]["integration_performed"], true);
    assert_eq!(git_stdout(&f.repo, &["rev-parse", "main"]), integrated);
}

#[test]
fn redirect_before_recovery_does_not_publish_the_superseded_request() {
    let f = Fixture::new();
    f.accept();
    f.work.cancel_goal_summary("PENDING").unwrap();
    let before = git_stdout(&f.repo, &["rev-parse", "main"]);
    assert!(
        f.service
            .resume_pending_control("PENDING")
            .unwrap()
            .is_none()
    );
    f.service.force_integrate("PENDING", &f.request).unwrap();
    assert_eq!(git_stdout(&f.repo, &["rev-parse", "main"]), before);
    assert_eq!(
        f.work.show_goal_detail("PENDING").unwrap()["status"],
        "cancelled"
    );
}

#[test]
fn pending_completion_reuses_an_integration_that_already_happened() {
    let f = Fixture::new();
    let decision = f.accept();
    f.service
        .clone()
        .for_occurrence(decision["generation"].as_u64().unwrap())
        .integrate_workflow_candidate(
            "PENDING",
            0,
            "default",
            "refine/PENDING/round-1",
            &f.candidate,
            "origin",
        )
        .unwrap();
    let integrated = git_stdout(&f.repo, &["rev-parse", "main"]);
    assert_eq!(
        f.work.show_goal_detail("PENDING").unwrap()["workflow_integration_control"]["state"],
        "pending"
    );
    let engine = WorkflowEngine::with_target_root(&f.service.runtime_root, &f.repo);
    assert_eq!(
        engine.evaluate_workflow().unwrap().steps[0].final_status,
        "review"
    );
    assert_eq!(git_stdout(&f.repo, &["rev-parse", "main"]), integrated);
    assert_eq!(
        f.work.show_goal_detail("PENDING").unwrap()["integration_history"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn changed_inputs_fail_the_pending_request_without_integrating_replacement_work() {
    let f = Fixture::new();
    f.accept();
    let base = git_stdout(&f.repo, &["rev-parse", "main"]);
    f.work
        .update_goal_git_refs(
            "PENDING",
            "refine/PENDING/round-1",
            "main",
            &base,
            Some(&base),
        )
        .unwrap();
    assert!(f.service.resume_pending_control("PENDING").is_err());
    assert_eq!(git_stdout(&f.repo, &["rev-parse", "main"]), base);
    assert_eq!(
        f.work.show_goal_detail("PENDING").unwrap()["workflow_integration_control"]["state"],
        "failed"
    );
}

#[test]
fn redirect_after_preparation_prevents_the_target_compare_and_swap() {
    use super::super::publication_test_hooks::{Boundary, during};
    let f = Fixture::new();
    let before = git_stdout(&f.repo, &["rev-parse", "main"]);
    let work = f.work.clone();
    let repo = f.repo.clone();
    let candidate = f.candidate.clone();
    let result = during(
        Boundary::TargetCas,
        move || {
            // Fetch and the private merge really completed before the request
            // changed. Neither is permission to start target publication.
            assert_eq!(
                git_stdout(&repo, &["rev-parse", "origin/refine/PENDING/round-1"]),
                candidate
            );
            assert_ne!(
                git_stdout(
                    &repo.join(".git/refine-integration/target"),
                    &["rev-parse", "HEAD"]
                ),
                git_stdout(&repo, &["rev-parse", "main"])
            );
            work.cancel_goal_summary("PENDING").unwrap();
        },
        || f.service.force_integrate("PENDING", &f.request),
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("superseded workflow occurrence")
    );
    assert_eq!(git_stdout(&f.repo, &["rev-parse", "main"]), before);
    assert_eq!(
        git_stdout(&f.root.join("remote.git"), &["rev-parse", "main"]),
        before
    );
    let after = f.work.show_goal_detail("PENDING").unwrap();
    assert_eq!(after["status"], "cancelled");
    assert!(after["rounds"][0]["workflow_integration"].is_null());
    assert!(after["integration_history"].is_null());
}

#[test]
fn redirect_after_local_integration_retains_history_and_prevents_push() {
    use super::super::publication_test_hooks::{Boundary, during};
    let f = Fixture::new();
    let before = git_stdout(&f.repo, &["rev-parse", "main"]);
    let work = f.work.clone();
    let result = during(
        Boundary::Push,
        move || {
            let partial = work.show_goal_detail("PENDING").unwrap();
            assert_eq!(partial["integration_history"][0]["complete"], false);
            assert!(partial["rounds"][0]["workflow_integration"].is_null());
            work.cancel_goal_summary("PENDING").unwrap();
        },
        || f.service.force_integrate("PENDING", &f.request),
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("superseded workflow occurrence")
    );
    let integrated = git_stdout(&f.repo, &["rev-parse", "main"]);
    assert_ne!(integrated, before);
    assert!(git_succeeds(
        &f.repo,
        &["merge-base", "--is-ancestor", &f.candidate, &integrated]
    ));
    assert_eq!(
        git_stdout(&f.root.join("remote.git"), &["rev-parse", "main"]),
        before
    );
    let after = f.work.show_goal_detail("PENDING").unwrap();
    assert_eq!(after["status"], "cancelled");
    assert_eq!(after["workflow_integration_control"]["state"], "superseded");
    assert!(after["rounds"][0]["workflow_integration"].is_null());
    assert_eq!(
        after["integration_history"][0]["integration"]["target_commit"],
        integrated
    );
    assert_eq!(
        after["integration_history"][0]["integration"]["pushed"],
        false
    );
    assert_eq!(after["integration_history"][0]["complete"], false);
}

#[test]
fn actual_integration_is_retained_after_node_transfer_without_settling_current_work() {
    let f = Fixture::new();
    let decision = f.accept();
    let result = f
        .service
        .clone()
        .for_occurrence(decision["generation"].as_u64().unwrap())
        .integrate_workflow_candidate(
            "PENDING",
            0,
            "default",
            "refine/PENDING/round-1",
            &f.candidate,
            "origin",
        )
        .unwrap();
    // The side effect completed, but its workflow completion was interrupted.
    f.work.cancel_goal_summary("PENDING").unwrap();
    crate::application::fleet::nodes::FileNodeRegistryService::new(&f.work.refine_dir)
        .create("replacement")
        .unwrap();
    f.work
        .transfer_goal_to_node("replacement", "PENDING")
        .unwrap();
    let before = f.work.show_goal_detail("PENDING").unwrap();
    let old_node = FileWorkItemService::for_node(&f.work.refine_dir, "default");
    old_node
        .record_actual_integration(
            "PENDING",
            0,
            decision["generation"].as_u64().unwrap(),
            &result,
        )
        .unwrap();
    old_node
        .finish_controlled_integration("PENDING", &f.request.request_id, &Ok(json!(result)))
        .unwrap();
    let after = f.work.show_goal_detail("PENDING").unwrap();
    assert_eq!(after["status"], before["status"]);
    assert_eq!(after["node_id"], "replacement");
    assert_eq!(after["event_generation"], before["event_generation"]);
    assert_eq!(after["workflow_integration_control"]["state"], "superseded");
    assert_eq!(after["integration_history"].as_array().unwrap().len(), 1);
    assert_eq!(after["workflow_controls"][0]["integration_performed"], true);
}

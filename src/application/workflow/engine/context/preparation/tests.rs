use super::*;
use crate::application::work_items::FileWorkItemService;
use crate::application::workflow::engine::context::execution::hydrate_plan_or_implement_context;
use std::fs;
use std::path::{Path, PathBuf};

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
    runtime: PathBuf,
    items: FileWorkItemService,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("refine-prepared-checkout-{}", uuid::Uuid::new_v4()));
        let repository = root.join("repository");
        let runtime = root.join("runtime");
        fs::create_dir_all(&repository).unwrap();
        for arguments in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["commit", "--allow-empty", "-m", "base"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(arguments)
                    .current_dir(&repository)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let items = FileWorkItemService::new(
            crate::infrastructure::storage::project_layout::prepare_refine_dir(&repository)
                .unwrap(),
        );
        items
            .create_goal_summary("Recover checkout", Some("GOAL1"))
            .unwrap();
        items
            .append_goal_round_summary("GOAL1", "Reporter", "Reproduce this request")
            .unwrap();
        items
            .set_goal_status_unchecked("GOAL1", &GoalStatus::Plan)
            .unwrap();
        Self {
            root,
            repository,
            runtime,
            items,
        }
    }

    fn context(&self, status: GoalStatus) -> WorkflowContext<'_> {
        let (round, revision, prompt) = self.items.authored_goal_commitment("GOAL1").unwrap();
        let authority = self
            .items
            .claim_workflow_attempt("GOAL1", status, round, revision, &prompt)
            .unwrap();
        WorkflowContext::new(
            &self.runtime,
            &self.repository,
            "GOAL1".into(),
            "default".into(),
            "smoke-ai".into(),
            round,
            authority,
            Default::default(),
            self.items.clone(),
        )
    }

    fn commit_file(&self, directory: &Path, content: &str) -> String {
        fs::write(directory.join("generated.txt"), content).unwrap();
        for arguments in [
            vec!["add", "generated.txt"],
            vec!["commit", "-m", "generated work"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(arguments)
                    .current_dir(directory)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        FileGitWorktreeService::new(directory)
            .resolve_commit("HEAD")
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn generated_checkout_corruption_restarts_but_valid_dirty_work_is_reused() {
    for damaged in [false, true] {
        let fixture = Fixture::new();
        let mut initial = fixture.context(GoalStatus::Plan);
        hydrate_plan_or_implement_context(&mut initial, "refine/{goal_id}", "main").unwrap();
        let branch = initial.branch.clone().unwrap();
        let worktree = PathBuf::from(initial.worktree_path.clone().unwrap());
        let git = FileGitWorktreeService::new(&fixture.repository);
        let original_tip = git.resolve_commit(&branch).unwrap();
        let partial = worktree.join("partial.txt");
        fs::write(&partial, "unfinished generated work").unwrap();
        fixture
            .items
            .set_goal_status_unchecked("GOAL1", &GoalStatus::Implement)
            .unwrap();
        if damaged {
            fs::remove_file(worktree.join(".git")).unwrap();
        }

        let mut resumed = fixture.context(GoalStatus::Implement);
        hydrate_plan_or_implement_context(&mut resumed, "refine/{goal_id}", "main").unwrap();
        let goal = fixture.items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(
            fs::read_to_string(&partial).unwrap(),
            "unfinished generated work"
        );
        assert_eq!(git.resolve_commit(&branch).unwrap(), original_tip);
        assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
        assert_eq!(goal["rounds"][0]["prompt"], "Reproduce this request");
        if damaged {
            assert_eq!(goal["status"], "plan");
            assert_ne!(resumed.branch.as_deref(), Some(branch.as_str()));
            assert_ne!(
                Path::new(resumed.worktree_path.as_deref().unwrap()),
                worktree
            );
            assert!(
                Path::new(resumed.worktree_path.as_deref().unwrap())
                    .join(".git")
                    .is_file()
            );
            assert_eq!(
                goal["rounds"][0]["workspace_recoveries"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            let receipt = goal["workflow_controls"]
                .as_array()
                .unwrap()
                .last()
                .unwrap();
            assert_eq!(receipt["source_round"], 1);
            assert_eq!(
                receipt["request"]["reason"],
                goal["rounds"][0]["workspace_recoveries"][0]["reason"]
            );
        } else {
            assert_eq!(goal["status"], "implement");
            assert_eq!(resumed.branch.as_deref(), Some(branch.as_str()));
            assert_eq!(
                Path::new(resumed.worktree_path.as_deref().unwrap()),
                worktree
            );
            assert!(goal["rounds"][0]["workspace_recoveries"].is_null());
        }
    }
}

#[test]
fn invalid_generated_branch_aliases_regenerate_but_invalid_configuration_remains_visible() {
    for (field, invalid_configuration) in [
        ("workspace_branch", false),
        ("branch_name", false),
        ("workspace_branch", true),
    ] {
        let fixture = Fixture::new();
        let mut initial = fixture.context(GoalStatus::Plan);
        hydrate_plan_or_implement_context(&mut initial, "refine/{goal_id}", "main").unwrap();
        let branch = initial.branch.clone().unwrap();
        let worktree = Path::new(initial.worktree_path.as_deref().unwrap());
        fs::write(worktree.join("partial.txt"), "retained partial work").unwrap();
        fixture
            .items
            .set_goal_status_unchecked("GOAL1", &GoalStatus::Implement)
            .unwrap();
        let summary = fixture.items.show_goal_summary("GOAL1").unwrap();
        let path = fixture.items.refine_dir.join(&summary.goal.json_path);
        let mut damaged: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        if field == "workspace_branch" {
            damaged["rounds"][0][field] = json!("invalid branch..");
        } else {
            damaged[field] = json!("invalid branch..");
        }
        fs::write(&path, serde_json::to_vec_pretty(&damaged).unwrap()).unwrap();

        let mut resumed = fixture.context(GoalStatus::Implement);
        if invalid_configuration {
            resumed.settings.insert(
                "branch_name_pattern".into(),
                json!("invalid config/{goal_id}"),
            );
        }
        let before = fs::read(&path).unwrap();
        let prepared = prepare_workspace(&mut resumed, GoalStatus::Implement, "main");
        if invalid_configuration {
            assert!(matches!(prepared, Err(RefineError::InvalidInput(_))));
            assert_eq!(fs::read(&path).unwrap(), before);
        } else {
            let prepared = prepared.unwrap();
            assert_eq!(prepared.status, GoalStatus::Plan);
            assert_ne!(prepared.branch, branch);
            validate_branch_name(&prepared.branch).unwrap();
            let goal = fixture.items.show_goal_detail("GOAL1").unwrap();
            assert_eq!(goal["rounds"].as_array().unwrap().len(), 1);
            assert_eq!(goal["rounds"][0]["prompt"], "Reproduce this request");
            assert_eq!(
                goal["rounds"][0]["workspace_recoveries"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
        }
        assert_eq!(
            fs::read_to_string(worktree.join("partial.txt")).unwrap(),
            "retained partial work"
        );
        let git = FileGitWorktreeService::new(&fixture.repository);
        assert_eq!(
            git.resolve_commit(&branch).unwrap(),
            git.resolve_commit("main").unwrap()
        );
    }
}

#[cfg(unix)]
#[test]
fn workspace_probe_propagates_filesystem_failures_without_selecting_replacement() {
    let fixture = Fixture::new();
    let mut initial = fixture.context(GoalStatus::Plan);
    hydrate_plan_or_implement_context(&mut initial, "refine/{goal_id}", "main").unwrap();
    let before = fixture.items.show_goal_detail("GOAL1").unwrap();
    let worktree = Path::new(initial.worktree_path.as_deref().unwrap());
    let loop_path = fixture.root.join("registration-loop");
    std::os::unix::fs::symlink(&loop_path, &loop_path).unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", loop_path.display()),
    )
    .unwrap();
    let git = FileGitWorktreeService::new(&fixture.repository);
    assert!(matches!(
        git.worktree_requires_reconstruction(initial.branch.as_deref().unwrap(), false),
        Err(RefineError::Io(_))
    ));
    assert_eq!(fixture.items.show_goal_detail("GOAL1").unwrap(), before);
}

#[test]
fn malformed_or_mismatched_integration_receipt_regenerates_and_retains_evidence() {
    for (status, kind) in [GoalStatus::Todo, GoalStatus::Governance]
        .into_iter()
        .flat_map(|status| ["malformed", "candidate", "target"].map(|kind| (status.clone(), kind)))
    {
        let fixture = Fixture::new();
        let mut initial = fixture.context(GoalStatus::Plan);
        hydrate_plan_or_implement_context(&mut initial, "refine/{goal_id}", "main").unwrap();
        let branch = initial.branch.clone().unwrap();
        let git = FileGitWorktreeService::new(&fixture.repository);
        let main = git.resolve_commit("main").unwrap();
        fixture
            .items
            .update_goal_git_refs("GOAL1", &branch, "main", &main, Some(&main))
            .unwrap();
        fixture
            .items
            .set_goal_status_unchecked("GOAL1", &status)
            .unwrap();
        let receipt = if kind == "malformed" {
            json!({"candidate_commit":main})
        } else {
            json!({"candidate_commit":if kind=="candidate" { "another-candidate" } else { &main },
                "target_branch":if kind=="target" {"another-target"}else{"main"},
                "target_commit":main,"remote":"origin","pushed":false,
                "integrated_at":"2026-09-13T00:00:00Z","merge":{"ok":true,"conflicts":[],"message":null}})
        };
        fixture
            .items
            .update_goal_round_evaluation_summary(
                "GOAL1",
                0,
                &json!({"workflow_integration":receipt}),
            )
            .unwrap();
        let mut resumed = fixture.context(status.clone());
        if status == GoalStatus::Todo {
            use crate::application::workflow::engine::behaviors::contract::WorkflowBehavior;
            crate::application::workflow::engine::behaviors::WorkflowTodo
                .advance(&mut resumed)
                .unwrap();
        } else {
            crate::application::workflow::engine::context::execution::hydrate_retry_context(
                &mut resumed,
                status,
            )
            .unwrap();
        }
        let goal = fixture.items.show_goal_detail("GOAL1").unwrap();
        assert_eq!(resumed.start_status, GoalStatus::Plan);
        assert_ne!(resumed.branch.as_deref(), Some(branch.as_str()));
        assert_eq!(goal["rounds"][0]["prompt"], "Reproduce this request");
        assert!(goal["rounds"][0]["workflow_integration"].is_null());
        assert!(
            goal["rounds"][0]["prior_attempts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|attempt| attempt["workflow_integration"] == receipt)
        );
        assert_eq!(git.resolve_commit("main").unwrap(), main);
        assert_eq!(git.resolve_commit(&branch).unwrap(), main);
    }
}

#[test]
fn ahead_branch_is_preserved_for_implementation_but_requires_fresh_quality_and_governance_work() {
    for status in [
        GoalStatus::Implement,
        GoalStatus::Quality,
        GoalStatus::Governance,
    ] {
        let fixture = Fixture::new();
        let mut initial = fixture.context(GoalStatus::Plan);
        hydrate_plan_or_implement_context(&mut initial, "refine/{goal_id}", "main").unwrap();
        let branch = initial.branch.clone().unwrap();
        let git = FileGitWorktreeService::new(&fixture.repository);
        let base = git.resolve_commit("main").unwrap();
        fixture
            .items
            .update_goal_git_refs("GOAL1", &branch, "main", &base, Some(&base))
            .unwrap();
        let ahead = fixture.commit_file(
            Path::new(initial.worktree_path.as_deref().unwrap()),
            "unrecorded implementation",
        );
        fixture
            .items
            .set_goal_status_unchecked("GOAL1", &status)
            .unwrap();
        let mut resumed = fixture.context(status.clone());
        let prepared = prepare_workspace(&mut resumed, status.clone(), "main").unwrap();
        assert_eq!(git.resolve_commit(&branch).unwrap(), ahead);
        assert_eq!(git.resolve_commit("main").unwrap(), base);
        if status == GoalStatus::Implement {
            assert_eq!(prepared.status, GoalStatus::Implement);
            assert_eq!(prepared.branch, branch);
        } else {
            assert_eq!(prepared.status, GoalStatus::Plan);
            assert_ne!(prepared.branch, branch);
            assert_eq!(prepared.candidate, None);
        }
    }
}

#[test]
fn interrupted_rebase_retains_its_exact_original_branch_for_existing_recovery() {
    let fixture = Fixture::new();
    let mut initial = fixture.context(GoalStatus::Plan);
    hydrate_plan_or_implement_context(&mut initial, "refine/{goal_id}", "main").unwrap();
    let branch = initial.branch.clone().unwrap();
    let worktree = Path::new(initial.worktree_path.as_deref().unwrap());
    let git = FileGitWorktreeService::new(&fixture.repository);
    let base = git.resolve_commit("main").unwrap();
    let candidate = fixture.commit_file(worktree, "candidate version");
    fixture
        .items
        .update_goal_git_refs("GOAL1", &branch, "main", &base, Some(&candidate))
        .unwrap();
    fixture.commit_file(&fixture.repository, "target version");
    assert!(
        !std::process::Command::new("git")
            .args(["rebase", "main"])
            .current_dir(worktree)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        FileGitWorktreeService::new(worktree)
            .operation_in_progress()
            .unwrap()
    );
    fixture
        .items
        .set_goal_status_unchecked("GOAL1", &GoalStatus::Governance)
        .unwrap();
    let mut resumed = fixture.context(GoalStatus::Governance);
    let prepared = prepare_workspace(&mut resumed, GoalStatus::Governance, "main").unwrap();
    assert_eq!(prepared.status, GoalStatus::Governance);
    assert_eq!(prepared.branch, branch);
    assert_eq!(prepared.candidate.as_deref(), Some(candidate.as_str()));
    assert_eq!(git.resolve_commit(&branch).unwrap(), candidate);
    assert!(
        FileGitWorktreeService::new(worktree)
            .operation_in_progress()
            .unwrap()
    );
}

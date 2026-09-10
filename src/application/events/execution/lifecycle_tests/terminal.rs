//! Terminal lifecycle hooks remain runnable when a Goal stops before Plan.
use super::*;
use crate::model::workflow::GoalStatus;

#[test]
fn preplan_terminal_entry_and_exit_skills_use_lifecycle_workspaces() {
    for status in [GoalStatus::Cancelled, GoalStatus::Failed] {
        let f = Fixture::new();
        let _smoke = SmokeSkill::install(&f.service, &f.temp);
        let enter = format!("workflow.{}.enter", status.as_str());
        let exit = format!("workflow.{}.exit", status.as_str());
        f.gate(&enter, BindingMode::Blocking);
        f.gate(&exit, BindingMode::Blocking);
        let before = f.snapshot();
        f.work()
            .transition_goal_status("FRESH", GoalStatus::Todo)
            .unwrap();
        if status == GoalStatus::Cancelled {
            f.work().cancel_goal_summary("FRESH").unwrap();
        } else {
            f.work()
                .advance_automated_goal_status("FRESH", GoalStatus::Failed)
                .unwrap();
        }
        // Forced settlement does not wait for the configured terminal Entry.
        assert_eq!(
            f.work().show_goal_detail("FRESH").unwrap()["status"],
            status.as_str()
        );
        f.assert_no_candidate();
        f.dispatch();
        let entry = f.invocation(&enter);
        assert_eq!(
            entry.state,
            InvocationState::Pending,
            "{enter}: {:?}",
            entry.error
        );
        let completed = f.execute(&entry);
        assert_eq!(completed.state, InvocationState::Succeeded);
        assert_eq!(
            completed.context.lifecycle.as_ref().unwrap().source_commit,
            f.base
        );
        f.request_todo();
        f.dispatch();
        let exit = f.invocation(&exit);
        assert_eq!(f.execute(&exit).state, InvocationState::Succeeded);
        f.dispatch();
        assert_eq!(
            f.work().show_goal_detail("FRESH").unwrap()["status"],
            "todo"
        );
        assert_eq!(completed, f.service.invocation(&entry.id).unwrap());
        f.assert_no_candidate();
        assert_eq!(before, f.snapshot());
    }
}

#[test]
fn terminal_skills_do_not_replace_a_missing_recorded_implementation_workspace() {
    let f = Fixture::new();
    let _smoke = SmokeSkill::install(&f.service, &f.temp);
    f.gate("workflow.cancelled.enter", BindingMode::Blocking);
    let branch = "refine/FRESH/round-1";
    let repository = FileGitWorktreeService::new(&f.primary);
    let path = repository.managed_worktree_path(branch).unwrap();
    git(&f.primary, &["branch", branch, &f.base]);
    f.work()
        .update_goal_git_refs("FRESH", branch, "main", &f.base, Some(&f.base))
        .unwrap();
    // The durable candidate survives, but no checkout is registered for it.
    let before = f.snapshot();
    f.work().cancel_goal_summary("FRESH").unwrap();
    f.dispatch();
    let invocation = f.invocation("workflow.cancelled.enter");
    assert_eq!(invocation.state, InvocationState::Error);
    assert!(invocation.context.lifecycle.is_none());
    assert!(invocation.attempts.is_empty());
    assert!(!path.exists());
    assert_eq!(git(&f.primary, &["rev-parse", branch]), f.base);
    assert!(
        git(
            &f.primary,
            &["branch", "--list", "refine/FRESH/lifecycle-*"]
        )
        .is_empty()
    );
    assert_eq!(before, f.snapshot());
}

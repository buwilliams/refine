use super::*;
#[test]
fn result_construction_fault_and_panic_settle_without_overwriting_newer_intent() {
    for panic in [false, true] {
        let root =
            std::env::temp_dir().join(format!("refine-result-fault-{}", uuid::Uuid::new_v4()));
        let target = root.join("app");
        std::fs::create_dir_all(&target).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["config", "user.name", "Test"],
            vec!["commit", "--allow-empty", "-m", "base"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .current_dir(&target)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let runtime = root.join("run");
        let items = FileWorkItemService::new(prepare_refine_dir(&target).unwrap());
        items
            .create_goal_summary("Result failure", Some("GOAL1"))
            .unwrap();
        items
            .append_goal_round_summary("GOAL1", "Reporter", "Request")
            .unwrap();
        items
            .transition_goal_status("GOAL1", GoalStatus::Todo)
            .unwrap();
        let (round, revision, request) = items.authored_goal_commitment("GOAL1").unwrap();
        let authority = items
            .claim_workflow_attempt("GOAL1", GoalStatus::Todo, round, revision, &request)
            .unwrap();
        let authority = items
            .advance_claimed_goal_status("GOAL1", authority, GoalStatus::Todo, GoalStatus::Plan)
            .unwrap();
        let context = WorkflowContext::new(
            &runtime,
            &target,
            "GOAL1".into(),
            "default".into(),
            "smoke-ai".into(),
            round,
            authority,
            Default::default(),
            items.clone(),
        );
        let engine = WorkflowEngine::with_target_root(&runtime, &target);
        if panic {
            test_hooks::install(
                &runtime,
                std::sync::Arc::new(|_, _, stage, _| {
                    if stage == "result" {
                        panic!("result panic");
                    }
                    Ok(())
                }),
            );
        }
        let error = engine.complete_goal_result(context).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(if panic { "panicked" } else { "branch" })
        );
        assert_eq!(
            items.show_goal_summary("GOAL1").unwrap().goal.status,
            GoalStatus::Failed
        );
        assert_eq!(
            items.show_goal_detail("GOAL1").unwrap()["rounds"][0]["failure_category"],
            "result"
        );
        test_hooks::remove(&runtime);
        items
            .create_goal_summary("Follow-up", Some("GOAL2"))
            .unwrap();
        items
            .append_goal_round_summary("GOAL2", "Reporter", "Request")
            .unwrap();
        items
            .transition_goal_status("GOAL2", GoalStatus::Todo)
            .unwrap();
        let followed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = followed.clone();
        let records = items.clone();
        test_hooks::install(
            &runtime,
            std::sync::Arc::new(move |_, goal, stage, _| {
                if goal == "GOAL2" && stage == "executing" {
                    assert_eq!(
                        records.show_goal_summary(goal)?.goal.status,
                        GoalStatus::Plan
                    );
                    seen.store(true, std::sync::atomic::Ordering::SeqCst);
                    return Err(RefineError::Conflict("follow-up reached Plan".into()));
                }
                Ok(())
            }),
        );
        let ended = Instant::now();
        assert!(engine.execute_work().is_err());
        assert!(followed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(ended.elapsed() < Duration::from_secs(1));
        test_hooks::remove(&runtime);
        std::fs::remove_dir_all(root).unwrap();
    }
}

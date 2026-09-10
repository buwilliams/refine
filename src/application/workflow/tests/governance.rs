use super::*;

#[test]
fn file_automation_fails_after_the_governance_recovery_budget_is_exhausted() {
    let temp_root = unique_temp_dir("automation-governance");
    let target_root = temp_root.clone();
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    git(&temp_root, &["init", "-q"]).unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    // Each verdict names a different rule so the repeated-signature early stop
    // never fires and the shared Round budget is what settles the Goal.
    let violation_counter = format!("{}-governance-count", temp_root.display());
    fs::write(
            &smoke_ai,
            format!(
                "#!/bin/sh\n\
                 case \"$*\" in\n\
                 *\"Post-implementation governance review\"*)\n\
                   n=$(cat {violation_counter} 2>/dev/null || echo 0)\n\
                   n=$((n+1))\n\
                   echo $n > {violation_counter}\n\
                   printf '%s\\n' '{{\"status\":\"failed\",\"message\":\"Do not append smoke markers.\",\"violations\":[{{\"rule_id\":\"rule-'$n'\",\"rule\":\"Do not append smoke markers.\",\"message\":\"app.py contains a smoke marker\"}}],\"recovery_analysis\":\"The implementation appended a forbidden marker.\",\"recovery_round_prompt\":\"Remove the smoke marker from app.py and preserve the health function without generated markers.\"}}'\n\
                   ;;\n\
                 *)\n\
                   printf '\\n# automated by smoke-ai governance violation\\n' >> app.py\n\
                   printf '%s\\n' 'smoke-ai goal-agent response'\n\
                   ;;\n\
                 esac\n"
            ),
        )
        .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    git(&temp_root, &["add", "smoke-ai"]).unwrap();
    git(
        &temp_root,
        &["commit", "-q", "-m", "Add test provider fixture"],
    )
    .unwrap();

    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var(
            "REFINE_SMOKE_AI_PATH",
            crate::application::events::test_support::adapt_fixture(&smoke_ai),
        );
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Governed implementation", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();
    FileGovernanceService::new(&refine_dir)
        .save(&json!({
            "product": "A small app.",
            "constitution": "Keep generated markers out of app.py.",
            "rules": [{"id": "rule-1", "text": "Do not append smoke markers.", "source": "manual"}]
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let error = automation.evaluate_workflow().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Governance findings remain after 5 automatic recovery Rounds"),
        "{error}"
    );
    let goal = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["status"], "failed");
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 6);
    let latest = &goal["rounds"][5];
    assert_eq!(latest["rule_state"], "failed");
    assert_eq!(latest["quality_state"], "passed");
    assert!(
        latest["governance_message"]
            .as_str()
            .unwrap_or("")
            .contains("Do not append smoke markers.")
    );
    assert_eq!(latest["governance_details"]["phase"], "post_implementation");
    assert_eq!(
        latest["governance_rule_actions"][0]["rule_id"],
        "workflow.governance.enter:default-governance:rule-6"
    );
    assert_eq!(goal["rounds"][5]["automatic_retry"]["attempt"], 5);
    assert_eq!(goal["rounds"][5]["automatic_retry"]["kind"], "governance");
    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_automation_stops_early_when_consecutive_governance_findings_are_identical() {
    let temp_root = unique_temp_dir("automation-governance-repeat");
    let target_root = temp_root.clone();
    let refine_dir = test_refine_dir(&target_root);
    let runtime_root = temp_root.join("run/8080");
    let smoke_ai = temp_root.join("smoke-ai");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(temp_root.join("app.py"), "def health():\n    return 'ok'\n").unwrap();
    git(&temp_root, &["init", "-q"]).unwrap();
    git(
        &temp_root,
        &["config", "user.email", "refine-test@example.invalid"],
    )
    .unwrap();
    git(&temp_root, &["config", "user.name", "Refine Test"]).unwrap();
    git(&temp_root, &["add", "app.py"]).unwrap();
    git(&temp_root, &["commit", "-q", "-m", "Initialize test app"]).unwrap();
    // The verdict names the same rule every time: the second Round reproduces
    // the first Round's exact signature, so retries stop at two Rounds instead
    // of burning the remaining automatic Round budget.
    fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *\"Post-implementation governance review\"*)\n\
               printf '%s\\n' '{\"status\":\"failed\",\"message\":\"Do not append smoke markers.\",\"violations\":[{\"rule_id\":\"rule-1\",\"rule\":\"Do not append smoke markers.\",\"message\":\"app.py contains a smoke marker\"}],\"recovery_analysis\":\"The implementation appended a forbidden marker.\",\"recovery_round_prompt\":\"Remove the smoke marker from app.py and preserve the health function without generated markers.\"}'\n\
               ;;\n\
             *)\n\
               printf '\\n# automated by smoke-ai governance violation\\n' >> app.py\n\
               printf '%s\\n' 'smoke-ai goal-agent response'\n\
               ;;\n\
             esac\n",
        )
        .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&smoke_ai).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&smoke_ai, permissions).unwrap();
    }
    git(&temp_root, &["add", "smoke-ai"]).unwrap();
    git(
        &temp_root,
        &["commit", "-q", "-m", "Add test provider fixture"],
    )
    .unwrap();

    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var(
            "REFINE_SMOKE_AI_PATH",
            crate::application::events::test_support::adapt_fixture(&smoke_ai),
        );
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Governed implementation", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Prompt")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();
    FileGovernanceService::new(&refine_dir)
        .save(&json!({
            "product": "A small app.",
            "constitution": "Keep generated markers out of app.py.",
            "rules": [{"id": "rule-1", "text": "Do not append smoke markers.", "source": "manual"}]
        }))
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let error = automation.evaluate_workflow().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("identical findings; automatic retries were stopped early"),
        "{error}"
    );
    let goal = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["status"], "failed");
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 2);
    let latest = &goal["rounds"][1];
    assert_eq!(latest["automatic_retry"]["kind"], "governance");
    assert_eq!(latest["automatic_retry"]["attempt"], 1);
    assert_eq!(latest["failure_category"], "governance_retry_exhausted");
    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(temp_root).unwrap();
}

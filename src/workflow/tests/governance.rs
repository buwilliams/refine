use super::*;

#[test]
fn governance_review_prose_with_code_braces_reads_the_verdict_not_the_first_brace() {
    // Shape of the review that was misread: a code reference containing
    // braces, the phrase "rule violation" used while *clearing* a rule, and
    // the real verdict last.
    let output = "## Rule 3\n\
             The `error: () => { setDiaryError(true) }` handler in \
             `onClaimNumberChange` is unchanged, so this is not a rule violation. \
             Compliant.\n\n\
             {\"status\":\"passed\",\"message\":\"Round 1 is a clean, well-scoped diary-editor bug fix.\"}";

    let evaluation = parse_governance_provider_output(output, 3);

    assert!(!evaluation.failed);
    assert_eq!(
        evaluation.message.as_deref(),
        Some("Round 1 is a clean, well-scoped diary-editor bug fix.")
    );
    assert_eq!(evaluation.details["failed_actions"], json!([]));
    assert_eq!(evaluation.details["verdict"]["status"], "passed");
}

#[test]
fn governance_verdict_survives_an_unclosed_brace_in_the_review_prose() {
    let output = "Reviewed `if (claim.diary) {` in the editor.\n\
             {\"status\":\"passed\",\"message\":\"All rules compliant.\"}";

    let evaluation = parse_governance_provider_output(output, 2);

    assert!(!evaluation.failed);
    assert_eq!(evaluation.details["verdict"]["status"], "passed");
}

#[test]
fn governance_review_without_a_verdict_fails_closed_as_a_parse_error() {
    let output = "The change looks fine to me. No rules were broken.";

    let evaluation = parse_governance_provider_output(output, 2);

    // Unreadable verdicts must be obviously a parsing problem, never a
    // silent pass and never a fabricated rule violation.
    assert!(evaluation.failed);
    assert_eq!(
        evaluation.message.as_deref(),
        Some(GOVERNANCE_VERDICT_UNPARSABLE)
    );
    assert_eq!(evaluation.details["verdict_parse_error"], true);
    assert_eq!(
        evaluation.details["failed_actions"][0]["action"],
        "verdict_parse_error"
    );
    // The review body is kept for triage, but is not itself the failure.
    assert_eq!(evaluation.details["raw_output"], output);
    assert_ne!(evaluation.details["failed_actions"][0]["message"], output);
}

#[test]
fn governance_failing_verdict_records_the_parsed_violations() {
    let output = "Rule 1 is violated.\n\
             {\"status\":\"failed\",\"message\":\"app.py contains a smoke marker\",\
             \"violations\":[{\"rule_id\":\"rule-1\",\"message\":\"smoke marker appended\"}]}";

    let evaluation = parse_governance_provider_output(output, 1);

    assert!(evaluation.failed);
    assert_eq!(
        evaluation.message.as_deref(),
        Some("app.py contains a smoke marker")
    );
    assert_eq!(evaluation.details["failed_actions"][0]["rule_id"], "rule-1");
}

#[test]
fn file_automation_fails_in_progress_goal_on_post_implementation_governance_violation() {
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
    fs::write(
            &smoke_ai,
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *\"Post-implementation governance review\"*)\n\
               printf '%s\\n' '{\"status\":\"failed\",\"message\":\"Do not append smoke markers.\",\"violations\":[{\"rule_id\":\"rule-1\",\"rule\":\"Do not append smoke markers.\",\"message\":\"app.py contains a smoke marker\"}]}'\n\
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

    let _smoke_ai_env_guard = smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous_smoke_ai = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
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
    assert!(error.to_string().contains("Do not append smoke markers."));
    let goal = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["status"], "failed");
    let latest = &goal["rounds"][0];
    assert_eq!(latest["rule_state"], "failed");
    assert_eq!(latest["quality_state"], "unclassified");
    assert!(
        latest["governance_message"]
            .as_str()
            .unwrap_or("")
            .contains("Do not append smoke markers.")
    );
    assert_eq!(latest["governance_details"]["phase"], "post_implementation");
    assert_eq!(latest["governance_rule_actions"][0]["rule_id"], "rule-1");
    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(temp_root).unwrap();
}

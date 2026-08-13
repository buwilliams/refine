use super::*;
use crate::tools::host::quality::{FileQualityService, QualitySettingsPatch};

#[test]
fn file_automation_fails_after_the_shared_quality_recovery_budget_is_exhausted() {
    let temp_root = unique_temp_dir("automation-quality-recovery");
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
         *\"Post-implementation Quality evaluation\"*)\n\
           printf '%s\\n' '{\"ok\":false,\"summary\":\"The health check fails.\",\"results\":[{\"test\":\"Health check passes\",\"status\":\"failed\",\"evidence\":\"The command exits unsuccessfully.\",\"command\":\"false\"}]}'\n\
           ;;\n\
         *\"Quality Recovery Investigation\"*)\n\
           printf '%s\\n' '{\"recovery_analysis\":\"The implementation does not satisfy the configured health check.\",\"recovery_round_prompt\":\"Correct the health-check implementation and add a focused regression test that passes before Quality runs again.\"}'\n\
           ;;\n\
         *)\n\
           printf '\\n# automated by smoke-ai quality fixture\\n' >> app.py\n\
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
        std::env::set_var("REFINE_SMOKE_AI_PATH", smoke_ai.to_str().unwrap());
    }
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Quality recovery", Some("GOAL1"))
        .unwrap();
    work_items
        .append_goal_round_summary("GOAL1", "Reporter", "Implement the health check")
        .unwrap();
    work_items
        .transition_goal_status("GOAL1", GoalStatus::Todo)
        .unwrap();
    FileSettingsService::new(&refine_dir)
        .update(&json!({"agent_cli": "smoke-ai"}))
        .unwrap();
    FileQualityService::new(&refine_dir)
        .save_settings(QualitySettingsPatch {
            tests: Some(vec!["Health check passes".to_string()]),
            ..QualitySettingsPatch::default()
        })
        .unwrap();

    let automation = WorkflowEngine::with_target_root(&runtime_root, &target_root);
    let error = automation.evaluate_workflow().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Quality findings remain after 5 automatic recovery Rounds"),
        "{error}"
    );
    let goal = work_items.show_goal_detail("GOAL1").unwrap();
    assert_eq!(goal["status"], "failed");
    assert_eq!(goal["rounds"].as_array().unwrap().len(), 6);
    let latest = &goal["rounds"][5];
    assert_eq!(latest["quality_state"], "failed");
    assert_eq!(latest["automatic_retry"]["attempt"], 5);
    assert_eq!(latest["automatic_retry"]["kind"], "quality");
    assert_eq!(
        latest["quality_recovery_details"]["phase"],
        "quality_recovery"
    );
    assert!(
        goal["rounds"][0]["quality_recovery_analysis"]
            .as_str()
            .unwrap_or("")
            .contains("health check")
    );
    unsafe {
        if let Some(previous) = previous_smoke_ai {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }

    fs::remove_dir_all(temp_root).unwrap();
}

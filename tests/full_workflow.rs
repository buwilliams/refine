mod support;

use std::fs;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use support::integration::IntegrationFixture;

#[test]
#[ignore]
fn daemon_automation_runs_full_goal_workflow_through_git_worktree() {
    let provider = deterministic_provider_script();
    let previous_provider = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let fixture = IntegrationFixture::start_with_agent_automation("full-workflow");
    fixture.api_json(
        "PATCH",
        "/api/settings",
        serde_json::json!({
            "agent_cli": "smoke-ai",
            "quality_enabled": "1",
            "target_app_test_command": "printf target-tests-ok",
            "branch_name_pattern": "refine/{goal_id}"
        }),
    );
    fixture.api_json(
        "PATCH",
        "/api/governance",
        serde_json::json!({
            "product": "Disposable automation workflow test app",
            "constitution": "Allow deterministic smoke workflow changes.",
            "rules": [{"text": "Allow deterministic smoke workflow changes.", "source": "test"}]
        }),
    );

    let goal_id = fixture.create_goal("full workflow git-backed goal");
    let round = fixture.run_refine(&[
        "goal",
        "round",
        &goal_id,
        "--reporter",
        "refine-smoke",
        "--prompt",
        "Implement the deterministic full workflow provider change.",
    ]);
    fixture.assert_success("author workflow round", &round);
    let start = fixture.run_refine(&["goal", "start", &goal_id]);
    fixture.assert_success("start goal workflow", &start);
    wait_for_goal_status(&fixture, &goal_id, "review");

    let branch = format!("refine/{goal_id}/round-1");
    let worktree = fixture
        .app_root
        .join(".git/refine-worktrees")
        .join(branch.replace('/', "-"));
    let app_py = fs::read_to_string(fixture.app_root.join("app.py")).unwrap();
    assert!(
        app_py.contains("full workflow provider edit"),
        "review did not contain the integrated provider edit:\n{app_py}"
    );
    let candidate_py = fs::read_to_string(worktree.join("app.py")).unwrap();
    assert!(
        candidate_py.contains("full workflow provider edit"),
        "review candidate did not contain provider edit:\n{candidate_py}"
    );

    let shown = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after workflow", &shown);
    let goal = fixture.json_stdout(&shown);
    let latest = &goal["goal"]["rounds"][0];
    assert_eq!(goal["goal"]["status"], "review", "{goal:#}");
    assert_eq!(goal["goal"]["branch_name"], branch);
    assert_eq!(goal["goal"]["target_branch"], "main");
    assert!(
        goal["goal"]["base_commit"]
            .as_str()
            .is_some_and(|commit| commit.len() == 40),
        "{goal:#}"
    );
    let candidate_commit = git(&worktree, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(goal["goal"]["candidate_commit"], candidate_commit);
    let reviewed_head = git(&fixture.app_root, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    assert!(latest.get("workflow_quality_timing").is_none(), "{goal:#}");
    assert_eq!(
        latest["workflow_integration"]["candidate_commit"], candidate_commit,
        "{goal:#}"
    );
    assert_eq!(
        latest["workflow_integration"]["target_commit"], reviewed_head,
        "{goal:#}"
    );
    assert_eq!(
        latest["quality_details"]["evaluation_scope"], "isolated_candidate",
        "{goal:#}"
    );
    assert_eq!(
        latest["quality_details"]["source_candidate_commit"], candidate_commit,
        "{goal:#}"
    );
    assert_eq!(
        latest["quality_details"]["candidate_commit"], candidate_commit,
        "{goal:#}"
    );
    assert_eq!(
        latest["quality_details"]["cwd"],
        worktree.display().to_string(),
        "{goal:#}"
    );
    let state_messages = latest["logs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|log| log["category"] == "state")
        .filter_map(|log| log["message"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        state_messages,
        vec![
            "Workflow status changed: todo -> plan",
            "Workflow status changed: plan -> implement",
            "Workflow status changed: implement -> quality",
            "Workflow status changed: quality -> governance",
            "Workflow status changed: governance -> review",
        ],
        "{goal:#}"
    );
    assert_eq!(latest["quality_state"], "passed", "{goal:#}");
    assert_eq!(latest["rule_state"], "passed", "{goal:#}");
    assert_eq!(
        latest["implementation_report"], "full workflow provider completed",
        "{goal:#}"
    );
    assert!(
        latest["implementation_reported_at"].as_str().is_some(),
        "{goal:#}"
    );

    let audit_path = fixture.app_root.join(".git/refine-audit.jsonl");
    let audit_before_approval = fs::read_to_string(&audit_path).unwrap();
    let approve = fixture.run_refine(&["goal", "approve", &goal_id]);
    fixture.assert_success("approve reviewed goal", &approve);
    wait_for_goal_status(&fixture, &goal_id, "done");
    assert_eq!(
        git(&fixture.app_root, &["rev-parse", "HEAD"]).trim(),
        reviewed_head,
        "approval reintegrated an already-integrated candidate"
    );
    let audit_after_approval = fs::read_to_string(&audit_path).unwrap();
    assert_eq!(
        audit_after_approval, audit_before_approval,
        "review approval performed a Git operation"
    );
    let app_py = fs::read_to_string(fixture.app_root.join("app.py")).unwrap();
    assert!(
        app_py.contains("full workflow provider edit"),
        "approved app.py did not contain provider edit:\n{app_py}"
    );
    let log = git(&fixture.app_root, &["log", "--oneline", "--decorate", "-5"]);
    assert!(
        log.contains(&format!("Implement {goal_id} round 1")),
        "missing approved implementation commit in target app log:\n{log}"
    );
    assert!(
        !log.contains("Sync Refine state"),
        "application history contains a Refine state commit:\n{log}"
    );

    unsafe {
        if let Some(previous) = previous_provider {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
}

/// The disruption doctrine, end to end at the riskiest phase: the daemon is
/// SIGKILLed mid-Governance — integrated-target transaction marker written,
/// governance review agent in flight — and a fresh daemon over the same
/// durable state must carry the Goal to Review with no manual intervention.
#[test]
#[ignore]
fn daemon_sigkill_mid_governance_recovers_to_review_without_manual_intervention() {
    let control_dir = std::env::temp_dir().join(format!(
        "refine-sigkill-governance-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&control_dir).unwrap();
    let provider = stalling_governance_provider_script(&control_dir);
    let previous_provider = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let mut fixture = IntegrationFixture::start_with_agent_automation("sigkill-governance");
    fixture.api_json(
        "PATCH",
        "/api/settings",
        serde_json::json!({
            "agent_cli": "smoke-ai",
            "quality_enabled": "1",
            "target_app_test_command": "printf target-tests-ok",
            "branch_name_pattern": "refine/{goal_id}"
        }),
    );
    fixture.api_json(
        "PATCH",
        "/api/governance",
        serde_json::json!({
            "product": "Disposable automation workflow test app",
            "constitution": "Allow deterministic smoke workflow changes.",
            "rules": [{"text": "Allow deterministic smoke workflow changes.", "source": "test"}]
        }),
    );

    let goal_id = fixture.create_goal("sigkill mid-governance goal");
    let round = fixture.run_refine(&[
        "goal",
        "round",
        &goal_id,
        "--reporter",
        "refine-smoke",
        "--prompt",
        "Implement the deterministic full workflow provider change.",
    ]);
    fixture.assert_success("author workflow round", &round);
    let start = fixture.run_refine(&["goal", "start", &goal_id]);
    fixture.assert_success("start goal workflow", &start);

    // The first governance review invocation signals and stalls; catching the
    // sentinel means the daemon is inside the integrated-target transaction.
    let sentinel = control_dir.join("governance-invoked");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !sentinel.exists() {
        assert!(
            Instant::now() < deadline,
            "governance review was never invoked; latest status was {}",
            fixture.goal_field(&goal_id, "status")
        );
        thread::sleep(Duration::from_millis(100));
    }
    let marker = fixture
        .app_root
        .join(".git/refine-integrated-target-transaction.json");
    assert!(
        marker.exists(),
        "the integrated-target transaction marker must be open mid-Governance"
    );
    let quality_operation_id =
        fixture.goal_field(&goal_id, "rounds")[0]["quality_details"]["operation_id"]
            .as_str()
            .expect("quality proof recorded before governance")
            .to_string();

    fixture.kill_daemon_hard();
    fixture.restart_daemon();

    // No manual intervention: the restarted runner's own recovery pass and
    // scheduler must finish the Goal.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let status = fixture.goal_field(&goal_id, "status");
        if status.as_str() == Some("review") {
            break;
        }
        assert!(
            status.as_str() != Some("failed"),
            "Goal {goal_id} failed after the restart instead of recovering"
        );
        assert!(
            Instant::now() < deadline,
            "Goal {goal_id} did not reach review after restart; latest status was {status}"
        );
        thread::sleep(Duration::from_millis(200));
    }

    let shown = fixture.run_refine(&["goal", "show", &goal_id]);
    fixture.assert_success("goal show after recovery", &shown);
    let goal = fixture.json_stdout(&shown);
    let latest = &goal["goal"]["rounds"][0];
    assert_eq!(goal["goal"]["status"], "review", "{goal:#}");
    let candidate = goal["goal"]["candidate_commit"].as_str().unwrap();
    assert_eq!(
        latest["workflow_integration"]["candidate_commit"], candidate,
        "{goal:#}"
    );
    // The Quality proof survived the kill and was reused, not regenerated.
    assert_eq!(
        latest["quality_details"]["operation_id"], quality_operation_id,
        "{goal:#}"
    );
    assert_eq!(latest["rule_state"], "passed", "{goal:#}");
    assert!(
        !marker.exists(),
        "the interrupted transaction marker must be recovered and closed"
    );
    let app_py = fs::read_to_string(fixture.app_root.join("app.py")).unwrap();
    assert!(
        app_py.contains("full workflow provider edit"),
        "recovered review did not contain the integrated provider edit:\n{app_py}"
    );

    unsafe {
        if let Some(previous) = previous_provider {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
    let _ = fs::remove_dir_all(&control_dir);
}

fn wait_for_goal_status(fixture: &IntegrationFixture, goal_id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = fixture.goal_field(goal_id, "status");
        if status.as_str() == Some(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Goal {goal_id} did not reach {expected}; latest status was {status}"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

fn deterministic_provider_script() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "refine-full-workflow-provider-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &path,
        "#!/bin/sh\n\
         case \"$*\" in\n\
         *\"# Current Workflow Phase: Plan\"*)\n\
           printf '%s\\n' '{\"state\":\"completed\",\"message\":\"Plan proposed.\",\"guidance_applied\":[],\"planning_result\":{\"summary\":\"Implement and verify the deterministic provider change through the governed workflow.\",\"checklist\":[{\"id\":\"P1\",\"description\":\"Apply and verify the requested deterministic provider change.\"}]}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
           ;;\n\
         *\"# Current Workflow Phase: Criticize\"*)\n\
           printf '%s\\n' '{\"state\":\"completed\",\"message\":\"Plan criticized.\",\"guidance_applied\":[],\"planning_result\":{\"summary\":\"No material omissions found.\",\"findings\":[]}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
           ;;\n\
         *\"# Current Workflow Phase: Revise\"*)\n\
           printf '%s\\n' '{\"state\":\"completed\",\"message\":\"Plan finalized.\",\"guidance_applied\":[],\"planning_result\":{\"summary\":\"Implement and verify the deterministic provider change through the governed workflow.\",\"checklist\":[{\"id\":\"P1\",\"description\":\"Apply and verify the requested deterministic provider change.\"}],\"criticism_resolutions\":[]}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
           ;;\n\
         *\"# Goal Workflow Quality\"*)\n\
           printf '%s\\n' '{\"state\":\"completed\",\"message\":\"Quality reviewed the candidate and retained the passing test.\",\"guidance_applied\":[]}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
           ;;\n\
         *\"Post-implementation Quality evaluation\"*)\n\
           printf '%s\\n' '{\"ok\":true,\"summary\":\"Quality planned.\",\"results\":[{\"test\":\"Migrated Quality command passes: printf target-tests-ok\",\"status\":\"passed\",\"evidence\":\"legacy command selected\",\"command\":\"printf target-tests-ok\"}]}'\n\
           ;;\n\
         *\"Plan-stage governance pre-check\"*)\n\
           printf '%s\\n' '{\"status\":\"passed\",\"message\":\"Plan pre-check passed.\",\"violations\":[]}'\n\
           ;;\n\
         *\"Post-implementation governance review\"*)\n\
           printf '%s\\n' '{\"status\":\"passed\",\"message\":\"Governance passed.\",\"violations\":[]}'\n\
           ;;\n\
         *)\n\
           printf '\\n# full workflow provider edit\\n' >> app.py\n\
           printf '%s\\n' '{\"state\":\"completed\",\"message\":\"full workflow provider completed\",\"guidance_applied\":[],\"implementation_evidence\":{\"checklist\":[{\"id\":\"P1\",\"outcome\":\"completed\",\"evidence\":\"Applied the deterministic app.py change.\"}],\"verification\":[\"provider fixture completed\"]}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
           ;;\n\
         esac\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
    path
}

/// The deterministic provider, except the FIRST post-implementation
/// governance review drops a sentinel and stalls so the test can land a
/// SIGKILL mid-Governance; every later review passes.
fn stalling_governance_provider_script(control_dir: &std::path::Path) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "refine-sigkill-governance-provider-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let control = control_dir.display();
    fs::write(
        &path,
        format!(
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *\"# Current Workflow Phase: Plan\"*)\n\
               printf '%s\\n' '{{\"state\":\"completed\",\"message\":\"Plan proposed.\",\"guidance_applied\":[],\"planning_result\":{{\"summary\":\"Implement and verify the deterministic provider change through the governed workflow.\",\"checklist\":[{{\"id\":\"P1\",\"description\":\"Apply and verify the requested deterministic provider change.\"}}]}}}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
               ;;\n\
             *\"# Current Workflow Phase: Criticize\"*)\n\
               printf '%s\\n' '{{\"state\":\"completed\",\"message\":\"Plan criticized.\",\"guidance_applied\":[],\"planning_result\":{{\"summary\":\"No material omissions found.\",\"findings\":[]}}}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
               ;;\n\
             *\"# Current Workflow Phase: Revise\"*)\n\
               printf '%s\\n' '{{\"state\":\"completed\",\"message\":\"Plan finalized.\",\"guidance_applied\":[],\"planning_result\":{{\"summary\":\"Implement and verify the deterministic provider change through the governed workflow.\",\"checklist\":[{{\"id\":\"P1\",\"description\":\"Apply and verify the requested deterministic provider change.\"}}],\"criticism_resolutions\":[]}}}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
               ;;\n\
             *\"# Goal Workflow Quality\"*)\n\
               printf '%s\\n' '{{\"state\":\"completed\",\"message\":\"Quality reviewed the candidate and retained the passing test.\",\"guidance_applied\":[]}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
               ;;\n\
             *\"Post-implementation Quality evaluation\"*)\n\
               printf '%s\\n' '{{\"ok\":true,\"summary\":\"Quality planned.\",\"results\":[{{\"test\":\"Migrated Quality command passes: printf target-tests-ok\",\"status\":\"passed\",\"evidence\":\"legacy command selected\",\"command\":\"printf target-tests-ok\"}}]}}'\n\
               ;;\n\
             *\"Plan-stage governance pre-check\"*)\n\
               printf '%s\\n' '{{\"status\":\"passed\",\"message\":\"Plan pre-check passed.\",\"violations\":[]}}'\n\
               ;;\n\
             *\"Post-implementation governance review\"*)\n\
               if [ -f \"{control}/governance-stalled\" ]; then\n\
                 printf '%s\\n' '{{\"status\":\"passed\",\"message\":\"Governance passed.\",\"violations\":[]}}'\n\
               else\n\
                 : > \"{control}/governance-stalled\"\n\
                 : > \"{control}/governance-invoked\"\n\
                 sleep 600\n\
               fi\n\
               ;;\n\
             *)\n\
               printf '\\n# full workflow provider edit\\n' >> app.py\n\
               printf '%s\\n' '{{\"state\":\"completed\",\"message\":\"full workflow provider completed\",\"guidance_applied\":[],\"implementation_evidence\":{{\"checklist\":[{{\"id\":\"P1\",\"outcome\":\"completed\",\"evidence\":\"Applied the deterministic app.py change.\"}}],\"verification\":[\"provider fixture completed\"]}}}}' > \"$REFINE_AGENT_SIGNAL_PATH\"\n\
               ;;\n\
             esac\n"
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
    path
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

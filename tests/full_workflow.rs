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

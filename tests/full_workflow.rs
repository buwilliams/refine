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
    let start = fixture.run_refine(&["goal", "start", &goal_id]);
    fixture.assert_success("start goal workflow", &start);
    wait_for_goal_status(&fixture, &goal_id, "review");

    let branch = format!("refine/{goal_id}/round-1");
    let worktree = fixture.app_root.parent().unwrap().join(format!(
        "{}-{}",
        fixture.app_root.file_name().unwrap().to_string_lossy(),
        branch.replace('/', "-")
    ));
    let app_py = fs::read_to_string(fixture.app_root.join("app.py")).unwrap();
    assert!(
        !app_py.contains("full workflow provider edit"),
        "review modified the target branch before approval:\n{app_py}"
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
    assert_eq!(latest["quality_state"], "passed", "{goal:#}");
    assert_eq!(latest["rule_state"], "passed", "{goal:#}");

    let approve = fixture.run_refine(&["goal", "approve", &goal_id]);
    fixture.assert_success("approve reviewed goal", &approve);
    wait_for_goal_status(&fixture, &goal_id, "done");
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
        "#!/bin/sh\nprintf '\\n# full workflow provider edit\\n' >> app.py\nprintf '%s\\n' 'full workflow provider completed'\n",
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

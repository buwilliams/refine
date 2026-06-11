mod support;

use std::fs;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use support::integration::IntegrationFixture;

#[test]
#[ignore]
fn daemon_automation_runs_full_gap_workflow_through_git_worktree() {
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
            "branch_name_pattern": "refine/{gap_id}"
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

    let gap_id = fixture.create_gap("full workflow git-backed gap");
    let transition = fixture.run_refine(&["workflow", "transition", &gap_id, "todo"]);
    fixture.assert_success("transition gap to todo", &transition);
    wait_for_gap_status(&fixture, &gap_id, "review");

    let app_py = fs::read_to_string(fixture.app_root.join("app.py")).unwrap();
    assert!(
        app_py.contains("full workflow provider edit"),
        "merged app.py did not contain provider edit:\n{app_py}"
    );
    let log = git(&fixture.app_root, &["log", "--oneline", "--decorate", "-5"]);
    assert!(
        log.contains(&format!("Implement {gap_id} round 1")),
        "missing implementation commit in target app log:\n{log}"
    );

    let shown = fixture.run_refine(&["gap", "show", &gap_id]);
    fixture.assert_success("gap show after workflow", &shown);
    let gap = fixture.json_stdout(&shown);
    let latest = &gap["gap"]["rounds"][0];
    assert_eq!(gap["gap"]["status"], "review", "{gap:#}");
    assert_eq!(
        gap["gap"]["branch_name"],
        format!("refine/{gap_id}/round-1")
    );
    assert_eq!(latest["quality_state"], "passed", "{gap:#}");
    assert_eq!(latest["rule_state"], "passed", "{gap:#}");

    let round = fixture.run_refine(&[
        "gap",
        "round",
        &gap_id,
        "--reporter",
        "QA",
        "--actual",
        "First round needs another deterministic pass",
        "--target",
        "Second round is queued through todo",
    ]);
    fixture.assert_success("submit second round from review", &round);
    let round_payload = fixture.json_stdout(&round);
    assert_eq!(round_payload["gap"]["status"], "todo", "{round_payload:#}");
    assert_eq!(round_payload["gap"]["round_count"], 2, "{round_payload:#}");

    unsafe {
        if let Some(previous) = previous_provider {
            std::env::set_var("REFINE_SMOKE_AI_PATH", previous);
        } else {
            std::env::remove_var("REFINE_SMOKE_AI_PATH");
        }
    }
}

fn wait_for_gap_status(fixture: &IntegrationFixture, gap_id: &str, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = fixture.gap_field(gap_id, "status");
        if status.as_str() == Some(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Gap {gap_id} did not reach {expected}; latest status was {status}"
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

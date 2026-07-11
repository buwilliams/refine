use std::process::{Command, Output};

fn smoke_ai_path() -> Option<String> {
    std::env::var("REFINE_SMOKE_AI_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn run_smoke_ai(args: &[&str], stdin: Option<&str>, debug: bool) -> Output {
    let path = smoke_ai_path().expect("REFINE_SMOKE_AI_PATH must be set by xtask test-smoke-ai");
    let mut command = Command::new(path);
    command.args(args);
    if debug {
        command.env("SMOKE_AI_DEBUG", "1");
    }
    if stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    } else {
        command.stdin(std::process::Stdio::null());
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().expect("failed to start smoke-ai fixture");
    if let Some(input) = stdin {
        use std::io::Write;
        let mut smoke_ai_stdin = child.stdin.take().expect("smoke-ai stdin was not piped");
        smoke_ai_stdin
            .write_all(input.as_bytes())
            .expect("failed to write smoke-ai stdin");
    }
    child
        .wait_with_output()
        .expect("failed to wait for smoke-ai fixture")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn smoke_ai_contract() {
    if smoke_ai_path().is_none() {
        eprintln!("skipping smoke_ai_contract; REFINE_SMOKE_AI_PATH is not set");
        return;
    }

    let unknown = run_smoke_ai(&["--unknown", "--another-flag"], None, false);
    assert!(unknown.status.success());
    assert!(!stdout(&unknown).is_empty());

    let preflight = run_smoke_ai(
        &["Say exactly the single word hello and nothing else."],
        None,
        false,
    );
    assert!(preflight.status.success());
    assert_eq!(stdout(&preflight), "hello\n");

    for (prompt, expected) in [
        (
            "Start a chat about a product defect.",
            "smoke-ai chat response",
        ),
        (
            "Run the goal agent for a ready Goal.",
            "smoke-ai goal-agent response",
        ),
        ("Import these CSV rows into goals.", "\"kind\": \"import\""),
        (
            "Check the target app and report health.",
            "\"kind\": \"target-app\"",
        ),
        (
            "Generate governance rules for this project.",
            "smoke-ai governance response",
        ),
        ("Completely unrelated prompt.", "smoke-ai fallback response"),
    ] {
        let result = run_smoke_ai(&[prompt], None, false);
        assert!(result.status.success(), "stderr: {}", stderr(&result));
        assert!(stdout(&result).contains(expected), "prompt: {prompt}");
    }

    let delayed_started_at = std::time::Instant::now();
    let delayed = run_smoke_ai(&["Start a smoke-ai queue delay chat turn."], None, false);
    assert!(delayed.status.success(), "stderr: {}", stderr(&delayed));
    assert!(stdout(&delayed).contains("smoke-ai chat response"));
    assert!(delayed_started_at.elapsed() >= std::time::Duration::from_millis(3_500));

    let stdin = run_smoke_ai(
        &[],
        Some("Please import this markdown list into Refine."),
        false,
    );
    assert!(stdin.status.success());
    assert!(stdout(&stdin).contains("\"kind\": \"import\""));

    let target_app = run_smoke_ai(&["Check the target app status."], None, false);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&target_app)).expect("target-app template should be JSON");
    assert_eq!(parsed["kind"], "target-app");
    assert_eq!(parsed["ok"], true);

    let import = run_smoke_ai(&["Import these issues."], None, false);
    let kinds = stdout(&import)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["kind"].clone())
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec!["import", "import"]);

    let normal = run_smoke_ai(&["Run the goal agent."], None, false);
    let debug = run_smoke_ai(&["Run the goal agent."], None, true);
    assert_eq!(stdout(&normal), stdout(&debug));
    assert!(!stderr(&debug).is_empty());
    if let Ok(path) = std::env::var("REFINE_SMOKE_AI_STDERR_ARTIFACT") {
        if !path.trim().is_empty() {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(parent).expect("failed to create smoke-ai artifact dir");
            }
            std::fs::write(path, stderr(&debug)).expect("failed to write smoke-ai stderr artifact");
        }
    }
}

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::json;
use uuid::Uuid;

use super::*;

fn test_directory(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("refine-{label}-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

fn collect_progress() -> (Vec<String>, impl FnMut(&str)) {
    (Vec::new(), |_line: &str| {})
}

fn options(port: u16) -> SystemUpdateOptions {
    SystemUpdateOptions {
        provider: None,
        port,
        rescue: false,
    }
}

fn snapshot(checkout: &Path) -> SourcePromotionSnapshot {
    SourcePromotionSnapshot {
        checkout_path: checkout.display().to_string(),
        current_commit: "aaaaaaaaaaaaaaaa".to_string(),
        remote: "origin".to_string(),
        local_branch: "main".to_string(),
        branch: "main".to_string(),
        available_commit: "bbbbbbbbbbbbbbbb".to_string(),
        clean: true,
        fast_forward: false,
        update_available: true,
        active_work: Vec::new(),
        operation: None,
    }
}

fn failed_operation(checkout: &Path) -> SourcePromotionOperation {
    serde_json::from_value(json!({
        "id": "source-test-operation",
        "status": "failed",
        "stage": "build_candidate",
        "message": "Refine update failed during build_candidate",
        "checkout_path": checkout.display().to_string(),
        "from_commit": "aaaaaaaaaaaaaaaa",
        "to_commit": "bbbbbbbbbbbbbbbb",
        "started_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:01:00Z",
        "error": "cargo build failed",
        "recovery": "Fix the build and retry the update",
        "stashed_changes": "stashsha (refine-update-2026-01-01T00-00-00Z)",
    }))
    .unwrap()
}

#[test]
fn gitless_installation_is_blocked_with_manual_runbook_pointer() {
    let home = test_directory("system-update-gitless");
    let paths = RefineCheckoutPaths::for_test(&home);
    let (_lines, mut progress) = collect_progress();
    let report = run_system_update(&paths, options(65_401), &mut progress);
    assert!(!report.ok);
    assert_eq!(report.outcome, "blocked");
    let blocker = report.blocker.unwrap();
    assert!(blocker.contains("gitless"), "unexpected blocker: {blocker}");
    assert!(
        blocker.contains("install.md"),
        "blocker should point at the manual runbook: {blocker}"
    );
    fs::remove_dir_all(&home).unwrap();
}

#[test]
fn missing_deployed_binary_is_blocked_with_build_guidance() {
    let home = test_directory("system-update-no-binary");
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&home)
            .status()
            .unwrap()
            .success()
    );
    let paths = RefineCheckoutPaths::for_test(&home);
    let (_lines, mut progress) = collect_progress();
    let report = run_system_update(&paths, options(65_402), &mut progress);
    assert!(!report.ok);
    assert_eq!(report.outcome, "blocked");
    let blocker = report.blocker.unwrap();
    assert!(
        blocker.contains("./r system build"),
        "blocker should point at system build: {blocker}"
    );
    fs::remove_dir_all(&home).unwrap();
}

#[test]
fn divergence_rescue_writes_context_and_constrains_the_prompt() {
    let home = test_directory("system-update-rescue-divergence");
    let runtime = home.join("run/8082");
    let source = snapshot(&home);
    let mut captured: Option<ProviderInvocation> = None;
    let output = attempt_agent_rescue(
        &RescueTrigger::Divergence(&source),
        "claude",
        &home,
        &runtime,
        |invocation| {
            captured = Some(invocation);
            Ok("resolved by pushing local commits".to_string())
        },
    )
    .unwrap();
    assert_eq!(output, "resolved by pushing local commits");
    let invocation = captured.unwrap();
    assert_eq!(invocation.provider, "claude");
    assert_eq!(invocation.cwd.as_deref(), Some(home.to_str().unwrap()));
    assert!(invocation.prompt.contains("divergence"));
    assert!(invocation.prompt.contains("fast-forwardable"));
    assert!(invocation.prompt.contains("never discard or reset user work"));
    assert!(invocation.prompt.contains("Do NOT run `./r system update`"));
    assert_eq!(
        invocation.process_metadata.get("kind"),
        Some(&json!("system_update_rescue"))
    );
    let context_path = invocation
        .prompt
        .split_whitespace()
        .find(|token| token.contains("update-rescue-context-"))
        .map(|token| token.trim_end_matches('.').to_string())
        .unwrap();
    let context: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&context_path).unwrap()).unwrap();
    assert_eq!(context["kind"], "divergence");
    assert_eq!(
        context["source"]["available_commit"],
        source.available_commit
    );
    fs::remove_dir_all(&home).unwrap();
}

#[test]
fn terminal_failure_rescue_hands_the_agent_the_full_operation() {
    let home = test_directory("system-update-rescue-terminal");
    let runtime = home.join("run/8082");
    let operation = failed_operation(&home);
    let mut captured: Option<ProviderInvocation> = None;
    attempt_agent_rescue(
        &RescueTrigger::TerminalFailure(&operation),
        "codex",
        &home,
        &runtime,
        |invocation| {
            captured = Some(invocation);
            Ok(String::new())
        },
    )
    .unwrap();
    let invocation = captured.unwrap();
    assert!(invocation.prompt.contains("terminal_failure"));
    let context_path = invocation
        .prompt
        .split_whitespace()
        .find(|token| token.contains("update-rescue-context-"))
        .map(|token| token.trim_end_matches('.').to_string())
        .unwrap();
    let context: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&context_path).unwrap()).unwrap();
    assert_eq!(context["kind"], "terminal_failure");
    assert_eq!(context["operation"]["id"], "source-test-operation");
    assert_eq!(context["operation"]["error"], "cargo build failed");
    assert_eq!(
        context["operation"]["stashed_changes"],
        "stashsha (refine-update-2026-01-01T00-00-00Z)"
    );
    fs::remove_dir_all(&home).unwrap();
}

#[test]
fn rescue_invocation_failure_propagates() {
    let home = test_directory("system-update-rescue-error");
    let runtime = home.join("run/8082");
    let source = snapshot(&home);
    let error = attempt_agent_rescue(
        &RescueTrigger::Divergence(&source),
        "claude",
        &home,
        &runtime,
        |_invocation| Err(RefineError::Degraded("provider crashed".to_string())),
    )
    .unwrap_err();
    assert!(error.to_string().contains("provider crashed"));
    fs::remove_dir_all(&home).unwrap();
}

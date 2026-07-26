use super::*;

#[test]
fn static_plan_mode_uses_managed_terminal_with_initial_context() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();

    assert!(toolbar.contains("INTERACTIVE_TERMINAL_MODES"));
    assert!(toolbar.contains(r#"profile: tab.mode"#));
    assert!(toolbar.contains(r#"initial_prompt: tab.initialPrompt"#));
    assert!(toolbar.contains(r#"data-testid="terminal-start""#));
    assert!(toolbar.contains(r#"data-testid="terminal-stop""#));
    assert!(toolbar.contains("async function activateToolbarTab"));
    assert!(toolbar.contains("if (shouldStart) await startTerminalSession(tab)"));
    assert!(!toolbar.contains("renderChatPanel"));
    assert!(!toolbar.contains("/api/chat/start"));
}

#[test]
fn static_goal_log_tail_uses_toolbar_and_shared_sse_activity() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();
    let goal_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();
    let common = fs::read_to_string(static_root.join("js/common.js")).unwrap();
    let toolbar_css = fs::read_to_string(static_root.join("css/toolbar.css")).unwrap();

    assert!(goal_detail.contains(r#"data-testid="goal-action-watch-logs""#));
    assert!(goal_detail.contains("openGoalLogTail({ goalId: liveGoal().id"));
    assert!(toolbar.contains("function openGoalLogTail"));
    assert!(toolbar.contains("function loadGoalLogTail"));
    assert!(toolbar.contains("/api/activity?${params}"));
    assert!(toolbar.contains(r#"dir: "desc""#));
    assert!(toolbar.contains(r#"role="log" aria-live="polite""#));
    assert!(toolbar.contains("function handleGoalLogSseEvent"));
    assert!(common.contains(r#"addEventListener("goal_log_added""#));
    assert!(toolbar_css.contains(".goal-log-tail"));
}

#[test]
fn static_toolbar_is_lazy_multi_agent_and_uses_shared_managed_terminal() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();
    let toolbar_css = fs::read_to_string(static_root.join("css/toolbar.css")).unwrap();

    assert!(toolbar.contains("CHAT_TABS_STORAGE_VERSION = 2"));
    assert!(toolbar.contains("function toolbarStateStorage()"));
    assert!(toolbar.contains(r#"typeof sessionStorage === "undefined""#));
    assert!(toolbar.contains(r#"["agent", "Agent"]"#));
    assert!(toolbar.contains(r#"["standalone", "Agent in Worktree"]"#));
    assert!(toolbar.contains(r#"["todo", "Todo List"]"#));
    assert!(toolbar.contains(r#"["plan", "Planing Agent"]"#));
    assert!(toolbar.contains("function createToolbarTab"));
    assert!(!toolbar.contains("ensureSupervisorTab"));
    assert!(toolbar.contains(r#"api("POST", "/api/terminal/session"#));
    assert!(toolbar.contains("toolbarTabUsesTerminal(active)"));
    assert!(toolbar.contains("renderTerminalPanel(active)"));
    assert!(!toolbar.contains("renderSupervisorPanel"));
    assert!(!toolbar.contains("renderChatPanel"));
    assert!(!toolbar.contains(r#"data-testid="supervisor-agent-conversation""#));
    assert!(!toolbar_css.contains(".supervisor-agent-summary"));
    assert!(!toolbar_css.contains(".chat-input-wrap"));
    assert!(toolbar_css.contains(".terminal-panel"));
    assert!(toolbar_css.contains("position: absolute"));
    assert!(toolbar_css.contains(".toolbar-dock:not(.open) .toolbar-add-options"));
    assert!(toolbar_css.contains("min-height: 36px"));
    assert!(toolbar_css.contains("padding-inline: 0"));
    assert!(toolbar_css.contains("font-size: 15px"));
    assert!(toolbar.contains("observeTerminalOutputSize(output, liveTab())"));
    assert!(toolbar.contains("scheduleActiveTerminalFit()"));
}

use super::*;

#[test]
fn static_goal_detail_opens_the_workflow_agent_instead_of_goal_chat() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let goal_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();

    assert!(goal_detail.contains(r#"data-testid="goal-open-agent""#));
    assert!(goal_detail.contains("Open Agent"));
    assert!(goal_detail.contains("openAgentDock({ goalId: liveGoal().id"));
    assert!(toolbar.contains("function openAgentDock"));
    assert!(!goal_detail.contains("goal-open-chat"));
    assert!(!toolbar.contains("openChatDock"));
}

#[test]
fn static_work_item_tables_use_shared_readable_name_layout() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let common_css = fs::read_to_string(static_root.join("css/common.css")).unwrap();
    let goals_css = fs::read_to_string(static_root.join("css/goals.css")).unwrap();
    let goals_list = fs::read_to_string(static_root.join("js/features/goals-list.js")).unwrap();
    let features = fs::read_to_string(static_root.join("js/features/features.js")).unwrap();

    assert!(common_css.contains(".work-items-table"));
    assert!(common_css.contains(".work-item-name-col"));
    assert!(common_css.contains(".work-item-name-cell"));
    assert!(!common_css.contains(".table-scroll {\n  max-width: 100%;\n  overflow-x: auto;"));
    assert!(!common_css.contains("min-width: var(--work-items-table-min-width"));
    assert!(common_css.contains("overflow-wrap: break-word"));
    assert!(common_css.contains("word-break: normal"));
    assert!(common_css.contains("width: var(--work-item-select-width, 4%)"));

    assert_eq!(goals_css.matches("--work-item-name-width: 20%").count(), 2);
    assert!(goals_css.contains("--work-item-select-width: 4%"));
    assert!(goals_css.contains(".features-col-next {\n  width: 17%;"));
    assert!(goals_css.contains(".features-col-updated {\n  width: 9%;"));
    assert!(!goals_css.contains(".features-name-cell {\n  overflow-wrap: anywhere;"));

    for source in [goals_list.as_str(), features.as_str()] {
        assert!(source.contains("work-items-table"));
        assert!(source.contains("work-item-name-col"));
        assert!(source.contains("work-item-name-cell"));
    }
}

#[test]
fn static_goal_detail_logs_feature_blocking_notice_to_system() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let goals_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();

    assert!(goals_detail.contains("feature_blocking_notice"));
    assert!(goals_detail.contains(r#"data-testid="goal-feature-blocking-banner""#));
    assert!(goals_detail.contains("function recordFeatureBlockingNotice"));
    assert!(goals_detail.contains("recordUiNotice(notice.message"));
    assert!(goals_detail.contains(r#"source: "workflow""#));
}

#[test]
fn static_goal_detail_uses_shared_governance_review_state_helpers() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let common = fs::read_to_string(static_root.join("js/common.js")).unwrap();
    let goals_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();

    assert!(common.contains("function governanceReviewStatus"));
    assert!(common.contains(r#""pass", "passed""#));
    assert!(common.contains("function reviewStateClass"));
    assert!(goals_detail.contains("governanceReviewStatus(round)"));
    assert!(goals_detail.contains("governanceReviewStatus(latest)"));
    assert!(goals_detail.contains("reviewStateClass(states.product)"));
    assert!(goals_detail.contains("reviewStateClass(states.constitution)"));
    assert!(!goals_detail.contains(r#"product_state === "pass""#));
    assert!(!goals_detail.contains(r#"constitution_state === "pass""#));
}

#[test]
fn static_goal_reports_and_bulk_jira_export_use_the_correct_surfaces() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let common = fs::read_to_string(static_root.join("js/common.js")).unwrap();
    let goals_detail = fs::read_to_string(static_root.join("js/features/goals-detail.js")).unwrap();
    let goals_list = fs::read_to_string(static_root.join("js/features/goals-list.js")).unwrap();
    let goals_bulk = fs::read_to_string(static_root.join("js/features/goals-bulk.js")).unwrap();
    let commands = fs::read_to_string(static_root.join("js/commands.js")).unwrap();

    assert!(goals_detail.contains("rnd.implementation_report"));
    assert!(goals_detail.contains(r#"data-testid="goal-implementation-report""#));
    assert!(goals_detail.contains(r#"data-testid="goal-implementation-report-body""#));
    assert!(goals_detail.contains("rnd.implementation_reported_at"));
    assert!(!goals_detail.contains(r#"data-testid="goal-action-export-jira""#));
    assert!(!goals_detail.contains("/export/jira"));
    assert!(goals_list.contains(r#"data-testid="goals-bulk-export-jira""#));
    assert!(goals_list.contains(r##"bindCommand("#bulk-export-jira", "goals.bulk.export_jira")"##));
    assert!(goals_bulk.contains("function exportSelectedGoalsForJira"));
    assert!(goals_bulk.contains(r#"api("POST", "/api/goals/export/jira""#));
    assert!(goals_bulk.contains("..._selectionRequestFields()"));
    assert!(goals_bulk.contains("waitForGoalsJiraExportOperation"));
    assert!(goals_bulk.contains("GOALS_JIRA_EXPORT_OPERATION_KEY"));
    assert!(goals_bulk.contains("/retry`"));
    assert!(goals_list.contains(r#"data-testid="goals-jira-export-operation""#));
    assert!(goals_bulk.contains(r#"data-testid="goals-jira-export-status""#));
    assert!(goals_bulk.contains(r#"data-testid="goals-jira-export-progress""#));
    assert!(goals_bulk.contains(r#"data-testid="goals-jira-export-logs""#));
    assert!(goals_bulk.contains(r#"data-testid="goals-jira-export-cancel""#));
    assert!(goals_bulk.contains(r#"data-testid="goals-jira-export-hide""#));
    assert!(goals_bulk.contains(r#"data-testid="goals-jira-export-download""#));
    assert!(goals_bulk.contains("/api/operations/${encodeURIComponent(operationId)}/cancel"));
    assert!(goals_list.contains("syncGoalsJiraExportOperation()"));
    assert!(common.contains(r#"err.code = "operation_interrupted""#));
    assert!(commands.contains(r#"id: "goals.bulk.export_jira""#));
}

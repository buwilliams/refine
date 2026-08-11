use super::*;

#[test]
fn web_server_route_groups_cover_static_web_surface() {
    let groups = API_GROUPS
        .iter()
        .map(|group| group.prefix)
        .collect::<std::collections::BTreeSet<_>>();
    for prefix in [
        "/activity",
        "/agents",
        "/apps",
        "/cache",
        "/changes",
        "/chat",
        "/cluster",
        "/dashboard",
        "/diagnostics",
        "/events",
        "/files",
        "/governance",
        "/guidance",
        "/import",
        "/operations",
        "/nodes",
        "/performance",
        "/processes",
        "/project",
        "/quality",
        "/reporters",
        "/runner-workers",
        "/settings",
        "/system",
        "/target-app",
        "/todos",
        "/upgrade",
        "/work",
        "/workflow",
    ] {
        assert!(groups.contains(prefix), "missing route group {prefix}");
    }

    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static/js");
    let guide = fs::read_to_string(static_root.join("features/guide.js")).unwrap();
    let guide_ids = extract_prefixed_string_literals(&guide, "guideItem(\"")
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let mut settings_ids = std::collections::BTreeSet::new();
    for entry in fs::read_dir(static_root.join("features")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("settings") || !name.ends_with(".js") {
            continue;
        }
        let source = fs::read_to_string(entry.path()).unwrap();
        settings_ids.extend(extract_settings_guide_label_ids(&source));
        settings_ids.extend(extract_prefixed_string_literals(&source, "guideItemId: \""));
    }
    let missing_ids = settings_ids
        .difference(&guide_ids)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        missing_ids.is_empty(),
        "settings Guide labels without guideItem targets: {missing_ids:?}"
    );

    let guide_hashes = extract_prefixed_string_literals(&guide, "hash: \"");
    let stale_hashes = guide_hashes
        .into_iter()
        .filter(|hash| {
            hash.starts_with("#/system")
                || hash.starts_with("#/settings")
                || hash.starts_with("#/project/application")
                || hash.starts_with("#/node/nodes")
                || hash.contains("application-config")
                || hash.contains("target-app-config")
        })
        .collect::<Vec<_>>();
    assert!(
        stale_hashes.is_empty(),
        "Guide targets point at removed screen locations: {stale_hashes:?}"
    );
}

#[test]
fn static_main_nav_consolidates_context_and_controls() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(static_root.join("index.html")).unwrap();
    let theme = fs::read_to_string(static_root.join("js/theme.js")).unwrap();
    let theme_css = fs::read_to_string(static_root.join("css/theme.css")).unwrap();
    let target_app = fs::read_to_string(static_root.join("js/target-app.js")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();

    let menu_start = index
        .find(r#"<details class="nav-menu nav-context-menu" id="nav-context-menu">"#)
        .expect("controls menu should exist");
    let menu_end = menu_start
        + index[menu_start..]
            .find("</details>")
            .expect("controls menu should close")
        + "</details>".len();
    let menu = &index[menu_start..menu_end];
    let summary_end = menu
        .find("</summary>")
        .expect("controls summary should close");
    let summary = &menu[..summary_end];

    assert!(summary.contains(r#"aria-label="Open controls""#));
    assert!(summary.contains(r#"class="nav-context-icon""#));
    assert!(summary.contains("<span>Controls</span>"));
    assert!(summary.contains(r#"class="nav-context-main""#));
    assert!(summary.contains(r#"class="nav-context-more" aria-hidden="true""#));
    assert!(!summary.contains("target-app-dot"));
    assert!(!summary.contains("context-app-name"));
    assert!(!summary.contains("context-reporter-name"));

    for control_id in [
        r#"id="target-app-indicator""#,
        r#"id="global-reporter""#,
        r#"id="agent-status-indicator""#,
        r#"id="btn-source-update""#,
        r#"id="btn-command-palette""#,
        r#"id="btn-refine-issue""#,
        r#"id="btn-theme-toggle""#,
    ] {
        assert!(
            menu.contains(control_id),
            "{control_id} should be inside the controls menu"
        );
        assert_eq!(
            index.matches(&format!(" {control_id}")).count(),
            1,
            "{control_id} should only appear once"
        );
    }

    assert!(menu.contains(r#"class="nav-control-status target-app-state""#));
    assert!(menu.contains(r#"class="nav-control-status agent-status-label""#));
    assert!(menu.contains(r#"class="nav-control-status nav-source-update-status""#));
    assert!(menu.contains(r#"class="nav-control-status nav-theme-status""#));
    assert!(menu.contains("<span>Report a bug to Refine Devs</span>"));
    assert!(!menu.contains("<span>Report a bug</span>"));
    assert!(menu.contains(r#"aria-pressed="false""#));
    assert!(theme.contains(r#"const STORAGE_KEY = "refine_color_theme""#));
    assert!(theme.contains(r#"new CustomEvent("refine-theme-change""#));
    assert!(theme_css.contains(r#"html[data-theme="dark"]"#));
    assert!(theme_css.contains("color-scheme: dark"));
    let theme_script = index
        .find(r#"<script src="/static/js/theme.js"></script>"#)
        .expect("theme bootstrap should be loaded");
    let base_styles = index
        .find(r#"<link rel="stylesheet" href="/static/css/base.css">"#)
        .expect("base stylesheet should be loaded");
    assert!(
        theme_script < base_styles,
        "theme bootstrap should run before styles paint"
    );
    let management_start = menu
        .find(r#">Management</div>"#)
        .expect("management section should exist");
    let source_update_start = menu
        .find(r#"id="btn-source-update""#)
        .expect("source update control should exist");
    let guide_start = menu
        .find(r#"id="nav-guide-open""#)
        .expect("guide management control should exist");
    assert!(
        management_start < source_update_start && source_update_start < guide_start,
        "source update should be the first management control"
    );
    assert!(menu.contains(
        r#"class="nav-menu-item nav-control-item nav-management-item nav-command-button""#
    ));
    let command_start = menu
        .find(r#"class="nav-menu-item nav-control-item nav-management-item nav-command-button""#)
        .expect("command palette management-style control should exist");
    let command_end = command_start
        + menu[command_start..]
            .find("</button>")
            .expect("command palette control should close");
    assert!(menu[command_start..command_end].contains(r#"class="nav-menu-icon""#));
    assert!(target_app.contains(r#"querySelector(".target-app-state")"#));
    assert!(target_app.contains(r#"`${statusLabel} · ${agentCount}`"#));
    assert!(releases.contains(r#"querySelector(".nav-source-update-status")"#));
}

#[test]
fn static_import_modal_exposes_feature_import_surface() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(static_root.join("index.html")).unwrap();
    let commands = fs::read_to_string(static_root.join("js/commands.js")).unwrap();
    let import_modes = fs::read_to_string(static_root.join("js/features/goals-import.js")).unwrap();
    let import_modal =
        fs::read_to_string(static_root.join("js/features/goals-import-modal.js")).unwrap();
    let import_prepare =
        fs::read_to_string(static_root.join("js/features/goals-import-prepare.js")).unwrap();
    let import_save =
        fs::read_to_string(static_root.join("js/features/goals-import-save.js")).unwrap();

    assert!(index.contains(r#"data-testid="nav-import-goals">Import</a>"#));
    assert!(commands.contains(r#"title: "Import""#));
    assert!(import_modes.contains(r#"mode: "feature""#));
    for label in [
        "Import Feature",
        "Import Goals",
        "Import Goals (.csv)",
        "Upload Goals (.csv)",
    ] {
        assert!(import_modes.contains(label), "missing import label {label}");
    }
    assert!(import_modal.contains(r#"data-testid="import-feature-text""#));
    assert!(import_modes.contains("Extract Feature"));
    assert!(import_modal.contains("startImportExtractOperation(text,"));
    assert!(import_modal.contains("force_provider: true"));
    assert!(import_modal.contains("queueImportPreparation(started.operation, activeMode"));
    assert!(import_modal.contains("startImportCsvParseOperation(csvText"));
    assert!(import_prepare.contains("function planDraftPayloadFromResult"));
    assert!(import_prepare.contains("async function startImportExtractOperation"));
    assert!(import_prepare.contains("async function startImportCsvParseOperation"));
    assert!(import_prepare.contains("async function saveImportDraftReviewState"));
    assert!(import_prepare.contains("async function reviewPlanFeatureDraftPayload"));
    assert!(import_save.contains("const oneBasedIndex ="));
    assert!(import_save.contains("const original = payload[oneBasedIndex - 1]"));
}

#[test]
fn static_system_log_exposes_sources_and_diagnostic_details() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let common = fs::read_to_string(static_root.join("js/common.js")).unwrap();
    let commands = fs::read_to_string(static_root.join("js/commands.js")).unwrap();
    let toolbar = fs::read_to_string(static_root.join("js/features/toolbar.js")).unwrap();
    let toolbar_css = fs::read_to_string(static_root.join("css/toolbar.css")).unwrap();

    assert!(common.contains("if (details) payload.details = details"));
    assert!(common.contains(r#"details: { operation_id: response.operation.id }"#));
    assert!(common.contains("function activitySystemOperationDetails"));
    assert!(common.contains("details.activity_id = entry.id"));
    assert!(common.contains("details.goal_id = entry.goal_id"));
    assert!(common.contains("details: activitySystemOperationDetails(entry)"));
    assert!(commands.contains(r#"details: { operation_id: operationId }"#));
    assert!(commands.contains(r#"details: { operation_id: response.operation.id }"#));
    assert!(toolbar.contains("details: payload?.details ?? null"));
    assert!(toolbar.contains("function systemOperationDetailEntries"));
    assert!(toolbar.contains(r#"data-testid="system-log-status""#));
    assert!(toolbar.contains(r#"data-testid="system-log-category""#));
    assert!(toolbar.contains(r#"data-testid="system-log-details""#));
    assert!(toolbar.contains(r#"data-testid="system-log-detail""#));
    assert!(toolbar.contains("existing.category !== item.category"));
    assert!(toolbar.contains("formatSystemOperationDetails(existing.details) !== itemDetails"));
    assert!(toolbar_css.contains(".system-log-status"));
    assert!(toolbar_css.contains(".system-log-category"));
    assert!(toolbar_css.contains(".system-log-detail dd"));
}

#[test]
fn local_http_daemon_serves_static_assets() {
    let temp_root = unique_temp_dir("static-assets");
    fs::create_dir_all(&temp_root).unwrap();
    fs::write(
        temp_root.join("index.html"),
        "<!doctype html><title>Refine</title>",
    )
    .unwrap();
    fs::create_dir_all(temp_root.join("css")).unwrap();
    fs::write(temp_root.join("css/base.css"), "body { color: black; }").unwrap();
    let daemon = LocalHttpDaemon {
        server: server_with_projection(),
        static_root: Some(temp_root.clone()),
    };
    daemon.recover_runtime_state().unwrap();

    let response = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });

    assert_eq!(response.status, 200);
    assert_eq!(response.content_type, "text/html; charset=utf-8");
    assert!(String::from_utf8(response.body).unwrap().contains("Refine"));

    let css = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/static/css/base.css".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(css.status, 200);
    assert_eq!(css.content_type, "text/css; charset=utf-8");
    assert!(
        String::from_utf8(css.body)
            .unwrap()
            .contains("color: black")
    );

    thread::sleep(Duration::from_millis(10));
    fs::write(temp_root.join("css/base.css"), "body { color: blue; }").unwrap();
    let updated_css = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/static/css/base.css".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(updated_css.status, 200);
    assert!(
        String::from_utf8(updated_css.body)
            .unwrap()
            .contains("color: blue")
    );

    let traversal = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/static/../Cargo.toml".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(traversal.status, 400);
    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_accepts_static_ui_api_aliases_for_work_routes() {
    let temp_root = unique_temp_dir("http-api-aliases");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(temp_root.join("run/8080"));

    let create_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals".to_string(),
        body: Some(json!({"id": "GOAL1", "name": "Goal One"})),
    });
    assert_eq!(create_goal.status, 201);
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Feature One"})),
    });
    assert_eq!(create_feature.status, 201);

    let add_goal = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/GOAL1".to_string(),
        body: None,
    });
    assert_eq!(add_goal.status, 200);
    assert_eq!(add_goal.body["goal_ids"], json!(["GOAL1"]));

    let workflow = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/workflow".to_string(),
        body: Some(json!({"status": "todo"})),
    });
    assert_eq!(workflow.status, 200);
    assert_eq!(workflow.body["rollup"]["status"], "todo");

    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/GOAL1/cancel".to_string(),
        body: None,
    });
    assert_eq!(cancel.status, 200);
    assert_eq!(cancel.body["goal"]["status"], "cancelled");

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_accepts_static_ui_bulk_api_aliases() {
    let temp_root = unique_temp_dir("http-bulk-api-aliases");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(temp_root.join("run/8080"));
    for (id, name) in [
        ("GOAL1", "Bulk One"),
        ("GOAL2", "Bulk Two"),
        ("GOAL3", "Bulk Active"),
    ] {
        let create = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/goals".to_string(),
            body: Some(json!({"id": id, "name": name})),
        });
        assert_eq!(create.status, 201);
    }
    let create_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA1", "name": "Bulk Feature"})),
    });
    assert_eq!(create_feature.status, 201);
    let create_second_feature = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features".to_string(),
        body: Some(json!({"id": "FEA2", "name": "Bulk Feature Two"})),
    });
    assert_eq!(create_second_feature.status, 201);

    let bulk_status = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL1", "GOAL2"],
            "update": {"status": "todo"}
        })),
    });
    assert_eq!(bulk_status.status, 200);
    assert_eq!(bulk_status.body["updated"], 2);

    let bulk_review = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL1", "GOAL2"],
            "update": {"status": "review"}
        })),
    });
    assert_eq!(bulk_review.status, 200);
    assert_eq!(bulk_review.body["updated"], 2);

    let bulk_done = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL1", "GOAL2"],
            "update": {"status": "done"}
        })),
    });
    assert_eq!(bulk_done.status, 200);
    assert_eq!(bulk_done.body["updated"], 2);

    FileWorkItemService::new(&refine_dir)
        .set_goal_status_unchecked("GOAL3", &GoalStatus::InProgress)
        .unwrap();
    let bulk_cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["GOAL1", "GOAL3"],
            "update": {"status": "cancelled"}
        })),
    });
    assert_eq!(bulk_cancel.status, 200);
    assert_eq!(bulk_cancel.body["updated"], 1);
    assert_eq!(bulk_cancel.body["ids"], json!(["GOAL3"]));
    assert_eq!(bulk_cancel.body["skipped"], 1);
    assert_eq!(
        bulk_cancel.body["skipped_details"][0],
        json!({"id": "GOAL1", "reason": "status:done"})
    );
    assert_eq!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary("GOAL3")
            .unwrap()
            .goal
            .status,
        GoalStatus::Cancelled
    );

    let bulk_assign = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/FEA1/goals/bulk".to_string(),
        body: Some(json!({"selected_ids": ["GOAL1", "GOAL2"]})),
    });
    assert_eq!(bulk_assign.status, 200);
    assert_eq!(bulk_assign.body["updated"], 2);
    assert!(
        fs::read_to_string(refine_dir.join("goals/GO/AL1/goal.json"))
            .unwrap()
            .contains("\"feature_id\": \"FEA1\"")
    );

    let bulk_feature_update = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/bulk".to_string(),
        body: Some(json!({
            "selected_ids": ["FEA1", "FEA2"],
            "update": {"reporter": "Feature Reporter"}
        })),
    });
    assert_eq!(bulk_feature_update.status, 200);
    assert_eq!(bulk_feature_update.body["updated"], 2);
    assert!(
        fs::read_to_string(refine_dir.join("features/FE/A2/feature.json"))
            .unwrap()
            .contains("\"reporter\": \"Feature Reporter\"")
    );

    let bulk_feature_delete = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/features/bulk/delete".to_string(),
        body: Some(json!({"selected_ids": ["FEA2"]})),
    });
    assert_eq!(bulk_feature_delete.status, 200);
    assert_eq!(bulk_feature_delete.body["deleted"], 1);
    assert!(!refine_dir.join("features/FE/A2/feature.json").exists());

    let bulk_delete = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/goals/bulk/delete".to_string(),
        body: Some(json!({"selected_ids": ["GOAL1"]})),
    });
    assert_eq!(bulk_delete.status, 200);
    assert_eq!(bulk_delete.body["deleted"], 1);
    assert!(!refine_dir.join("goals/GO/AL1/goal.json").exists());
    assert!(refine_dir.join("goals/GO/AL2/goal.json").exists());

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_records_and_lists_activity_for_static_ui() {
    let temp_root = unique_temp_dir("http-activity");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let recorded = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/activity/ui-error".to_string(),
        body: Some(json!({"message": "Boom", "source": "test"})),
    });
    assert_eq!(recorded.status, 200);
    assert_eq!(recorded.body["recorded"], true);
    assert!(refine_dir.join("logs/activity.jsonl").exists());

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/activity".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["activity"][0]["message"], "Boom");
    assert_eq!(listed.body["facets"]["categories"], json!(["ui"]));

    let filtered = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/activity?q=source&limit=1".to_string(),
        body: None,
    });
    assert_eq!(filtered.status, 200);
    assert_eq!(filtered.body["page"]["limit"], 1);
    assert_eq!(filtered.body["activity"][0]["message"], "Boom");

    remove_temp_dir(&temp_root);
}

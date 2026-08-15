use super::*;

#[test]
fn static_runtime_settings_expose_state_sync_controls() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let runtime = fs::read_to_string(static_root.join("js/features/settings_runtime.js")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();

    assert!(runtime.contains(r#"data-testid="runtime-state-sync-now""#));
    assert!(runtime.contains(r#"data-testid="runtime-state-sync-debounce""#));
    assert!(runtime.contains(r#"data-testid="runtime-project-update-pulse""#));
    assert!(runtime.contains(r#"data-testid="runtime-worktree-cleanup-delay""#));
    assert!(runtime.contains(r#"data-testid="runtime-worktree-cleanup-generated-paths""#));
    assert!(runtime.contains(r#"data-testid="runtime-worktree-cleanup-now""#));
    assert!(runtime.contains(r#"api("POST", "/api/project/sync", {})"#));
    assert!(runtime.contains(r#""/api/project/worktrees/cleanup""#));
    assert!(runtime.contains("resolveBackgroundOperationResponse"));
    assert!(
        runtime.contains(r##"state_sync_debounce_seconds: $("#s-state-sync-debounce").value"##)
    );
    assert!(runtime.contains(
        r##"state_sync_stale_threshold_seconds: $("#s-state-sync-stale-threshold").value"##
    ));
    assert!(runtime.contains("runtime-state-sync-stale-threshold"));
    assert!(runtime.contains(
        r##"project_update_pulse_interval_seconds: $("#s-project-update-pulse").value"##
    ));
    assert!(
        runtime
            .contains(r##"worktree_cleanup_after_seconds: $("#s-worktree-cleanup-delay").value"##)
    );
    assert!(runtime.contains(
        r##"worktree_cleanup_generated_paths: $("#s-worktree-cleanup-generated-paths").value"##
    ));
    assert!(!runtime.contains(r#"data-testid="source-upgrade-section""#));
    assert!(releases.contains(r#"data-testid="source-upgrade-section""#));
    assert!(releases.contains("<h3>Upgrade</h3>"));
    assert!(!releases.contains("Dogfood source"));
    assert!(releases.contains(r#"data-testid="source-promotion-check""#));
    assert!(releases.contains(r#"data-testid="source-promotion-promote""#));
    assert!(releases.contains("/api/system/source/check"));
    assert!(releases.contains("/api/system/source/promote"));
    assert!(releases.contains("Refine is restarting; reconnecting"));
}

#[test]
fn static_main_nav_exposes_refine_source_update_affordance() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(static_root.join("index.html")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();
    let init = fs::read_to_string(static_root.join("js/init.js")).unwrap();

    assert!(index.contains(r#"data-testid="nav-source-update""#));
    assert!(index.contains("hidden disabled"));
    assert!(releases.contains("const sourceUpdate = result.source_update || {}"));
    assert!(releases.contains("button.disabled = sourceUpdate.enabled !== true"));
    assert!(releases.contains(r#"fetchRemote ? "/api/system/source/check""#));
    assert!(releases.contains(r#"api("POST", "/api/system/source/promote", {})"#));
    assert!(releases.contains("Upgrade Refine"));
    assert!(releases.contains("source_check"));
    assert!(!releases.contains("window.confirm("));
    assert!(!releases.contains("hasAttachedProject()"));
    assert!(releases.contains("handleSourcePromotionSseEvent"));
    assert!(releases.contains("handleSourceUpdateCheckSseEvent"));
    assert!(releases.contains("handleSourceUpdateSseEvent"));
    assert!(!releases.contains("setInterval"));
    assert!(init.contains("initSourceUpdateNav()"));
}

#[test]
fn static_project_settings_explain_governance_and_quality_effects() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let governance =
        fs::read_to_string(static_root.join("js/features/settings_governance.js")).unwrap();
    let quality = fs::read_to_string(static_root.join("js/features/settings_quality.js")).unwrap();

    assert!(governance.contains(r#"data-testid="governance-explanation""#));
    assert!(governance.contains("A rule finding can draft a fresh recovery Round"));
    assert!(governance.contains("do not start a check now"));
    assert!(governance.contains("rules_revision:"));
    assert!(governance.contains("saved.rules || []"));
    assert!(governance.contains("e.status === 409"));
    assert!(governance.contains("refreshSettingsTab(\"governance\""));

    let guidance =
        fs::read_to_string(static_root.join("js/features/settings_guidance.js")).unwrap();
    assert!(guidance.contains("/api/guidance/${encodeURIComponent(current.id)}"));
    assert!(guidance.contains("{ ...item, revision }"));
    assert!(guidance.contains("e.status === 409"));
    assert!(guidance.contains("refreshSettingsTab(refreshTab, { force: true })"));
    assert!(!guidance.contains("api(\"PUT\", \"/api/guidance\""));

    assert!(quality.contains(r#"data-testid="quality-explanation""#));
    assert!(quality.contains("Passing checks advance the Goal to Governance"));
    assert!(quality.contains("preserve the candidate"));
    assert!(quality.contains("do not start a run now"));
}

#[test]
fn static_releases_surface_separates_prepare_from_confirmed_publish() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(static_root.join("index.html")).unwrap();
    let settings = fs::read_to_string(static_root.join("js/features/settings.js")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();

    assert!(index.contains("settings_releases.js"));
    let node_tabs = settings
        .split("  node: {")
        .nth(1)
        .and_then(|node| node.split("  project: {").next())
        .expect("Node settings surface");
    assert!(node_tabs.contains(r#"{ slug: "runtime", label: "Runtime Config" }"#));
    assert!(node_tabs.contains(r#"{ slug: "releases", label: "Refine (dev)" }"#));
    assert!(
        node_tabs.find(r#"slug: "runtime""#).unwrap()
            < node_tabs.find(r#"slug: "releases""#).unwrap()
    );
    assert!(
        node_tabs
            .trim_end()
            .ends_with("{ slug: \"releases\", label: \"Refine (dev)\" },\n    ],\n  },")
    );
    assert!(releases.contains(r#"data-testid="release-bump""#));
    assert!(releases.contains(r#"data-testid="release-preview""#));
    assert!(releases.contains(r#"data-testid="release-prepare""#));
    assert!(releases.contains(r#"data-testid="release-publish""#));
    assert!(releases.contains("explicit confirmation"));
    assert!(releases.contains("/api/system/releases/prepare"));
    assert!(releases.contains("/api/system/releases/publish"));
    assert!(releases.contains("/retry"));
}

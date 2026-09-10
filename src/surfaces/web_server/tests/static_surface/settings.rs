use super::*;

#[test]
fn static_runtime_settings_expose_state_sync_controls() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let runtime = fs::read_to_string(static_root.join("js/features/settings_runtime.js")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();

    assert!(runtime.contains(r#"data-testid="runtime-state-sync-now""#));
    assert!(runtime.contains(r#"placeholder="Automatic""#));
    assert!(runtime.contains(r#"valueLabel: parallelRunCap || "Automatic""#));
    assert!(runtime.contains(r#"emptyLabel: "Automatic""#));
    assert!(runtime.contains("Enter a number to override that conservative recommendation"));
    assert!(!runtime.contains("s.parallel_run_cap || 5"));
    assert!(runtime.contains(r#"data-testid="runtime-automatic-resource-budget-percent""#));
    assert!(runtime.contains(r#"min="1" max="100""#));
    assert!(runtime.contains(r#"s.automatic_agent_resource_budget_percent ?? "70""#));
    assert!(runtime.contains(
        r##"automatic_agent_resource_budget_percent: $("#s-automatic-resource-budget-percent").value"##
    ));
    assert!(runtime.contains("leaving the remainder for Docker"));
    assert!(runtime.contains(r#"data-testid="runtime-state-sync-debounce""#));
    assert!(runtime.contains(r#"data-testid="runtime-project-update-pulse""#));
    assert!(runtime.contains(r#"data-testid="runtime-worktree-cleanup-delay""#));
    assert!(!runtime.contains("worktree_cleanup_generated_paths"));
    assert!(runtime.contains(r#"data-testid="runtime-worktree-cleanup-now""#));
    assert!(runtime.contains(r#"api("POST", "/api/sync", {})"#));
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
    assert!(!runtime.contains(r#"data-testid="source-upgrade-section""#));
    assert!(releases.contains(r#"data-testid="source-upgrade-section""#));
    assert!(releases.contains("<h3>Update</h3>"));
    assert!(!releases.contains("checkout has uncommitted changes"));
    assert!(releases.contains(r#"data-testid="source-promotion-stash""#));
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
    assert!(releases.contains("Update Refine"));
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
fn static_settings_replace_retired_editors_with_events_and_skills() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(root.join("index.html")).unwrap();
    let settings = fs::read_to_string(root.join("js/features/settings.js")).unwrap();
    assert!(index.contains("settings_events.js"));
    assert!(settings.contains("slug: \"events\""));
    assert!(settings.contains("slug: \"skills\""));
    for retired in [
        "settings_governance.js",
        "settings_guidance.js",
        "settings_quality.js",
    ] {
        assert!(!index.contains(retired));
    }
}

#[test]
fn static_releases_surface_separates_prepare_from_confirmed_publish() {
    let static_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/surfaces/web/static");
    let index = fs::read_to_string(static_root.join("index.html")).unwrap();
    let settings = fs::read_to_string(static_root.join("js/features/settings.js")).unwrap();
    let releases =
        fs::read_to_string(static_root.join("js/features/settings_releases.js")).unwrap();

    assert!(index.contains("settings_releases.js"));
    let node_tabs = settings.split("const SETTINGS_SURFACES =").nth(1).unwrap();
    assert!(
        node_tabs.find("slug: \"runtime\"").unwrap()
            < node_tabs.find("slug: \"releases\"").unwrap()
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

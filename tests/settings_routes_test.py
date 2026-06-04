"""Static checks for Settings tab deep-link routes."""
from __future__ import annotations

import re
import sys
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    settings_core_js = (root / "refine_ui/static/js/features/settings.js").read_text(
        encoding="utf-8",
    )
    settings_tab_files = {
        name: (root / f"refine_ui/static/js/features/{name}.js").read_text(
            encoding="utf-8",
        )
        for name in (
            "settings_application",
            "settings_processes",
            "settings_guidance",
            "settings_runtime",
            "settings_performance",
            "settings_governance",
            "settings_quality",
            "settings_nodes",
            "settings_reporters",
        )
    }
    settings_js = settings_core_js + "\n".join(settings_tab_files.values())
    settings_ops_py = (root / "refine_server/settings_ops.py").read_text(encoding="utf-8")
    dashboard_ops_py = (root / "refine_server/dashboard_ops.py").read_text(encoding="utf-8")
    router_js = (root / "refine_ui/static/js/router.js").read_text(
        encoding="utf-8",
    )
    target_app_js = (root / "refine_ui/static/js/target-app.js").read_text(
        encoding="utf-8",
    )
    common_js = (root / "refine_ui/static/js/common.js").read_text(
        encoding="utf-8",
    )
    commands_js = (root / "refine_ui/static/js/commands.js").read_text(
        encoding="utf-8",
    )
    toolbar_js = (root / "refine_ui/static/js/features/toolbar.js").read_text(
        encoding="utf-8",
    )
    guide_js = (root / "refine_ui/static/js/features/guide.js").read_text(
        encoding="utf-8",
    )
    import_js = (root / "refine_ui/static/js/features/gaps-import.js").read_text(
        encoding="utf-8",
    )
    dashboard_js = (root / "refine_ui/static/js/features/dashboard.js").read_text(
        encoding="utf-8",
    )
    gaps_list_js = (root / "refine_ui/static/js/features/gaps-list.js").read_text(
        encoding="utf-8",
    )
    gaps_css = (root / "refine_ui/static/css/gaps.css").read_text(
        encoding="utf-8",
    )
    modals_css = (root / "refine_ui/static/css/modals.css").read_text(
        encoding="utf-8",
    )
    gaps_bulk_js = (root / "refine_ui/static/js/features/gaps-bulk.js").read_text(
        encoding="utf-8",
    )
    gaps_detail_js = (root / "refine_ui/static/js/features/gaps-detail.js").read_text(
        encoding="utf-8",
    )
    features_js = (root / "refine_ui/static/js/features/features.js").read_text(
        encoding="utf-8",
    )
    changes_js = (root / "refine_ui/static/js/features/changes.js").read_text(
        encoding="utf-8",
    )
    logs_js = (root / "refine_ui/static/js/features/logs.js").read_text(
        encoding="utf-8",
    )
    index_html = (root / "refine_ui/static/index.html").read_text(
        encoding="utf-8",
    )
    common_css = (root / "refine_ui/static/css/common.css").read_text(
        encoding="utf-8",
    )
    api_py = (root / "refine_ui/api.py").read_text(encoding="utf-8")
    server_py = (root / "refine_ui/server.py").read_text(encoding="utf-8")
    sse_py = (root / "refine_ui/sse.py").read_text(encoding="utf-8")
    system_events_py = (root / "refine_ui/system_events.py").read_text(encoding="utf-8")
    runner_py = (root / "refine_server/runner.py").read_text(encoding="utf-8")
    project_state_py = (root / "refine_server/project_state.py").read_text(
        encoding="utf-8",
    )
    dashboard_css = (root / "refine_ui/static/css/dashboard.css").read_text(
        encoding="utf-8",
    )
    base_css = (root / "refine_ui/static/css/base.css").read_text(
        encoding="utf-8",
    )
    toolbar_css = (root / "refine_ui/static/css/toolbar.css").read_text(
        encoding="utf-8",
    )
    guide_css = (root / "refine_ui/static/css/guide.css").read_text(
        encoding="utf-8",
    )

    assert "const SETTINGS_SURFACES = {" in settings_js
    assert 'basePath: "#/system"' in settings_js
    assert 'basePath: "#/node"' in settings_js
    assert 'basePath: "#/project"' in settings_js
    assert '{ slug: "processes", label: "Processes" }' in settings_js
    assert '{ slug: "performance", label: "Performance" }' in settings_js
    assert '{ slug: "nodes", label: "Nodes" }' in settings_js
    assert '{ slug: "reporters", label: "Reporters" }' in settings_js
    assert '{ slug: "application", label: "Application" }' in settings_js
    assert '{ slug: "quality", label: "Quality" }' in settings_js
    assert '{ slug: "governance", label: "Governance" }' in settings_js
    assert '{ slug: "guidance", label: "Guidance" }' in settings_js
    system_surface = settings_js.split('settings: {', 1)[1].split('node: {', 1)[0]
    assert '{ slug: "runtime", label: "Runtime" }' not in system_surface
    slugs = [
        "processes", "performance", "runtime", "nodes", "reporters",
        "application", "quality", "governance", "guidance",
    ]

    assert 'return { route: "settings", tab: parts[1] || null };' in router_js
    assert 'return { route: "node", tab: parts[1] || null };' in router_js
    assert 'return { route: "project", tab: parts[1] || null };' in router_js
    assert 'gaps_plan: renderGapPlan' in router_js
    assert 'if (parts[1] === "plan") return { route: "gaps_plan" };' in router_js
    assert 'r.route === "settings" || r.route === "node" || r.route === "project"' in router_js
    assert "refreshSettingsTab(slug).catch(showActionError);" in router_js
    assert 'parsed.route === state.currentRoute && !parsed.tab' in settings_js
    assert 'return first;' in settings_js
    assert 'if (slug === "system") return "processes";' in settings_js
    assert 'if (slug === "agents") return "processes";' in settings_js
    assert 'if (slug === "project") return surface.tabs[0]?.slug || null;' in settings_js
    assert "function activeSettingsTabFromRoute(surface = settingsSurfaceForRoute())" in settings_js
    assert "let _targetAppDraftDirty = false;" in settings_js
    assert "async function renderNodeSettings()" in settings_js
    assert "async function renderProjectSettings()" in settings_js
    assert "async function refreshSettings(options = {})" in settings_js
    assert "async function refreshActiveSettingsTab(options = {})" in settings_js
    assert "function updateSettingsTabContent(slug, body, bind)" in settings_js
    assert "if (card.innerHTML === next.innerHTML) return;" in settings_js
    assert "function reconcileSettingsNode(current, next)" in settings_js
    assert "_targetAppDraftDirty &&" in settings_js
    assert 'document.querySelector(\'[data-tab-pane="application"].active\')' in settings_js
    assert 'href="${surface.basePath}/${t.slug}"' in settings_js
    assert "<button class=\"settings-tab" not in settings_js
    assert '<div class="card settings-tab-card">${body}</div>' in settings_js
    assert 'input[type="text"], input[type="number"], input[type="url"], textarea, select' in common_css
    assert settings_js.count('<div class="card') == 1
    save_button_ids = re.findall(
        r'<button id="([^"]+)">Save[^<]*</button>',
        settings_js,
    )
    assert save_button_ids == [], save_button_ids
    assert "function createSettingsAutosave" in settings_js
    assert "function bindSettingsAutosave" in settings_js
    assert "function revertSettingsAutosaveValues" in settings_js
    assert 'await modalAlert(' in settings_js
    assert '{ title: "Save failed" }' in settings_js
    assert "The fields were restored to the last saved values." in settings_js
    assert "await refreshActiveSettingsTab({ force: true });" in settings_js
    assert 'id="s-project-update-pulse"' in settings_js
    assert "project_update_pulse_interval_seconds" in settings_js
    assert 'id="s-file-browser-ignore"' in settings_js
    assert "file_browser_ignore_patterns" in settings_js
    assert "node_modules, .git, .refine, run" in settings_js
    assert 'id="s-worker-memory"' in settings_js
    assert 'id="s-ui-memory"' in settings_js
    assert 'id="s-worker-memory" min="0" value="${s.worker_memory_limit_mb ?? 2000}"' in settings_js
    assert 'id="s-ui-memory" min="0" value="${s.ui_memory_limit_mb ?? 2000}"' in settings_js
    assert 'id="s-worker-cpu-priority"' in settings_js
    assert 'id="s-cap-resource-label"' not in settings_js
    assert "function workerResourceBudgetLabel" not in settings_js
    assert 'id="s-resource-isolation"' in settings_js
    assert '["very_low", "Very low"]' in settings_js
    assert '["best_effort", "Best effort"]' in settings_js
    assert 'api("POST", "/api/cache/rebuild", { background: true })' in commands_js
    assert "function drawSqliteCacheProgress" in settings_js
    assert "onProgress: drawSqliteCacheProgress" in commands_js
    assert 'slug: "performance"' in settings_js
    assert 'api("GET", typeof performanceApiPath === "function"' in settings_js
    assert 'slug: "processes"' in settings_js
    assert 'api("GET", "/api/processes")' in settings_js
    assert '@route("GET", r"/api/processes")' in server_py
    assert '@route("POST", r"/api/processes/background")' in server_py
    assert '@route("POST", r"/api/processes/agents")' in server_py
    assert "def process_summary" in api_py
    assert "def set_background_processes" in api_py
    assert "def set_agent_processes" in api_py
    assert "def _system_operation(label: str)" in api_py
    for label in (
        "Start app",
        "Stop app",
        "Rebuild app",
        "Check app",
        "Sync app",
        "Transfer Gaps between nodes",
        "Bulk assign Gaps to Feature",
        "Bulk update Gaps",
        "Bulk delete Gaps",
        "Import Gaps",
        "Extract Gaps from import text",
        "Prepare CSV import",
        "Deduplicate import drafts",
        "Pause or unpause agents",
        "Clean up logs",
        "Attach or switch app",
        "Remove app",
        "Create scaffold Gap",
        "Retry Gap merge",
        "Merge reporters",
        "Run quality regressions",
        "Generate governance rules",
        "Recheck provider auth",
        "Generate app configuration",
        "Hard reset worktree",
        "Clean up performance data",
        "Cancel background job",
    ):
        assert f'@_system_operation("{label}")' in api_py
    process_ops_py = (root / "refine_server/process_ops.py").read_text(encoding="utf-8")
    assert "process_ops.set_background_processes(" in api_py
    assert "process_ops.set_agent_processes(" in api_py
    assert "M_BACKGROUND_PROCESSES_SET" in process_ops_py
    assert "M_ENFORCE_SCHEDULING" in process_ops_py
    assert "def _background_processes_stopped_response" in api_py
    assert "background_processes_stopped" in api_py
    assert 'allow_busy_when=lambda _owner: _background_processes_stopped()' in api_py
    activate_node_body = api_py.split("def activate_node", 1)[1].split("\n@_system_operation", 1)[0]
    assert "runtime.refresh_local_node" in activate_node_body
    assert "runtime.stop_runner()" in activate_node_body
    assert activate_node_body.index("runtime.stop_runner()") < activate_node_body.index("M_CHAT_RESET_ALL")
    assert "function renderProcessesTab" in settings_js
    assert "function renderRunnerWorkRow" in settings_js
    assert "function renderRuntimeAgentCards" not in settings_js
    assert 'api("POST", "/api/performance/cleanup"' in settings_js
    assert '@route("GET", r"/api/performance")' in server_py
    assert 'limit=int(_get_one(q, "limit", "50"))' in server_py
    assert 'offset=int(_get_one(q, "offset", "0"))' in server_py
    assert '@route("POST", r"/api/performance/cleanup")' in server_py
    assert "def performance_summary" in api_py
    assert "offset: int = 0" in api_py
    assert "offset=offset" in api_py
    assert "observability_ops.performance_summary(" in api_py
    assert "backend=runtime.backend_info()" in api_py
    assert "def backend_info" in (root / "refine_ui/runtime.py").read_text(encoding="utf-8")
    assert "function backendProcessLabel" in settings_js
    assert "function performanceResourceLabel" in settings_js
    assert "const PERFORMANCE_DEFAULT_LIMIT = 50;" in settings_js
    assert "function performanceFiltersFromHash()" in settings_js
    assert "function performanceHashFromFilters(f)" in settings_js
    assert "function performanceApiPath" in settings_js
    assert 'renderPaginationControls("performance"' in settings_js
    assert 'bindPaginationControls(root, "performance"' in settings_js
    assert 'id="performance-filter-shell"' in settings_js
    assert 'id="performance-filter-clear"' in settings_js
    assert 'params.set("offset", String((f.page - 1) * f.limit));' in settings_js
    assert "<dt>Process model</dt>" in settings_js
    assert "<th>Resource</th>" in settings_js
    assert "function drawRuntimeRecovery(error)" in settings_js
    assert '@route("POST", r"/api/cache/rebuild")' in server_py
    assert "def rebuild_sqlite_cache" in api_py
    assert 'errorTitle: "SQLite cache rebuild failed"' in commands_js
    assert 'await showActionError(e, "Target app action failed");' in target_app_js
    assert 'No rebuild command is configured. Queue the stop/start rebuild sequence anyway?' in target_app_js
    assert 'Refine will stop, rebuild, and start the app on the host.' in target_app_js
    assert 'Target application rebuild queued' in target_app_js
    assert 'id="s-agent-limit-pause"' in settings_js
    assert "agent_limit_pause_seconds" in settings_js
    assert '"30",    "30 seconds"' in settings_js
    assert '"60",    "1 minute"' in settings_js
    assert '"3600",  "1 hour"' in settings_js
    assert '"10800", "3 hours"' in settings_js
    assert "project_update_pulse_interval_seconds" in settings_ops_py
    assert "file_browser_ignore_patterns" in settings_ops_py
    assert '${cliOption("copilot", "GitHub Copilot")}' in settings_js
    assert '${cliOption("smoke-ai", "Smoke AI (deterministic testing)")}' in settings_js
    assert "REFINE_SMOKE_AI_PATH" in settings_js
    assert '"copilot": "copilot login"' in dashboard_ops_py
    assert '"smoke-ai": "REFINE_SMOKE_AI_PATH"' in dashboard_ops_py
    assert 'id="runtime-upgrade-banner"' in settings_js
    assert 'class="settings-release-status" aria-live="polite"' in settings_core_js
    assert 'id="runtime-upgrade-banner"' not in settings_tab_files["settings_processes"]
    assert "bindRuntimeUpgradeBanner();" in settings_core_js
    assert "refreshRuntimeUpgradeBanner();" in settings_core_js
    assert 'api("GET", "/api/upgrade")' in settings_js
    assert "function renderRuntimeUpgradeBanner" in settings_js
    assert "Upgrade available ${htmlEscape(latest || current)}" in settings_js
    assert "Running latest ${htmlEscape(current)}" in settings_js
    assert "Running latest ${htmlEscape(latest)}" in settings_js
    assert "Local development checkout" not in settings_js
    assert "Version status unavailable" not in settings_js
    assert 'const command = "./r update";' in settings_js
    assert "data-runtime-copy-upgrade" in settings_js
    assert "function copyRuntimeUpgradeCommand" in settings_js
    assert "navigator.clipboard.writeText" in settings_js
    assert ".settings-tabs-row" in common_css
    assert ".settings-release-status" in common_css
    assert ".runtime-version-status" in common_css
    assert ".runtime-version-status-upgrade" in common_css
    assert ".runtime-upgrade-command" not in common_css
    assert ".runtime-copy-upgrade-command" in common_css
    assert '@route("GET", r"/api/upgrade")' in server_py
    assert "def upgrade_status" in api_py
    runtime_save_body = settings_js.split("async function autosaveSettingsRuntime", 1)[1]
    runtime_save_body = runtime_save_body.split("\nfunction bindNodeRuntimeConfigControls", 1)[0]
    application_save_body = settings_js.split("async function autosaveSettingsApplication", 1)[1]
    application_save_body = application_save_body.split("\nfunction applyGeneratedTargetAppConfig", 1)[0]
    assert 'worker_memory_limit_mb: $("#s-worker-memory").value' in runtime_save_body
    assert 'ui_memory_limit_mb: $("#s-ui-memory").value' in runtime_save_body
    assert 'worker_cpu_priority: $("#s-worker-cpu-priority").value' in runtime_save_body
    assert 'resource_isolation_mode: $("#s-resource-isolation").value' in runtime_save_body
    assert 'agent_limit_pause_seconds: $("#s-agent-limit-pause").value' in runtime_save_body
    assert "/api/features" not in settings_js
    assert "Feature flags" not in settings_js
    assert "data-feature-cell" not in settings_js
    assert "featureEnabled(" not in common_js
    assert "refreshFeatures" not in common_js
    assert '@route("GET", r"/api/features")' in server_py
    assert '@route("POST", r"/api/features")' in server_py
    assert '@route("POST", r"/api/features/([0-9A-Za-z]{26})/cancel")' in server_py
    assert '@route("DELETE", r"/api/features/([0-9A-Za-z]{26})")' in server_py
    assert 'r"/api/features/([0-9A-Za-z]{26})"' in server_py
    assert 'r"/api/features/([0-9A-Za-z]{26})/gaps/([0-9A-Za-z]{26})"' in server_py
    assert 'r"/api/features/([0-9A-Za-z]{26})/gaps/bulk"' in server_py
    assert "def list_features" in api_py
    assert "def create_feature" in api_py
    assert "def cancel_feature" in api_py
    assert "def delete_feature" in api_py
    assert "def bulk_assign_feature_gaps" in api_py
    assert "feature_ops.list_features" in api_py
    assert "feature_ops.assign_gap" in api_py
    assert "feature_ops.bulk_assign_gaps" in api_py
    assert "feature_ops.cancel_feature" in api_py
    assert "feature_ops.delete_feature" in api_py
    assert "features: renderFeaturesList" in router_js
    assert "features_detail: renderFeatureDetail" not in router_js
    assert 'if (r.route === "features_detail")' in router_js
    assert "openFeatureDetailModal(r.id)" in router_js
    assert "const fromFeatureDetail" in router_js
    assert "const fromGapDetail" in router_js
    assert "if (!fromFeatureDetail && !fromGapDetail)" in router_js
    features_detail_router_body = router_js.split('if (r.route === "features_detail")', 1)[1].split("return;", 1)[0]
    assert "closeGapDetailModal({ navigateAway: false })" in features_detail_router_body
    assert "closeFeatureModal({ navigateAway: false })" in features_detail_router_body
    assert features_detail_router_body.index("closeGapDetailModal") < features_detail_router_body.index("openFeatureDetailModal")
    assert features_detail_router_body.index("closeFeatureModal") < features_detail_router_body.index("openFeatureDetailModal")
    assert "closeFeatureModal({ navigateAway: false })" in router_js
    assert "features_new: renderFeatureNew" in router_js
    assert 'if (parts[0] === "features")' in router_js
    assert '<a href="#/features" data-route="features">Features</a>' in index_html
    assert 'id="btn-new-feature">New Feature</a>' in index_html
    assert index_html.index('data-route="dashboard">Dashboard') < index_html.index('data-route="features">Features')
    assert index_html.index('data-route="features">Features') < index_html.index('data-route="gaps">Gaps')
    assert index_html.index('data-route="gaps">Gaps') < index_html.index('data-route="changes">Changes')
    assert index_html.index('data-route="changes">Changes') < index_html.index('data-route="logs">Logs')
    assert '<script src="/static/js/features/features.js"></script>' in index_html
    assert "function renderFeatureDetail" in features_js
    assert "function openFeatureDetailModal" in features_js
    feature_detail_modal_body = features_js.split("async function openFeatureDetailModal", 1)[1].split("\nfunction closeFeatureModal", 1)[0]
    assert 'typeof closeGapDetailModal === "function"' in feature_detail_modal_body
    assert "closeGapDetailModal({ navigateAway: false })" in feature_detail_modal_body
    assert "function openFeatureModal" in features_js
    assert "function closeFeatureModal" in features_js
    assert "ensureFeatureModalUnderlay" in features_js
    assert 'openFeatureModal(data.feature, { navigateAway: true })' in features_js
    assert 'class="modal feature-modal"' in features_js
    assert 'feature ? "feature-detail-modal" : "feature-create-modal"' in features_js
    assert 'class="modal-close"' in features_js
    assert 'id="features-new"' not in features_js
    assert 'id="feature-reporter"' not in features_js
    assert "function bindFeatureAutosave" in features_js
    assert "Feature autosave failed" in features_js
    assert 'root.querySelector("[data-ok]")?.addEventListener("click", async () => {' in features_js
    assert "reporter: state.lastReporter || \"\"" in features_js
    assert "reporter: feature.reporter || \"\"" in features_js
    assert "data-cancel>Close" not in features_js
    assert "data-ok>Save" not in features_js
    assert "feature-modal-head" in features_js
    assert "feature-modal-top-actions" in features_js
    assert "feature-modal-meta muted small" in features_js
    assert "feature-modal-gap-heading" in features_js
    assert "feature-gap-heading-actions" in features_js
    assert "feature-gap-add-btn" in features_js
    assert 'data-feature-new-gap aria-label="New Gap" title="New Gap">+</button>' in features_js
    assert "data-feature-assign-gap" not in features_js
    assert "Assign existing" not in features_js
    assert "ID <code>${htmlEscape(feature.id)}</code>" in features_js
    assert 'node <span title="${htmlEscape(nodeOwnerTitle)}">${htmlEscape(nodeDisplayName)}</span>' in features_js
    assert "Create the Feature before adding ordered Gaps." not in features_js
    assert "function bindFeatureGapActions" in features_js
    assert "function bindFeatureGapDragReorder" in features_js
    assert "function featureGapActionIcon" in features_js
    assert "openFeatureAssignGapModal" not in features_js
    assert "openFeatureNewGapFlow" in features_js
    assert "cancelFeatureFromUi" in features_js
    assert "deleteFeatureFromUi" in features_js
    assert "moveFeatureWorkflowFromUi" in features_js
    assert 'data-feature-workflow="backlog"' in features_js
    assert 'data-feature-workflow="todo"' in features_js
    assert "FEATURE_WORKFLOW_PROTECTED_STATUSES" in features_js
    assert 'api("POST", `/api/features/${encodeURIComponent(featureId)}/workflow`' in features_js
    assert "data-feature-move" in features_js
    assert "data-feature-remove-gap" not in features_js
    assert "data-feature-delete-gap" in features_js
    assert "data-feature-drag-gap" in features_js
    assert 'draggable="true"' in features_js
    assert "feature-gap-drag-handle" in features_js
    assert "feature-gap-icon-btn" in features_js
    assert 'featureGapActionIcon("chevron-up")' in features_js
    assert 'featureGapActionIcon("chevron-down")' in features_js
    assert 'featureGapActionIcon("trash")' in features_js
    assert ">Up</button>" not in features_js
    assert ">Down</button>" not in features_js
    assert ">Remove</button>" not in features_js
    assert "[position]: targetGapId" in features_js
    assert 'api("DELETE", `/api/gaps/${encodeURIComponent(gapId)}`' in features_js
    assert "Delete this Gap from the Feature? This cannot be undone." in features_js
    assert 'location.hash = `#/features/${encodeURIComponent(row.dataset.featureId)}`;' in features_js
    assert '<td class="features-name-cell" data-label="Name">${htmlEscape(feature.name || "Untitled Feature")}</td>' in features_js
    assert '<a href="#/features/${encodeURIComponent(feature.id)}"' not in features_js
    assert '<table class="table features-table mobile-card-table">' in features_js
    assert '<col class="features-col-name">' in features_js
    assert '<col class="features-col-status">' in features_js
    assert 'class="features-status-cell"' in features_js
    assert '<span class="sort-arrow">' in features_js
    assert "FEATURE_MODAL_GAP_PAGE_SIZE" in features_js
    assert 'renderPaginationControls("feature-modal-gaps"' in features_js
    assert 'bindPaginationControls(root, "feature-modal-gaps"' in features_js
    assert 'data-neighbor-id="${htmlEscape(gaps[globalIdx - 1]?.id || "")}"' in features_js
    assert 'api("POST", `/api/features/${encodeURIComponent(featureId)}/gaps/${encodeURIComponent(gapId)}/reorder`' in features_js
    assert 'api("DELETE", `/api/features/${encodeURIComponent(featureId)}/gaps/${encodeURIComponent(gapId)}`' not in features_js
    assert 'api("POST", `/api/features/${encodeURIComponent(featureId)}/cancel`' in features_js
    assert 'api("DELETE", `/api/features/${encodeURIComponent(featureId)}`' in features_js
    assert 'r"/api/features/([0-9A-Za-z]{26})/workflow"' in server_py
    assert "def move_feature_workflow" in api_py
    assert "feature_ops.move_feature_workflow" in api_py
    assert 'params.set("feature", f.feature);' in gaps_list_js
    assert 'placeholder="Feature ID or standalone"' in gaps_list_js
    assert 'id="bulk-assign-feature"' in gaps_list_js
    assert 'bindCommand("#bulk-assign-feature", "gaps.bulk.feature")' in gaps_list_js
    assert "openBulkAssignFeatureModal" in gaps_bulk_js
    assert 'api("POST", `/api/features/${encodeURIComponent(featureId)}/gaps/bulk`' in gaps_bulk_js
    assert "renderGapFeatureCell" in gaps_list_js
    assert "renderGapFeatureAssociation" in gaps_detail_js
    assert "openGapFeatureAssignModal" in gaps_detail_js
    assert '<button class="nav-menu-item" type="button" id="btn-gap-feature-assign">Move to Feature</button>' in gaps_detail_js
    assert '<button class="nav-menu-item" type="button" id="btn-gap-feature-remove" ${gap.feature_id ? "" : "disabled"}>Remove from Feature</button>' in gaps_detail_js
    assert 'id="btn-gap-feature-assign" type="button">${gap.feature_id ? "Move" : "Assign"}' not in gaps_detail_js
    assert 'id="btn-gap-feature-remove" type="button">Remove</button>' not in gaps_detail_js
    assert "moveGapWithinFeature" not in gaps_detail_js
    assert 'id="btn-gap-feature-up"' not in gaps_detail_js
    assert 'id="btn-gap-feature-down"' not in gaps_detail_js
    assert "data-gap-feature-move" not in gaps_list_js
    assert ">Up</button>" not in gaps_list_js
    assert ">Down</button>" not in gaps_list_js
    assert 'name="import-feature-mode"' in import_js
    assert "new_feature_name" in import_js
    assert "feature_id: dest.existingId.trim()" in import_js
    assert "features.is_enabled" not in runner_py
    assert "feature_disabled" not in runner_py
    assert 'key.startswith("feature_")' not in project_state_py
    assert not (root / "refine_server/features.py").exists()
    assert 'await api("PATCH", "/api/settings", collectSettingsApplicationPayload())' in application_save_body
    assert "_targetAppDraftDirty = false;" in application_save_body
    assert 'await refreshSettingsTab("application", { force: true });' in application_save_body
    assert "autosaveSettingsGovernance" in settings_js
    assert "autosaveSettingsQuality" in settings_js
    assert "autosaveSettingsApplication" in settings_js
    assert "autosaveSettingsRuntime" in settings_js
    assert "function renderSettingsMarkdownField" in settings_js
    assert "function renderSettingsGuideLabel" in settings_js
    assert "function renderSettingsGuideIcon" in settings_js
    assert 'data-guide-label-item="${htmlEscape(itemId)}"' in settings_js
    assert 'tabindex="-1"' in settings_js
    assert ".settings-guide-icon" in common_css
    settings_guide_icon_css = re.search(r"\.settings-guide-icon \{(.*?)\}", common_css, re.S)
    assert settings_guide_icon_css
    assert "border: 0;" in settings_guide_icon_css.group(1)
    assert "background: transparent;" in settings_guide_icon_css.group(1)
    assert "box-shadow: none;" in settings_guide_icon_css.group(1)
    settings_guide_hover_css = re.search(
        r"\.settings-guide-icon:hover:not\(:disabled\),\s*\.settings-guide-icon:focus-visible \{(.*?)\}",
        common_css,
        re.S,
    )
    assert settings_guide_hover_css
    assert "background: transparent;" in settings_guide_hover_css.group(1)
    assert "border-color: transparent;" in settings_guide_hover_css.group(1)
    assert "box-shadow: none;" in settings_guide_hover_css.group(1)
    assert ".settings-guide-icon:hover:not(:disabled) svg" in common_css
    settings_guide_svg_hover_css = re.search(
        r"\.settings-guide-icon:hover:not\(:disabled\) svg,\s*\.settings-guide-icon:focus-visible svg \{(.*?)\}",
        common_css,
        re.S,
    )
    assert settings_guide_svg_hover_css
    assert "color: var(--color-primary-hover);" in settings_guide_svg_hover_css.group(1)
    assert "transform: scale(1.12);" in settings_guide_svg_hover_css.group(1)
    assert "[data-guide-label-item]" in guide_js
    assert "openGuide({ itemId, openTarget: false })" in guide_js
    assert "guideState.activeTab = guideTabForCategory(found.category)" in guide_js
    open_guide_body = guide_js.split("function openGuide(options = {})", 1)[1].split("function selectGuideTab", 1)[0]
    assert "if (requested) {\n    activateGuideItem(requested);" in open_guide_body
    assert "guideState.activeTab = GUIDE_TAB_GET_STARTED;" in open_guide_body
    assert "guideState.activeTab = normalizeGuideTab(guideState.activeTab) || GUIDE_TAB_REFERENCE;" not in open_guide_body
    assert "function bindSettingsMarkdownFields" in settings_js
    assert "data-settings-markdown-preview" in settings_js
    assert "data-settings-markdown-editor" in settings_js
    assert "data-settings-markdown-edit" in settings_js
    assert "function settingsMarkdownIcon(name)" in settings_js
    assert 'settingsMarkdownIcon("edit")' in settings_js
    assert 'settingsMarkdownIcon(editing ? "save" : "edit")' in settings_js
    assert "function commitSettingsMarkdownField(field)" in settings_js
    assert "function editSettingsMarkdownField(field)" in settings_js
    assert 'preview.innerHTML = trimmed ? mdToHtml(value)' in settings_js
    assert 'editor.dispatchEvent(new Event("change", { bubbles: true }));' in settings_js
    assert 'btn.addEventListener("mousedown"' in settings_js
    assert 'if (editor && !editor.hidden) e.preventDefault();' in settings_js
    assert 'editor.addEventListener("blur"' in settings_js
    assert 'commitSettingsMarkdownField(editor.closest("[data-settings-markdown-field]"));' in settings_js
    assert "mdToHtml(value)" in settings_js
    assert 'preview?.setAttribute("hidden", "")' in settings_js
    assert 'editor.hidden = false;' in settings_js
    assert 'editor.hidden = true;' in settings_js
    assert "btn.hidden = true;" not in settings_js
    for old_save_id in (
        'id="s-save"',
        'id="s-save-cli"',
        'id="s-save-scope"',
        'id="s-save-target"',
        'id="s-save-application"',
        'id="s-save-runtime"',
        'id="s-governance-save"',
        'id="s-quality-save"',
    ):
        assert old_save_id not in settings_js
    assert "settings-save-section" not in settings_js
    governance_body = settings_tab_files["settings_governance"]
    quality_body = settings_tab_files["settings_quality"]
    assert 'id: "s-governance-product"' in governance_body
    assert 'title: "Product"' in governance_body
    assert 'id: "s-governance-constitution"' in governance_body
    assert 'title: "Constitution"' in governance_body
    assert 'bindSettingsMarkdownFields(root);' in governance_body
    assert 'id: "s-quality-business-requirements"' in quality_body
    assert 'title: "Business requirements"' in quality_body
    assert 'id: "s-quality-instructions"' in quality_body
    assert 'title: "Instructions"' in quality_body
    assert 'bindSettingsMarkdownFields(root);' in quality_body
    assert ".settings-markdown-preview" in common_css
    assert ".settings-markdown-edit svg" in common_css
    settings_tabs_css = re.search(r"\.settings-tabs \{(.*?)\}", common_css, re.S)
    settings_tab_css = re.search(r"\.settings-tab \{(.*?)\}", common_css, re.S)
    settings_tab_active_css = re.search(
        r"\.settings-tab\.active \{(.*?)\}",
        common_css,
        re.S,
    )
    settings_section_css = re.search(
        r"\.settings-section:not\(:first-child\) \{(.*?)\}",
        common_css,
        re.S,
    )
    assert settings_tabs_css and "border: 1px solid var(--color-border)" in settings_tabs_css.group(1)
    assert settings_tabs_css and "border-bottom" not in settings_tabs_css.group(1)
    assert settings_tab_css and "background: transparent" in settings_tab_css.group(1)
    assert "button.settings-tab:hover:not(:disabled)" in common_css
    assert ".settings-tab.active:hover" in common_css
    settings_hover_css = re.search(
        r"\.settings-tab:hover,\s*\.settings-tab\.active:hover,\s*button\.settings-tab:hover:not\(:disabled\) \{(.*?)\}",
        common_css,
        re.S,
    )
    assert settings_hover_css and "background: var(--color-primary-hover)" in settings_hover_css.group(1)
    assert settings_hover_css and "color: white" in settings_hover_css.group(1)
    assert "border: 0" in settings_tab_css.group(1)
    assert "text-decoration: none" in settings_tab_css.group(1)
    assert settings_tab_active_css and "box-shadow: var(--shadow-sm)" in settings_tab_active_css.group(1)
    assert settings_section_css and "border-top: 1px solid var(--border)" in settings_section_css.group(1)

    for slug in slugs:
        assert f'if (slug === "{slug}")' in settings_js or f'slug: "{slug}"' in settings_js
    assert "surface.tabs.map(pane).join" in settings_js

    primary_nav = index_html.split('<nav class="nav">', 1)[1].split("</nav>", 1)[0]
    assert 'data-route="settings"' not in primary_nav
    assert 'data-route="node"' not in primary_nav
    assert 'data-route="project"' not in primary_nav
    assert 'class="nav-menu nav-context-menu" id="nav-context-menu"' in index_html
    context_panel = index_html.split('class="nav-menu-panel nav-context-panel"', 1)[1].split("</details>", 1)[0]
    assert '<label class="nav-menu-label nav-context-section-label" for="global-reporter">Reporter</label>' in context_panel
    assert '<div class="nav-menu-label nav-context-section-label">Management</div>' in context_panel
    assert '<a class="nav-menu-item nav-management-item" href="#/guide" id="nav-guide-open" data-route="guide">' in context_panel
    assert '<a class="nav-menu-item nav-management-item" href="#/node/nodes" data-route="node">' in context_panel
    assert '<a class="nav-menu-item nav-management-item" href="#/project/application" data-route="project">' in context_panel
    assert '<a class="nav-menu-item nav-management-item" href="#/system/processes" data-route="settings">' in context_panel
    assert context_panel.count('class="nav-menu-icon"') == 4
    assert 'id="nav-context-app-summary">Application</span>' in index_html
    assert 'id="nav-context-reporter-summary">No reporter</span>' in index_html
    assert '<select id="global-reporter" aria-label="Reporter"></select>' in index_html
    assert 'class="nav-create-group"' in index_html
    assert 'id="btn-command-palette"' in index_html
    assert 'id="btn-refine-issue"' in index_html
    assert 'class="nav-bug-icon"' in index_html
    assert 'id="btn-new-gap">+ New Gap</a>' in index_html
    assert 'class="nav-menu nav-create-menu" id="nav-create-menu"' in index_html
    assert 'id="btn-plan">Plan</a>' in index_html
    assert 'id="btn-import">Import gaps</a>' in index_html
    assert 'id="btn-refine-issue-menu">Request refine feature/bugfix</a>' in index_html
    assert index_html.index('id="btn-plan"') < index_html.index('id="btn-import"')
    assert 'id="target-app-indicator" class="target-app-indicator nav-context-status"' in index_html
    assert 'id="agent-status-indicator" class="agent-status-indicator nav-status-indicator"' in index_html
    assert '<span class="agent-status-label">0</span>' in index_html
    assert index_html.index('id="nav-context-menu"') < index_html.index('id="agent-status-indicator"')
    assert index_html.index('id="agent-status-indicator"') < index_html.index('id="btn-command-palette"')
    assert index_html.index('id="btn-command-palette"') < index_html.index('id="btn-refine-issue"')
    assert index_html.index('id="btn-refine-issue"') < index_html.index('class="nav-create-group"')
    assert index_html.index('class="nav-create-group"') < index_html.index('id="btn-new-gap"')
    assert 'indicator.href = opensApp ? appUrl : "#/system/processes";' in target_app_js
    assert 'indicator.target = "_blank";' in target_app_js
    assert 'indicator.removeAttribute("target");' in target_app_js
    assert 'const contextMenu = document.getElementById("nav-context-menu");' in target_app_js
    assert "updateNavAppContextLabel(projectLabel)" in target_app_js
    assert 'api("GET", "/api/processes")' in target_app_js
    assert 'processes.filter((proc) => proc.kind === "agent").length' in target_app_js
    assert 'const label = `Agents (${agentCount})`;' in target_app_js
    assert "const compactLabel = String(agentCount);" in target_app_js
    assert 'scheduleAgentStatusRefresh()' in common_js
    assert "function updateNavReporterContext()" in common_js
    assert "function updateNavAppContextLabel(label)" in common_js
    assert "function closeTopbarMenus(target = null)" in common_js
    assert 'e.target.closest("#btn-plan")' in common_js
    assert 'runCommand("plan.open")' in common_js
    assert "async function openPlanChatDock(options = {})" in toolbar_js
    assert "{ purpose: \"plan\" }" in toolbar_js
    assert "Draft Feature" in toolbar_js
    assert "function planTranscriptText(tab)" in toolbar_js
    assert "function planHasAgentResponse(tab)" in toolbar_js
    assert "function syncPlanDraftButton(tab)" in toolbar_js
    assert "btn.disabled = !planHasAgentResponse(tab);" in toolbar_js
    assert "function openPlanDraftModalFromText(text)" in import_js
    assert "featureDestination: {" in import_js
    assert 'mode: "new"' in import_js
    assert "inferPlanFeatureName(text)" in import_js
    assert ".nav-context-summary" in base_css
    assert ".nav-context-section-label" in base_css
    assert "margin-top: 12px;" in base_css
    assert ".nav-menu-icon" in base_css
    assert 'id="nav-guide-open"' in index_html
    assert '<aside id="guide-panel"' in index_html
    assert '<script src="/static/js/features/guide.js"></script>' in index_html
    assert '<link rel="stylesheet" href="/static/css/guide.css">' in index_html
    assert "const GUIDE_CATEGORIES = [" in guide_js
    assert 'const GUIDE_TAB_GET_STARTED = "get-started"' in guide_js
    assert 'const GUIDE_TAB_REFERENCE = "reference"' in guide_js
    assert "activeTab: GUIDE_TAB_GET_STARTED" in guide_js
    assert 'title: "Get Started"' in guide_js
    assert 'description: "The minimum steps needed to run refine on this app."' in guide_js
    assert 'title: "Quick Start"' not in guide_js
    assert 'Get Started &middot;' not in guide_js
    assert "completed/skipped" not in guide_js
    assert "function guideChecklistItemsInOrder" in guide_js
    assert "function guideReferenceCategories" in guide_js
    assert "function filteredGuideReferenceCategories" in guide_js
    assert "function renderGuideReferenceCategories" in guide_js
    assert 'const GUIDE_STATE_KEY_PREFIX = "refine_guide_state:"' in guide_js
    assert "function guideStateStorageKey" in guide_js
    assert "function readGuideStoredState" in guide_js
    assert "function saveGuideState" in guide_js
    assert "function loadGuideStateForProject" in guide_js
    assert "function loadGuideStateForCurrentApp" in guide_js
    assert "function guideChecklistComplete" in guide_js
    assert "function clearGuideSelection" in guide_js
    assert "function normalizeGuideTab" in guide_js
    assert "function guideTabForCategory" in guide_js
    assert "function guideCategoryCompleteIcon" in guide_js
    assert "function guideSearchIcon" in guide_js
    assert "nextInput.setSelectionRange(selectionStart, selectionEnd)" in guide_js
    assert "localStorage.setItem(guideStateStorageKey(), JSON.stringify({" in guide_js
    assert "localStorage.removeItem(guideStateStorageKey())" in guide_js
    assert "loadGuideStateForCurrentApp({ redraw: false })" in guide_js
    assert "async function selectGuideItem" in guide_js
    assert "const previousBodyScrollTop = guideBodyScrollTop()" in guide_js
    assert "function guideBodyScrollTop" in guide_js
    assert "function restoreGuideBodyScrollTop" in guide_js
    assert "restoreGuideBodyScrollTop(previousBodyScrollTop)" in guide_js
    assert "await openGuideItemTarget(found.item)" in guide_js
    assert "function selectGuideTab" in guide_js
    assert "function renderGuideTabStrip" in guide_js
    assert "function renderGuideGetStartedPane" in guide_js
    assert "function renderGuideReferencePane" in guide_js
    assert guide_js.index("${renderGuideTabStrip()}") < guide_js.index(
        '<p class="guide-intro">${htmlEscape(guideContextMessage())}</p>',
    )
    assert "root.querySelectorAll(\"[data-guide-tab]\")" in guide_js
    assert 'class="settings-tabs guide-tabs"' in guide_js
    assert 'class="settings-tab ${guideState.activeTab === tab.slug ? "active" : ""}"' in guide_js
    assert 'data-guide-tab="${tab.slug}"' in guide_js
    assert 'data-guide-tab-pane="${GUIDE_TAB_GET_STARTED}"' in guide_js
    assert 'class="guide-get-started-list"' in guide_js
    assert "guideChecklistItemsInOrder().map(({ item }) => renderGuideItem(item)).join(\"\")" in guide_js
    assert "const searchingReference = !category.checklist && Boolean(guideState.referenceQuery.trim())" in guide_js
    assert 'class="guide-tab-pane guide-reference"' in guide_js
    assert 'data-guide-tab-pane="${GUIDE_TAB_REFERENCE}"' in guide_js
    assert 'class="guide-reference-search"' in guide_js
    assert 'class="guide-reference-search-icon"' in guide_js
    assert 'class="guide-category-complete"' in guide_js
    assert 'data-guide-reference-search' in guide_js
    assert "Reference" in guide_js
    assert "Explanations for fields, settings, screens, and daily workflows." in guide_js
    assert 'id: "guide.open"' in guide_js
    assert 'id: "guide.toggle"' in guide_js
    assert 'class="guide-category"' in guide_js
    assert 'class="guide-item ' in guide_js
    assert 'data-guide-open-item' in guide_js
    assert 'data-guide-status' in guide_js
    assert 'data-guide-default' in guide_js
    assert 'data-guide-skip' in guide_js
    assert 'data-guide-complete' in guide_js
    assert "canUseDefault: options.canUseDefault !== false" in guide_js
    assert "{ canUseDefault: false }" in guide_js
    assert "const defaultButton = checklist && item.canUseDefault" in guide_js
    assert 'class="guide-progress"' not in guide_js
    assert 'class="guide-status guide-status-' in guide_js
    assert "function firstIncompleteGuideItem" in guide_js
    assert "function openGuideItemTarget" in guide_js
    assert "function completeGuideItem" in guide_js
    assert "function resetGuideState" in guide_js
    assert "localStorage.removeItem(GUIDE_CHECKLIST_KEY)" in guide_js
    assert "function clearGuideTargetHighlight" in guide_js
    assert "setTimeout(() => el.classList.remove(\"guide-target-highlight\")" not in guide_js
    assert "if (active?.category?.checklist && !guideChecklistComplete())" in guide_js
    assert 'class="guide-item-kind"' not in guide_js
    assert "Focus in app" not in guide_js
    assert 'hash: "#/node/application"' in guide_js
    assert 'hash: "#/project/quality"' in guide_js
    assert 'hash: "#/system/processes"' in guide_js
    no_app_guide_request = re.compile(
        r'context: "no-app",\s+'
        r'categoryId: "get-started",\s+'
        r'itemId: "quickstart-add-app",\s+'
        r'openTarget: true,'
    )
    assert len(no_app_guide_request.findall(common_js)) == 2
    assert no_app_guide_request.search(settings_core_js)
    assert 'guideItem("quickstart-add-app", "Add app"' in guide_js
    assert 'guideItem("project-application", "App selection and creation"' in guide_js
    settings_guide_field_ids = [
        "node-manage",
        "reporter-manage",
        "reporter-merge-into",
        "node-copy-settings-source",
        "application-agent-subpath",
        "application-merge-target",
        "application-url",
        "application-start",
        "application-stop",
        "application-rebuild",
        "application-auto-rebuild",
        "application-status",
        "application-working-directory",
        "application-environment",
        "application-start-timeout",
        "application-stop-timeout",
        "application-rebuild-timeout",
        "application-status-timeout",
        "application-log-path",
        "application-http-check-url",
        "application-tcp-host",
        "application-tcp-port",
        "application-process-check-command",
        "runtime-parallel-run-cap",
        "runtime-branch-name-pattern",
        "runtime-agent-idle-timeout",
        "runtime-agent-hard-cap",
        "runtime-worker-memory-limit",
        "runtime-ui-memory-limit",
        "runtime-worker-cpu-priority",
        "runtime-resource-isolation",
        "runtime-agent-limit-pause",
        "runtime-chat-idle-timeout",
        "runtime-backlog-promote",
        "runtime-project-update-pulse",
        "runtime-file-browser-ignore",
        "runtime-ai-provider",
        "project-known-apps",
        "quality-enabled",
        "quality-gate",
        "quality-regressions-enabled",
        "quality-regression-title",
        "quality-regression-scenario",
        "quality-requirements",
        "quality-instructions",
        "governance-product",
        "governance-constitution",
        "governance-rules",
        "guidance-items",
        "guidance-name",
        "guidance-rule",
        "guidance-instructions",
        "guidance-status",
        "process-management",
        "process-agent-processes",
        "process-runner-processes",
        "performance-overview",
        "performance-operation-filter",
        "performance-outcome-filter",
        "performance-limit",
    ]
    for field_id in settings_guide_field_ids:
        assert f'"{field_id}"' in settings_js
        assert f'guideItem("{field_id}"' in guide_js
    assert 'command: "gap.new"' in guide_js
    assert 'command: "gap.import"' in guide_js
    assert 'command: "refine.issue.request"' in guide_js
    assert ".guide-resize::after" in guide_css
    assert ".guide-tabs-row" in guide_css
    assert ".guide-tabs" in guide_css
    guide_tabs_row = re.search(r"\.guide-tabs-row \{(.*?)\}", guide_css, re.S)
    assert guide_tabs_row
    assert "width: 100%;" in guide_tabs_row.group(1)
    guide_tabs_css = re.search(r"\.guide-tabs \{(.*?)\}", guide_css, re.S)
    assert guide_tabs_css
    assert "width: 100%;" in guide_tabs_css.group(1)
    assert "max-width: none;" in guide_tabs_css.group(1)
    assert ".guide-progress" not in guide_css
    assert ".guide-tab-pane" in guide_css
    assert ".guide-get-started-list" in guide_css
    assert ".guide-get-started-list .guide-item" in guide_css
    active_guide_item = re.search(r"\.guide-item\.active \{(.*?)\}", guide_css, re.S)
    assert active_guide_item
    active_guide_item_css = active_guide_item.group(1)
    assert "background: var(--color-success-soft);" in active_guide_item_css
    assert "rgba(22, 163, 74" in active_guide_item_css
    assert "#fffbea" not in active_guide_item_css
    assert "color: #111827;" in guide_css
    target_highlight_css = guide_css.split(".guide-target-highlight", 1)[1].split("}", 1)[0]
    assert "background-color" not in target_highlight_css
    assert "rgba(22, 163, 74" in target_highlight_css
    assert "rgba(37, 99, 235" not in target_highlight_css
    target_pulse_css = guide_css.split("@keyframes guide-target-pulse", 1)[1]
    assert "rgba(22, 163, 74" in target_pulse_css
    assert "rgba(37, 99, 235" not in target_pulse_css
    assert ".guide-reference" in guide_css
    assert ".guide-reference-search" in guide_css
    assert ".guide-reference-search input" in guide_css
    assert ".guide-reference-search-icon" in guide_css
    assert ".guide-category-complete" in guide_css
    assert ".guide-item-open" in guide_css
    assert ".guide-item-actions" in guide_css
    assert ".guide-status-checked" in guide_css
    assert ".guide-status-skipped" in guide_css
    assert "animation: guide-target-pulse" in guide_css
    assert "@keyframes guide-target-pulse" in guide_css
    assert "body.guide-open .toolbar-dock" in guide_css
    assert "--guide-panel-width" in guide_css
    assert ".nav-issue-button" in base_css
    assert ".nav-bug-icon" in base_css
    assert ".nav-create-group" in base_css
    assert ".nav-menu-panel" in base_css
    topbar_css = re.search(r"\.topbar \{(.*?)\}", base_css, re.S)
    assert topbar_css and "position: relative" in topbar_css.group(1)
    assert topbar_css and "z-index: 120" in topbar_css.group(1)
    assert '.agent-status-indicator[data-state="running"] .target-app-dot' in base_css
    assert '.agent-status-indicator[data-state="paused"] .target-app-dot' in base_css
    assert '.agent-status-indicator[data-state="down"] .target-app-dot' in base_css
    assert '.nav-context-menu[data-state="running"] .nav-context-summary-dot' in base_css
    assert 'data-rmerge="${r.id}"' in settings_js
    assert "function openReporterMergeModal(source)" in settings_js
    assert 'api("POST", `/api/reporters/${b.dataset.rmerge}/merge`' in settings_js
    assert "Merging a reporter moves its Gaps to another" in settings_js
    assert "renderSettingsReportersTab(data.reps, data.activeNodeLabel)" in settings_js
    assert "bindSettingsReportersTab();" in settings_js
    assert 'def merge_reporter(rid: int, body: dict)' in api_py
    assert 'reporter_ops.merge_reporter(_backend_runner_call, rid, target_rid)' in api_py
    assert 'M_MERGE_REPORTER' in (root / "refine_server/reporter_ops.py").read_text(encoding="utf-8")
    assert '@route("POST", r"/api/reporters/(\\d+)/merge")' in server_py
    for name in settings_tab_files:
        assert f'<script src="/static/js/features/{name}.js"></script>' in index_html
    assert index_html.index(f"/static/js/features/{name}.js") < index_html.index("/static/js/features/settings.js")
    assert 'slug: "nodes"' in settings_js
    assert 'api("GET", "/api/nodes")' in settings_js
    assert "Transfer Gaps" not in settings_tab_files["settings_nodes"]
    assert "node-transfer" not in settings_tab_files["settings_nodes"]
    assert 'api("POST", "/api/nodes/transfer-gaps"' not in settings_tab_files["settings_nodes"]
    assert 'api("POST", "/api/nodes/transfer-gaps"' in gaps_bulk_js
    processes_body = settings_tab_files["settings_processes"]
    application_body = settings_tab_files["settings_application"]
    runtime_body = settings_tab_files["settings_runtime"]
    assert 'id="s-rebuild-cache"' not in runtime_body
    assert 'id="s-target-run-rebuild"' in processes_body
    assert 'id="s-target-run-start"' in processes_body
    assert 'id="s-target-run-stop"' in processes_body
    assert 'id="s-target-sync-now"' in processes_body
    assert 'id="s-target-health-now"' in processes_body
    assert processes_body.index('id="s-target-run-start"') < processes_body.index('id="s-target-run-stop"') < processes_body.index('id="s-target-run-rebuild"')
    assert 'class="target-app-action-slot"' in processes_body
    assert 'rebuildBtn.disabled = inFlight;' in processes_body
    assert 'No rebuild command configured; rebuild will still run the stop/start sequence.' in processes_body
    assert 'refreshAgentStatusIndicator === "function"' in commands_js
    assert "function targetAppShowsStopAction" in processes_body
    assert "function setTargetAppActionVisible" in processes_body
    assert 'proc.runner_reachable && !paused ? "" : "disabled"' in processes_body
    assert 'data-toggle-background-processes' in processes_body
    assert '${stopped ? "Start" : "Stop"} Background</button>' in processes_body
    assert '${stopped ? "Start" : "Stop"} Background Processes' not in processes_body
    assert 'api("POST", "/api/processes/background", { stopped: shouldStop })' in processes_body
    assert 'data-toggle-agent-processes' in processes_body
    assert '${agentsPaused ? "Unpause" : "Pause"} agents</button>' in processes_body
    assert processes_body.index('if (proc.kind === "supervisor")') < processes_body.index('if (proc.kind === "agent_scheduler")')
    assert 'api("POST", "/api/processes/agents", { paused: shouldPause })' in processes_body
    assert 'title: "Pause agents", okLabel: "Pause agents"' in processes_body
    assert 'title: "Pause or unpause agents"' in commands_js
    assert 'api("POST", "/api/processes/agents", { paused: !agentsPaused })' in commands_js
    assert "scheduleProcessesTabRefreshes()" in commands_js
    assert "function scheduleProcessesTabRefreshes()" in processes_body
    assert '[data-tab-pane="processes"].active' in processes_body
    assert "refreshCurrentSettingsSurface()" in common_js
    assert '["settings", "node", "project"].includes(state.currentRoute || "")' in common_js
    assert 'sseSource.addEventListener("system_operation"' in common_js
    assert 'sseSource.addEventListener("activity_added"' in common_js
    assert "message: entry.message" in common_js
    assert "recordSystemOperation(payload)" in common_js
    assert "SYSTEM_OPERATION_EVENT = \"system_operation\"" in system_events_py
    assert "sse.publish(SYSTEM_OPERATION_EVENT, payload)" in system_events_py
    assert "system_operation" in sse_py
    assert "const SYSTEM_TAB_ID = \"system\";" in toolbar_js
    assert "const SYSTEM_OPERATION_LOG_LIMIT = 250;" in toolbar_js
    assert "const SYSTEM_LOG_FILTERS = [" in toolbar_js
    assert '{ status: "complete", label: "Completed" }' in toolbar_js
    assert '{ status: "error", label: "Errors" }' in toolbar_js
    assert "filters: new Set()" in toolbar_js
    assert 'label: "System", mode: "system"' in toolbar_js
    assert "function recordSystemOperation(payload)" in toolbar_js
    assert "function normalizeSystemLogStatus(status)" in toolbar_js
    assert "systemOperationState.messages.slice(-SYSTEM_OPERATION_LOG_LIMIT)" in toolbar_js
    assert "function renderSystemPanel()" in toolbar_js
    assert "function renderSystemLogFilters(messages, activeFilters)" in toolbar_js
    assert "messages.filter((item) => activeFilters.has(item.status))" in toolbar_js
    assert "function bindSystemPanel(root)" in toolbar_js
    assert '<label class="system-log-filter${!activeFilters.size ? " active" : ""}">' in toolbar_js
    assert 'type="checkbox"' in toolbar_js
    assert "data-system-log-filter" in toolbar_js
    assert "systemOperationState.filters.add(filter)" in toolbar_js
    assert "No system activity matches this filter." in toolbar_js
    assert "Waiting for system activity." in toolbar_js
    assert ".system-panel" in toolbar_css
    assert ".system-panel-header .muted.small" in toolbar_css
    assert ".system-log-filters" in toolbar_css
    assert "border-radius: 999px;" in toolbar_css
    assert ".system-log-filter-complete.active" in toolbar_css
    assert ".system-log-filter-error.active" in toolbar_css
    assert ".system-log-line" in toolbar_css
    assert "refreshActiveSettingsTab({ force: true })" in processes_body
    assert "function refreshProcessesSettingsTab(options = {})" in processes_body
    assert 'data-cancel-agent="' in processes_body
    assert 'data-stop-chat="' in processes_body
    assert "startBtn.style.display" not in settings_js
    assert "stopBtn.style.display" not in settings_js
    assert "setTargetAppActionVisible(startBtn, !showStop);" in settings_js
    assert "setTargetAppActionVisible(stopBtn, showStop);" in settings_js
    assert "startBtn.disabled = showStop || isRunning || inFlight || !snap.has_start_command;" in settings_js
    assert "stopBtn.disabled  = !showStop || isStopped || inFlight || !snap.has_stop_command;" in settings_js
    assert 'class="table process-table managed-process-table mobile-card-table"' in processes_body
    assert 'class="table process-table agents-process-table mobile-card-table"' in processes_body
    assert 'class="table process-table runner-workers-table mobile-card-table"' in processes_body
    assert processes_body.index('renderSettingsGuideLabel("Process management", "process-management")') < processes_body.index('renderSettingsGuideLabel("Agent processes", "process-agent-processes")') < processes_body.index('renderSettingsGuideLabel("Runner processes", "process-runner-processes")')
    managed_table = processes_body.split('class="table process-table managed-process-table mobile-card-table"', 1)[1].split("</table>", 1)[0]
    runner_table = processes_body.split('class="table process-table runner-workers-table mobile-card-table"', 1)[1].split("</table>", 1)[0]
    agents_table = processes_body.split('class="table process-table agents-process-table mobile-card-table"', 1)[1].split("</table>", 1)[0]
    assert "<th>CPU priority</th>" in managed_table
    assert "<th>Max memory</th>" in managed_table
    assert "<th>Elapsed</th>" not in managed_table
    assert "<th>Idle</th>" not in managed_table
    assert "<th>Worker</th>" in runner_table
    assert "<th>Queue</th>" in runner_table
    assert 'data-runner-target-app-rebuild' in processes_body
    assert '["running", "queued", "unknown", "paused"].includes(work.status)' in processes_body
    assert 'data-runner-log-cleanup-days aria-label="Activity log retention" ${paused ? "disabled" : ""}' in processes_body
    assert 'data-runner-target-app-generate' in processes_body
    assert 'data-runner-cache-rebuild' in processes_body
    assert 'data-runner-log-cleanup' in processes_body
    assert 'data-runner-log-cleanup-days' in processes_body
    assert 'data-hard-reset-worktree' in processes_body
    assert 'api("POST", "/api/runner-workers/target-app-rebuilder/rebuild")' in settings_js
    assert 'api("POST", "/api/runner-workers/merger/hard-reset-worktree")' in commands_js
    assert 'api("POST", "/api/target-app/generate-instructions"' in commands_js
    assert 'api("POST", "/api/activity/cleanup"' in commands_js
    assert '@route("POST", r"/api/runner-workers/target-app-rebuilder/rebuild")' in server_py
    assert '@route("POST", r"/api/runner-workers/merger/hard-reset-worktree")' in server_py
    assert "<th>CPU priority</th>" in agents_table
    assert "<th>Max memory</th>" in agents_table
    assert "<th>Elapsed</th>" in agents_table
    assert "<th>Idle</th>" in agents_table
    assert "<th>Context</th>" in agents_table
    assert "renderAgentProcessRow" in processes_body
    assert '.filter((proc) => proc.kind === "agent" || proc.kind === "chat")' in processes_body
    assert '.filter((proc) => proc.kind !== "agent" && proc.kind !== "chat")' in processes_body
    assert "No active agent subprocesses or chat sessions." in processes_body
    assert "No active runner work." not in processes_body
    assert "refreshProcessesTabForChatChange" in toolbar_js
    assert 'proc.mode === "plan" ? "Plan chat"' in processes_body
    assert 'idle: "idle"' in processes_body
    assert 'data-full-details="${htmlEscape(details)}"' in processes_body
    assert "function bindProcessDetailCells" in processes_body
    assert "function openProcessDetailsIfOverflowing" in processes_body
    assert 'modalAlert(details' in processes_body
    assert '<span class="role-pill ${kind === "agent"' not in processes_body
    assert '<span class="role-pill merger"' not in processes_body
    assert 'class="process-actions"><div class="actions">' in processes_body
    assert 'renderSettingsGuideLabel("Process management", "process-management")' in processes_body
    assert 'renderSettingsGuideLabel("Agent processes", "process-agent-processes")' in processes_body
    assert 'renderSettingsGuideLabel("Runner processes", "process-runner-processes")' in processes_body
    assert 'data-process-id="${htmlEscape(proc.id || "")}"' in processes_body
    assert '[data-process-id="target-app"]' in settings_js
    assert "Background processes" in processes_body
    assert "function orderManagedProcessRows" in processes_body
    assert "supervisor_child_hidden: !supervisorProcessExpanded" in processes_body
    assert 'data-supervisor-toggle' in processes_body
    assert 'data-supervisor-child="1"' in processes_body
    assert "function bindSupervisorProcessToggle" in processes_body
    order_body = processes_body.split(
        "function orderManagedProcessRows", 1,
    )[1].split("function runnerProcessDetails", 1)[0]
    assert 'proc.kind === "target_app"' in order_body
    assert order_body.rfind("...(targetApp ? [targetApp] : [])") > order_body.find("scheduler")
    assert "runnerProcessDetails" in processes_body
    assert "<h3>Backend</h3>" not in processes_body
    assert 'id="target-app-status-block"' not in processes_body
    assert "<dt>Process model</dt>" not in processes_body
    assert "<dt>Runner transport</dt>" not in processes_body
    assert '<h3>Agent processes</h3>' not in runtime_body
    assert 'id="btn-pause"' not in runtime_body
    assert 'data-cancel-agent="' not in runtime_body
    assert 'id="s-target-run-start"' not in application_body
    assert 'id="s-project-sync-now"' not in processes_body
    assert 'await withButtonBusy(button, "Syncing...", async () => {' in commands_js
    assert "await syncProjectUpdates();" in commands_js
    assert "await refreshProcessesSettingsTab({ force: true });" in commands_js
    assert ".process-table {" in common_css
    assert ".process-table .cpu-col { width: 86px; }" in common_css
    assert ".process-table .memory-col { width: 92px; }" in common_css
    assert ".managed-process-table .actions-col { width: 326px; }" in common_css
    assert ".agents-process-table .agent-col" in common_css
    assert ".agents-process-table .agent-actions-col" in common_css
    assert ".runner-workers-table .worker-actions-col" in common_css
    assert 'id="s-target-generate"' not in settings_js
    assert 'id="logs-cleanup"' not in settings_js
    assert 'id="logs-cleanup-days"' not in settings_js
    assert ".process-table td[data-process-details]" in common_css
    assert ".process-table .process-actions .actions" in common_css
    assert ".process-table .process-details-cell.is-overflowing" in common_css
    assert ".process-table .process-details-cell:focus-visible" in common_css
    assert ".process-tree-toggle" in common_css
    assert ".supervisor-child-label" in common_css
    assert ".managed-process-table tr.supervisor-child[hidden]" in common_css
    assert ".target-app-action-slot" in common_css
    assert ".target-app-action-hidden" in common_css
    nodes_body = settings_tab_files["settings_nodes"]
    assert 'id="s-project-sync-now"' not in nodes_body
    assert "Trigger sync repo" not in nodes_body
    assert 'id="node-add"' in nodes_body
    assert 'api("GET", "/api/guidance")' in settings_js
    assert 'id="guidance-add"' in settings_js
    assert 'id="guidance-form"' in settings_js
    assert '<table class="table guidance-table">' in settings_js
    assert '<thead><tr><th>Name</th><th>Status</th><th>Rule</th></tr></thead>' in settings_js
    assert ".guidance-table-row td:first-child" in common_css
    assert "text-decoration: underline;" in common_css
    assert 'data-guidance-open' in settings_js
    assert 'data-toggle-enabled' in settings_js
    assert 'status-pill ${statusClass}' in settings_js
    assert 'enabled: guidanceEnabled' in settings_js
    assert 'data-delete>Delete guidance' in settings_js
    assert 'data-guidance-edit' not in settings_js
    assert 'data-guidance-remove' not in settings_js
    assert 'id="guidance-save"' not in settings_js
    assert 'api("PUT", "/api/guidance"' in settings_js
    assert 'name="instructions"' in settings_js
    assert 'await syncProjectUpdates();' not in settings_tab_files["settings_nodes"]
    assert 'sseSource.addEventListener("project_updated"' in (
        root / "refine_ui/static/js/common.js"
    ).read_text(encoding="utf-8")
    assert 'id="s-target-rebuild-command"' in settings_js
    assert 'id="s-target-app-url"' in settings_js
    assert '<input type="url" id="s-target-app-url"' in settings_js
    assert 'target_app_url: $("#s-target-app-url").value' in settings_js
    assert 'errLog?.message || workflowLog?.message || fallbackLog?.message || "Gap failed"' in gaps_detail_js
    assert "function currentRoundLog(log, workflowLog)" in gaps_detail_js
    assert "function latestStateBoundary(latest)" in gaps_detail_js
    assert "latest?.latest_state_log" in gaps_detail_js
    assert "logDatetime < workflowDatetime" in gaps_detail_js
    assert 'lastLog?.message || "Agent run failed"' not in gaps_detail_js
    assert "async function loadSettingsSurfaceData()" in settings_core_js
    assert 'api("GET", "/api/settings")' in settings_core_js
    assert "return 200, settings_ops.list_settings(conn)" in api_py
    assert "settings_ops.update_settings(" in api_py
    assert "ALLOWED_SETTINGS = {" in settings_ops_py
    assert '"target_app_env_json"' in settings_ops_py
    assert 'api("GET", "/api/reporters")' in settings_core_js
    assert "renderSettingsNodesTab({" in settings_js
    assert "renderSettingsReportersTab(data.reps, data.activeNodeLabel)" in settings_js
    assert "renderNodeApplicationConfigSections" in settings_js
    assert "renderNodeRuntimeConfigSections" in settings_js
    assert "function renderSettingsApplicationTab" in settings_js
    assert "target_app_url" not in common_js
    assert 'id="s-project-template"' in settings_js
    assert "Select app template" in settings_js
    assert "await openProjectTemplateSelector()" in settings_js
    assert 'id="s-application-copy-node"' in settings_js
    assert 'id="s-target-generate-ai"' in settings_js
    assert 'copySettingsFromNode("application"' in commands_js
    assert 'api("POST", "/api/target-app/generate-instructions", { kind: "all" })' in commands_js
    assert 'id="s-runtime-copy-node"' in runtime_body
    assert 'copySettingsFromNode("runtime"' in commands_js
    assert 'api("POST", "/api/nodes/copy-settings"' in settings_js
    assert '@route("POST", r"/api/nodes/copy-settings")' in server_py
    assert 'id="s-target-auto-rebuild"' in settings_js
    assert 'id="s-quality-enabled"' not in application_body
    assert 'id="s-quality-enabled"' in settings_js
    assert 'id="s-quality-timing"' in settings_js
    assert 'id="s-quality-regressions-enabled"' in settings_js
    assert 'id="s-quality-regression-new"' in settings_js
    assert 'id="s-quality-regression-run"' in settings_js
    assert "Choose whether QA runs before merge in the Gap worktree" in settings_js
    assert "after the shared application rebuild" in settings_js
    assert "Workflow QA runs these checks in the active QA environment." in settings_js
    assert "Run current checkout" in settings_js
    assert 'id="regression-create-input-title"' in settings_js
    assert 'id="regression-create-input-prompt"' in settings_js
    assert 'modalPrompt("Regression title"' not in settings_js
    assert 'const qualityEnabled = String(quality.enabled || "0") === "1";' in settings_js
    assert 'const qualityTiming = quality.timing === "post_rebuild" ? "post_rebuild" : "pre_merge";' in settings_js
    assert 'const regressionsEnabled = String(quality.regressions_enabled || "0") === "1";' in settings_js
    assert 'aria-pressed="${qualityEnabled ? "true" : "false"}"' in settings_js
    assert 'class="${qualityEnabled ? "" : "warn"}"' in settings_js
    assert 'QA ${qualityEnabled ? "enabled" : "disabled"}' in settings_js
    assert 'if (qualityEnabled) body.enabled = qualityEnabled.dataset.enabled === "1" ? "1" : "0";' in settings_js
    assert "if (qualityTiming) body.timing = qualityTiming.value;" in settings_js
    assert 'btn.classList.toggle("warn", !enabled);' in settings_js
    assert 'btn.textContent = enabled ? "QA enabled" : "QA disabled";' in settings_js
    assert ".toggle-button.on" not in common_css
    project_config_ops_py = (root / "refine_server/project_config_ops.py").read_text(encoding="utf-8")
    assert "return 200, project_config_ops.quality_get(conn)" in api_py
    assert '"enabled"] = db.get_setting(conn, "quality_enabled", "0") or "0"' in project_config_ops_py
    assert '"regressions_enabled"] = (' in project_config_ops_py
    assert "quality_regression_create" in api_py
    assert 'M_REGRESSION_RUN' in project_config_ops_py
    assert 'api("GET", "/api/quality")' in settings_js
    assert 'api("PATCH", "/api/quality"' in settings_js
    assert 'api("POST", "/api/quality/regressions"' in settings_js
    assert 'api("POST", "/api/quality/regressions/run"' in settings_js
    assert '@route("GET", r"/api/quality")' in server_py
    assert '@route("PATCH", r"/api/quality")' in server_py
    assert '@route("POST", r"/api/quality/regressions")' in server_py
    assert '@route("POST", r"/api/quality/regressions/run")' in server_py
    assert "def quality_get" in api_py
    assert "def quality_save" in api_py
    assert 'target_app_rebuild_command: $("#s-target-rebuild-command").value' in settings_js
    assert 'target_app_auto_rebuild: $("#s-target-auto-rebuild").value' in settings_js
    assert 'target_app_auto_rebuild_hour_utc: $("#s-target-auto-rebuild-hour-utc").value' in settings_js
    assert '"on_worktree_merge", "On worktree merge"' in settings_js
    assert 's.target_app_auto_rebuild || "on_worktree_merge"' in settings_js
    assert 's.parallel_run_cap || 5' in settings_js
    assert '"daily", "Daily (time)"' in settings_js
    assert 'id="s-target-auto-rebuild-hour-utc"' in settings_js
    assert 'Daily (${String(snap.auto_rebuild_hour_utc || "0").padStart(2, "0")}:00 UTC)' in settings_js
    assert "Nightly (midnight)" not in settings_js
    api_source = (root / "refine_ui/api.py").read_text(encoding="utf-8")
    target_app_ops_source = (root / "refine_server/target_app_ops.py").read_text(encoding="utf-8")
    assert "target_app_auto_rebuild" in settings_ops_py
    assert "target_app_auto_rebuild_hour_utc" in settings_ops_py
    assert '"target_app_url"' in settings_ops_py
    assert "return target_app_ops.snapshot(conn)" in api_source
    assert '"app_url": settings.get("target_app_url") or ""' in target_app_ops_source
    assert 'set("#s-target-rebuild-command", cfg.rebuild_command || "")' in settings_js
    assert "function applyGeneratedTargetAppConfig(cfg)" in settings_js
    generated_body = settings_js.split("function applyGeneratedTargetAppConfig(cfg)", 1)[1]
    generated_body = generated_body.split("\n}", 1)[0]
    assert "_targetAppDraftDirty = true;" in generated_body
    for expected in (
        'set("#s-target-start-command", cfg.start_command || "")',
        'set("#s-target-stop-command", cfg.stop_command || "")',
        'set("#s-target-status-command", cfg.status_command || "")',
        'set("#s-target-cwd", cfg.cwd || "")',
        'set("#s-target-env", JSON.stringify(cfg.env || {}, null, 2))',
        'set("#s-target-process-command", cfg.process_check_command || "")',
    ):
        assert expected in settings_js
    assert "const WORKFLOW_STATUSES = [" in common_js
    assert '"qa",' in common_js
    assert '"awaiting-rebuild",' in common_js
    assert "const orderedStatuses = workflowStatuses();" in dashboard_js
    assert "const POST_REBUILD_WORKFLOW_STATUSES = [" in common_js
    assert 'state.dashboard?.quality_timing === "post_rebuild"' in common_js
    assert "dashboard-status-grid" in dashboard_js
    assert "const AGENT_MANAGED_DASHBOARD_STATUSES = new Set([" in dashboard_js
    assert '"todo",' in dashboard_js
    assert '"in-progress",' in dashboard_js
    assert '"qa",' in dashboard_js
    assert '"ready-merge",' in dashboard_js
    assert '"awaiting-rebuild",' in dashboard_js
    assert "dashboard-status-card-agent" in dashboard_js
    assert "dashboard-agent-indicator" in dashboard_js
    assert "dashboard-status-head" in dashboard_js
    assert "Agent-managed automation" not in dashboard_js
    assert "AI-managed automation" in dashboard_js
    assert ">Auto<" not in dashboard_js
    assert ">AI<" in dashboard_js
    assert "dashboard-collapsible-shell" in dashboard_js
    assert "dashboardRefreshInFlight" in dashboard_js
    assert "dashboardRefreshQueued" in dashboard_js
    assert "DASHBOARD_REFRESH_TIMEOUT_MS" in dashboard_js
    assert "state.dashboardReviewSnapshot" in dashboard_js
    assert "scheduleDashboardRetry()" in dashboard_js
    assert "function dashboardScopeFromHash()" in dashboard_js
    assert "function dashboardHash(scope)" in dashboard_js
    assert "`/api/dashboard?node=${nodeParam}`" in dashboard_js
    assert "dashboard_ops.summary(" in api_py
    assert "dashboard_ops.empty_dashboard(" in api_py
    assert "def compute_reporter_stats(" in dashboard_ops_py
    assert "`&node=${nodeParam}&limit=200`" in dashboard_js
    assert "dashboard-title-row" in dashboard_js
    assert "dashboard-scope-switch" in dashboard_js
    assert "function wireDashboardScopeSwitch()" in dashboard_js
    assert "function syncDashboardScopeSwitch(scope)" in dashboard_js
    assert 'aria-label="Dashboard node scope"' in dashboard_js
    assert 'btn.setAttribute("aria-pressed", active ? "true" : "false")' in dashboard_js
    assert 'data-dashboard-scope="current"' in dashboard_js
    assert 'data-dashboard-scope="all"' in dashboard_js
    assert "Stats for" not in dashboard_js
    assert "scopeLabel" not in dashboard_js
    assert "Current node" not in dashboard_js
    assert "All nodes" not in dashboard_js
    assert "node: x.filter?.node || scope" in dashboard_js
    assert "gapsHash({ status: s, node: scope })" in dashboard_js
    assert "gapsHash({ reporter: row.dataset.reporter, node: scope })" in dashboard_js
    assert "const showReviewPanel = !!reviewReporter || needsAttention.length > 0;" in dashboard_js
    assert "Needs attention</span>" in dashboard_js
    assert "options.signal" in common_js
    assert 'id="reviews-for-reporter-card"${reviewsShellOpen ? " open" : ""}' in dashboard_js
    assert 'id="dashboard-reporter-stats-shell"${reporterStatsShellOpen ? " open" : ""}' in dashboard_js
    assert (
        dashboard_js.index("dashboard-status-grid")
        < dashboard_js.index("Awaiting your review")
        < dashboard_js.index("Needs attention")
    )
    assert dashboard_js.index("Awaiting your review") < dashboard_js.index("Reporter throughput")
    assert "repeat(10, minmax(88px, 1fr))" in dashboard_css
    assert "repeat(auto-fit, minmax(78px, 1fr))" in dashboard_css
    assert "dashboard-status-label" in dashboard_css
    assert ".dashboard-status-head" in dashboard_css
    assert "white-space: normal" in dashboard_css
    assert "position: absolute" in dashboard_css
    assert "z-index: 1" in dashboard_css
    assert ".dashboard-status-card-agent" in dashboard_css
    assert ".dashboard-agent-indicator" in dashboard_css
    assert ".dashboard-status-card.in-progress .dashboard-status-count" not in dashboard_css
    assert ".dashboard-status-card.awaiting-rebuild .dashboard-status-count" not in dashboard_css
    assert ".dashboard-title-row" in dashboard_css
    assert ".dashboard-scope-switch" in dashboard_css
    dashboard_scope_hover_css = re.search(
        r"\.dashboard-scope-switch button:hover:not\(:disabled\) \{(.*?)\}",
        dashboard_css,
        re.S,
    )
    assert dashboard_scope_hover_css and "background: var(--color-primary-hover)" in dashboard_scope_hover_css.group(1)
    assert dashboard_scope_hover_css and "color: white" in dashboard_scope_hover_css.group(1)
    assert ".dashboard-scope-switch button.active" in dashboard_css
    assert "#dash > .dashboard-collapsible-shell" in dashboard_css
    assert "${STATUS_FILTER_OPTIONS" in gaps_list_js
    assert "const filterShellOpen = filterShell ? filterShell.open : false;" in gaps_list_js
    assert "const GAPS_DEFAULT_LIMIT = 50;" in gaps_list_js
    assert 'renderPaginationControls("gaps"' in gaps_list_js
    assert 'bindPaginationControls(root, "gaps"' in gaps_list_js
    assert '<table class="table gaps-table mobile-card-table">' in gaps_list_js
    assert '<col class="gaps-col-name">' in gaps_list_js
    assert '<col class="gaps-col-status">' in gaps_list_js
    assert 'data-label="Name"' in gaps_list_js
    assert 'data-label="Updated"' in gaps_list_js
    assert 'class="gaps-status-cell"' in gaps_list_js
    assert ".gaps-table" in gaps_css
    assert "table-layout: fixed;" in gaps_css
    assert ".gaps-col-status" in gaps_css
    assert "width: 140px;" in gaps_css
    assert ".gaps-status-cell" in gaps_css
    assert "white-space: nowrap;" in gaps_css
    assert ".features-table" in gaps_css
    assert ".features-col-status" in gaps_css
    assert ".features-status-cell" in gaps_css
    assert ".feature-gaps-table" in gaps_css
    assert ".feature-gap-drag-handle" in gaps_css
    assert ".feature-gap-icon-btn" in gaps_css
    assert ".feature-gaps-table tr.drop-before td" in gaps_css
    assert ".feature-detail-modal" in modals_css
    assert "max-width: 1080px;" in modals_css
    assert "height: 75vh;" in modals_css
    assert "max-height: calc(100vh - 60px);" in modals_css
    assert ".feature-modal-head" in modals_css
    assert ".feature-modal-top-actions" in modals_css
    assert ".feature-gap-heading-actions" in modals_css
    assert ".feature-gap-add-btn" in modals_css
    assert "${STATUS_FILTER_OPTIONS" in changes_js
    assert "const filterShellOpen = filterShell ? filterShell.open : false;" in changes_js
    assert "const logsFilterShellOpen = logsFilterShell ? logsFilterShell.open : false;" in logs_js
    assert "flex: 0 0 auto;" in common_css
    assert "button.warn, .btn.warn {" in common_css
    assert "border-color: var(--warn);" in common_css
    assert "button.danger, .btn.danger {" in common_css
    assert "border-color: var(--error);" in common_css
    primary_button_hover_css = re.search(
        r"button:hover:not\(:disabled\), \.btn:hover, \.button-primary:hover \{(.*?)\}",
        common_css,
        re.S,
    )
    assert primary_button_hover_css and "background: var(--color-primary-hover)" in primary_button_hover_css.group(1)
    assert primary_button_hover_css and "color: white" in primary_button_hover_css.group(1)
    nav_action_hover_css = re.search(
        r"\.topbar-actions \.nav-action:hover \{(.*?)\}",
        base_css,
        re.S,
    )
    assert nav_action_hover_css and "background: var(--color-primary-hover)" in nav_action_hover_css.group(1)
    assert nav_action_hover_css and "color: white" in nav_action_hover_css.group(1)
    assert "button.toolbar-tab:hover:not(:disabled)" in toolbar_css
    assert ".toolbar-dock .toolbar-dock-toggle:hover:not(:disabled)" in toolbar_css
    assert ".files-pathbar" in toolbar_css
    assert ".files-path-label" in toolbar_css
    assert "grid-template-columns: auto minmax(0, 1fr) repeat(4, 34px);" in toolbar_css
    assert ".files-browser" in toolbar_css
    assert ".files-tree-panel" in toolbar_css
    assert ".files-tree-actions" in toolbar_css
    assert ".files-tree-search" in toolbar_css
    assert ".files-source-line" in toolbar_css
    assert ".files-load-more" in toolbar_css
    assert ".files-image-preview" in toolbar_css
    assert ".files-search-action" in toolbar_css
    assert "border: 1px solid var(--border);" in toolbar_css
    assert ".files-content-header .files-icon-btn" in toolbar_css
    assert "data-files-copy-content" in toolbar_js
    assert "const FILES_SEARCH_MAX_RESULTS = 20;" in toolbar_js
    assert "const FILES_SEARCH_DEBOUNCE_MS = 250;" in toolbar_js
    assert "let filesSearchAbortController = null;" in toolbar_js
    assert "const FILE_TEXT_CHUNK_BYTES = 128_000;" in toolbar_js
    assert "async function loadNextFileChunk()" in toolbar_js
    assert "function topFilesSearchFile(results)" in toolbar_js
    assert "function cancelFilesSearchRequest(" in toolbar_js
    assert "function normalizedFilesSearchSelectedIndex(" in toolbar_js
    assert "function moveFilesSearchSelection(delta)" in toolbar_js
    assert "function openSelectedFilesSearchResult()" in toolbar_js
    assert "openSelectedFile = false" in toolbar_js
    assert "openSelectedFile: true" in toolbar_js
    assert "{ signal: controller.signal }" in toolbar_js
    assert 'if (e?.name === "AbortError") return;' in toolbar_js
    assert "data-files-search-index" in toolbar_js
    assert "Enter to open" in toolbar_js
    assert "data-files-load-more" in toolbar_js
    assert "filesState.fileChunkLoading ? \"Loading...\" : \"Scroll to load more\"" in toolbar_js
    assert "file.kind === \"image\"" in toolbar_js
    assert "<img src=" in toolbar_js
    assert 'navigator.clipboard.writeText(filesState.file?.content || "")' in toolbar_js
    assert 'toast("File contents copied", "info")' in toolbar_js
    assert "background: #ffffff;" in toolbar_css
    assert "color: #111827;" in toolbar_css
    assert "#0f172a" not in toolbar_css
    assert "#e2e8f0" not in toolbar_css
    assert "color: white;" in toolbar_css
    assert '@route("GET", r"/api/files/tree")' in server_py
    assert '@route("GET", r"/api/files/read")' in server_py
    assert '@route("GET", r"/api/files/search")' in server_py
    assert '@route("GET", r"/api/diagnostics")' in server_py
    assert "diagnostics_ops.backend_diagnostics(" in api_py
    assert '@route("POST", r"/api/chat/start")' in server_py
    assert '@route("POST", r"/api/chat/([0-9A-Za-z]+)/input")' in server_py
    assert '@route("GET", r"/api/chat/([0-9A-Za-z]+)/read")' in server_py
    assert '@route("POST", r"/api/chat/([0-9A-Za-z]+)/stop")' in server_py
    assert "chat_ops.start(_backend_runner_call, body)" in api_py
    assert "chat_ops.input(_backend_runner_call, sid, body)" in api_py
    assert "chat_ops.read(_backend_runner_call, sid)" in api_py
    assert "chat_ops.stop(_backend_runner_call, sid)" in api_py
    assert "offset=int(_get_one(q, \"offset\", \"0\"))" in server_py
    assert "recursive = _get_one(q, \"recursive\", \"0\")" in server_py
    file_ops_py = (root / "refine_server/file_ops.py").read_text(encoding="utf-8")
    assert "FILE_TEXT_CHUNK_BYTES = 128_000" in file_ops_py
    assert "IMAGE_MIME_BY_EXT" in file_ops_py
    assert "def _fuzzy_path_score(" in file_ops_py
    assert "matches.sort(key=lambda item:" in file_ops_py
    assert "FILES_TREE_MAX_DEPTH = 3" in file_ops_py
    assert "FILES_TREE_MAX_ENTRIES = 200" in file_ops_py
    assert "return file_ops.tree(" in api_py
    assert "return file_ops.read(" in api_py
    assert "return file_ops.search(" in api_py
    assert "def files_tree(" in api_py
    assert "def files_read(" in api_py
    assert "def files_search(" in api_py
    assert "const CHANGES_DEFAULT_LIMIT = 50;" in changes_js
    assert "const CHANGES_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];" in changes_js
    assert '<table class="table changes-table mobile-card-table">' in changes_js
    assert 'data-label="Merge commit"' in changes_js
    assert 'managed-process-table mobile-card-table' in settings_js
    assert 'runner-workers-table mobile-card-table' in settings_js
    assert 'data-label="Details"' in settings_js
    assert 'performance-events-table mobile-card-table' in settings_js
    assert 'data-label="Resource"' in settings_js
    assert ".mobile-card-table td::before" in common_css
    assert "grid-template-columns: minmax(92px, 34%) minmax(0, 1fr)" in common_css
    assert 'renderPaginationControls("changes"' in changes_js
    assert 'bindPaginationControls(root, "changes"' in changes_js
    assert "const LOGS_DEFAULT_LIMIT = 50;" in logs_js
    assert 'renderPaginationControls("logs"' in logs_js
    assert 'bindPaginationControls(root, "logs"' in logs_js
    assert "const LOGS_DEFAULT_DIR = {" in logs_js
    assert '<table class="table logs-table mobile-card-table">' in logs_js
    assert '<td class="logs-entry-cell" colspan="5">' in logs_js
    assert '<div class="logs-entry-meta">' in logs_js
    assert '<div class="logs-entry-message logs-message-cell" data-label="Message">' in logs_js
    assert 'data-label="Message"' in logs_js
    assert "$$(\".table th.sortable\", root)" in logs_js
    assert 'renderPaginationControls("logs", pageMeta, entries.length, "entry", { boundaries: true })' in logs_js
    assert "params.set(\"sort\", f.sort);" in logs_js
    assert "params.set(\"dir\", f.dir);" in logs_js
    assert "recordUiError(msg, {" in common_js
    assert "function recordUiError(message, details = {})" in common_js
    assert 'fetch("/api/activity/ui-error"' in common_js
    assert "if (kind === \"error\" && !isDuplicateApiErrorToast(message))" in common_js
    assert '@route("POST", r"/api/activity/ui-error")' in server_py
    assert 'const BULK_STATUS_OPTIONS = [' in gaps_bulk_js
    assert '"__last_workflow_state"' in gaps_bulk_js
    assert "(Last workflow state)" in gaps_bulk_js
    assert 'value: "awaiting-rebuild", label: "awaiting-rebuild"' in gaps_bulk_js
    assert "failed merge" in gaps_bulk_js
    assert "attempts back to ready-merge" in gaps_bulk_js
    assert "failed QA attempts back to qa" in gaps_bulk_js
    assert "/retry-quality" in gaps_detail_js
    assert "isQualityRetryGap" in gaps_detail_js
    assert "renderQualitySummary" in gaps_detail_js
    assert 'forward: { label: "Review →"' not in gaps_detail_js
    assert 'todo:         { back:    { label: "← Backlog",  next: "backlog" } }' in gaps_detail_js
    assert '<span class="target-app-label">Application</span>' in index_html
    assert "function targetAppProjectLabel()" in target_app_js
    assert 'const app = apps.find((candidate) => candidate.path === current);' in target_app_js
    assert "if (lbl) lbl.textContent = projectLabel;" in target_app_js
    assert 'running: "running"' in target_app_js
    assert '"App: running"' not in target_app_js
    assert "label.replace(/^App: /, \"\")" not in target_app_js
    assert '@route("POST", r"/api/project/sync")' in (
        root / "refine_ui/server.py"
    ).read_text(encoding="utf-8")
    server_js = (root / "refine_ui/server.py").read_text(encoding="utf-8")
    assert '@route("GET", r"/api/project/templates")' in server_js
    assert '@route("POST", r"/api/project/scaffold")' in server_js
    assert '@route("GET", r"/api/guidance")' in server_js
    assert '@route("PUT", r"/api/guidance")' in server_js
    assert "def do_PUT" in server_js
    assert '"GET, POST, PATCH, PUT, DELETE, OPTIONS"' in server_js

    print("settings route tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

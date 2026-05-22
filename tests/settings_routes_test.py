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
            "settings_instances",
            "settings_reporters",
        )
    }
    settings_js = settings_core_js + "\n".join(settings_tab_files.values())
    router_js = (root / "refine_ui/static/js/router.js").read_text(
        encoding="utf-8",
    )
    target_app_js = (root / "refine_ui/static/js/target-app.js").read_text(
        encoding="utf-8",
    )
    common_js = (root / "refine_ui/static/js/common.js").read_text(
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
    gaps_bulk_js = (root / "refine_ui/static/js/features/gaps-bulk.js").read_text(
        encoding="utf-8",
    )
    gaps_detail_js = (root / "refine_ui/static/js/features/gaps-detail.js").read_text(
        encoding="utf-8",
    )
    changes_js = (root / "refine_ui/static/js/features/changes.js").read_text(
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
    dashboard_css = (root / "refine_ui/static/css/dashboard.css").read_text(
        encoding="utf-8",
    )

    settings_tab_block = re.search(
        r"const SETTINGS_TABS = \[(.*?)\];",
        settings_js,
        re.S,
    )
    assert settings_tab_block, "Settings tabs must be declared centrally"
    slugs = re.findall(r'slug:\s*"([^"]+)"', settings_tab_block.group(1))
    assert slugs == [
        "processes", "instances", "performance", "reporters",
        "guidance", "governance", "application", "runtime",
    ], slugs

    assert 'return { route: "settings", tab: parts[1] || null };' in router_js
    assert 'parsed.route === "settings" && !parsed.tab' in settings_js
    assert 'return first;' in settings_js
    assert 'if (slug === "system" || slug === "project") return "application";' in settings_js
    assert 'if (slug === "agents") return "guidance";' in settings_js
    assert "function activeSettingsTabFromRoute()" in settings_js
    assert "let _targetAppDraftDirty = false;" in settings_js
    assert "async function refreshSettings(options = {})" in settings_js
    assert "_targetAppDraftDirty &&" in settings_js
    assert 'document.querySelector(\'[data-tab-pane="application"].active\')' in settings_js
    assert 'href="#/system/${t.slug}"' in settings_js
    assert "<button class=\"settings-tab" not in settings_js
    assert '<div class="card settings-tab-card">${body}</div>' in settings_js
    assert settings_js.count('<div class="card') == 1
    save_button_ids = re.findall(
        r'<button id="([^"]+)">Save[^<]*</button>',
        settings_js,
    )
    assert save_button_ids == [
        "s-save-application",
        "s-save-runtime",
        "s-governance-save",
    ], save_button_ids
    assert "Feature flag changes are saved with Save runtime." in settings_js
    assert 'id="s-project-update-pulse"' in settings_js
    assert "project_update_pulse_interval_seconds" in settings_js
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
    assert 'api("POST", "/api/cache/rebuild", { background: true })' in settings_js
    assert "function drawSqliteCacheProgress" in settings_js
    assert "onProgress: drawSqliteCacheProgress" in settings_js
    assert 'slug: "performance"' in settings_js
    assert 'api("GET", "/api/performance")' in settings_js
    assert 'slug: "processes"' in settings_js
    assert 'api("GET", "/api/processes")' in settings_js
    assert '@route("GET", r"/api/processes")' in server_py
    assert "def process_summary" in api_py
    assert "function renderProcessesTab" in settings_js
    assert "function renderRunnerWorkRow" in settings_js
    assert "function renderRuntimeAgentCards" not in settings_js
    assert 'api("POST", "/api/performance/cleanup"' in settings_js
    assert '@route("GET", r"/api/performance")' in server_py
    assert '@route("POST", r"/api/performance/cleanup")' in server_py
    assert "def performance_summary" in api_py
    assert "snapshot[\"backend\"] = runtime.backend_info()" in api_py
    assert "def backend_info" in (root / "refine_ui/runtime.py").read_text(encoding="utf-8")
    assert "function backendProcessLabel" in settings_js
    assert "function performanceResourceLabel" in settings_js
    assert "<dt>Process model</dt>" in settings_js
    assert "<th>Resource</th>" in settings_js
    assert "function drawRuntimeRecovery(error)" in settings_js
    assert '@route("POST", r"/api/cache/rebuild")' in server_py
    assert "def rebuild_sqlite_cache" in api_py
    assert 'await showActionError(e, "SQLite cache rebuild failed");' in settings_js
    assert 'await showActionError(e, "Target app action failed");' in target_app_js
    assert 'id="s-agent-limit-pause"' in settings_js
    assert "agent_limit_pause_seconds" in settings_js
    assert '"30",    "30 seconds"' in settings_js
    assert '"60",    "1 minute"' in settings_js
    assert '"3600",  "1 hour"' in settings_js
    assert '"10800", "3 hours"' in settings_js
    assert "project_update_pulse_interval_seconds" in api_py
    runtime_save_body = settings_js.split('$("#s-save-runtime")?.addEventListener', 1)[1]
    runtime_save_body = runtime_save_body.split("\n  });", 1)[0]
    application_save_body = settings_js.split('$("#s-save-application")?.addEventListener', 1)[1]
    application_save_body = application_save_body.split("\n  });", 1)[0]
    feature_toggle_body = settings_js.split('$$("[data-feature-cell]").forEach', 1)[1]
    feature_toggle_body = feature_toggle_body.split('$$("[data-feature-clear]").forEach', 1)[0]
    assert 'api("POST", "/api/features/override"' in runtime_save_body
    assert 'worker_memory_limit_mb: $("#s-worker-memory").value' in runtime_save_body
    assert 'ui_memory_limit_mb: $("#s-ui-memory").value' in runtime_save_body
    assert 'worker_cpu_priority: $("#s-worker-cpu-priority").value' in runtime_save_body
    assert 'resource_isolation_mode: $("#s-resource-isolation").value' in runtime_save_body
    assert 'agent_limit_pause_seconds: $("#s-agent-limit-pause").value' in runtime_save_body
    assert 'api("POST", "/api/features/override"' not in feature_toggle_body
    assert 'await api("PATCH", "/api/settings", {' in application_save_body
    assert "_targetAppDraftDirty = false;" in application_save_body
    assert "await refreshSettings({ force: true });" in application_save_body
    for old_save_id in (
        'id="s-save"',
        'id="s-save-cli"',
        'id="s-save-scope"',
        'id="s-save-target"',
    ):
        assert old_save_id not in settings_js
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
    assert settings_tabs_css and "border-bottom" not in settings_tabs_css.group(1)
    assert settings_tab_css and "border: 1px solid var(--border)" in settings_tab_css.group(1)
    assert "text-decoration: none" in settings_tab_css.group(1)
    assert settings_tab_active_css and "border-bottom-color: var(--card)" in settings_tab_active_css.group(1)
    assert settings_section_css and "border-top: 1px solid var(--border)" in settings_section_css.group(1)

    for slug in slugs:
        assert settings_js.count(f'pane("{slug}",') == 1
    assert 'pane("project",' not in settings_js

    assert '<a href="#/system/processes" data-route="settings">System</a>' in index_html
    assert 'id="target-app-indicator" class="target-app-indicator"\n         href="#/system/processes"' in index_html
    for name in settings_tab_files:
        assert f'<script src="/static/js/features/{name}.js"></script>' in index_html
        assert index_html.index(f"/static/js/features/{name}.js") < index_html.index("/static/js/features/settings.js")
    assert 'slug: "instances"' in settings_js
    assert 'api("GET", "/api/instances")' in settings_js
    assert "const transferTargetInstances = instances.filter((inst) => !inst.archived);" in settings_js
    assert "Pause, cancel, and transfer" in settings_js
    assert "cancel_active: true" in settings_js
    assert "stopped ${r.stopped_processes || 0} processes" in settings_js
    processes_body = settings_tab_files["settings_processes"]
    application_body = settings_tab_files["settings_application"]
    runtime_body = settings_tab_files["settings_runtime"]
    assert 'id="s-rebuild-cache"' not in runtime_body
    assert 'id="s-target-run-rebuild"' in processes_body
    assert 'id="s-target-run-start"' in processes_body
    assert 'id="s-target-run-stop"' in processes_body
    assert 'id="s-target-health-now"' in processes_body
    assert processes_body.index('id="s-target-run-start"') < processes_body.index('id="s-target-run-stop"') < processes_body.index('id="s-target-run-rebuild"')
    assert 'class="target-app-action-slot"' in processes_body
    assert "function targetAppShowsStopAction" in processes_body
    assert "function setTargetAppActionVisible" in processes_body
    assert 'id="btn-pause"' in processes_body
    assert 'data-cancel-agent="' in processes_body
    assert 'data-stop-chat="' in processes_body
    assert "startBtn.style.display" not in settings_js
    assert "stopBtn.style.display" not in settings_js
    assert "setTargetAppActionVisible(startBtn, !showStop);" in settings_js
    assert "setTargetAppActionVisible(stopBtn, showStop);" in settings_js
    assert "startBtn.disabled = showStop || isRunning || inFlight || !snap.has_start_command;" in settings_js
    assert "stopBtn.disabled  = !showStop || isStopped || inFlight || !snap.has_stop_command;" in settings_js
    assert 'class="table process-table managed-process-table"' in processes_body
    assert 'class="table process-table agents-process-table"' in processes_body
    assert 'class="table process-table runner-workers-table"' in processes_body
    assert processes_body.index("<h3>Managed processes</h3>") < processes_body.index("<h3>Agents</h3>") < processes_body.index("<h3>Runner workers</h3>")
    managed_table = processes_body.split('class="table process-table managed-process-table"', 1)[1].split("</table>", 1)[0]
    runner_table = processes_body.split('class="table process-table runner-workers-table"', 1)[1].split("</table>", 1)[0]
    agents_table = processes_body.split('class="table process-table agents-process-table"', 1)[1].split("</table>", 1)[0]
    assert "<th>CPU priority</th>" in managed_table
    assert "<th>Max memory</th>" in managed_table
    assert "<th>Elapsed</th>" not in managed_table
    assert "<th>Idle</th>" not in managed_table
    assert "<th>Worker</th>" in runner_table
    assert "<th>Queue</th>" in runner_table
    assert 'data-runner-target-app-rebuild' in processes_body
    assert 'data-runner-target-app-generate' in processes_body
    assert 'data-runner-cache-rebuild' in processes_body
    assert 'data-runner-log-cleanup' in processes_body
    assert 'data-runner-log-cleanup-days' in processes_body
    assert 'api("POST", "/api/runner-workers/target-app-rebuilder/rebuild")' in settings_js
    assert 'api("POST", "/api/target-app/generate-instructions"' in settings_js
    assert 'api("POST", "/api/activity/cleanup"' in settings_js
    assert '@route("POST", r"/api/runner-workers/target-app-rebuilder/rebuild")' in server_py
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
    assert "refreshProcessesTabForChatChange" in (root / "refine_ui/static/js/features/chat.js").read_text(encoding="utf-8")
    assert 'idle: "idle"' in processes_body
    assert 'data-full-details="${htmlEscape(details)}"' in processes_body
    assert "function bindProcessDetailCells" in processes_body
    assert "function openProcessDetailsIfOverflowing" in processes_body
    assert 'modalAlert(details' in processes_body
    assert '<span class="role-pill ${kind === "agent"' not in processes_body
    assert '<span class="role-pill merger"' not in processes_body
    assert 'class="process-actions"><div class="actions">' in processes_body
    assert "<h3>Managed processes</h3>" in processes_body
    assert "<h3>Agents</h3>" in processes_body
    assert "<h3>Runner workers</h3>" in processes_body
    assert 'data-process-id="${htmlEscape(proc.id || "")}"' in processes_body
    assert '[data-process-id="target-app"]' in settings_js
    assert "Agent scheduler" in processes_body
    assert "runnerProcessDetails" in processes_body
    assert "<h3>Backend</h3>" not in processes_body
    assert 'id="target-app-status-block"' not in processes_body
    assert "<dt>Process model</dt>" not in processes_body
    assert "<dt>Runner transport</dt>" not in processes_body
    assert '<h3>Agents</h3>' not in runtime_body
    assert 'id="btn-pause"' not in runtime_body
    assert 'data-cancel-agent="' not in runtime_body
    assert 'id="s-target-run-start"' not in application_body
    assert 'id="s-project-sync-now"' not in processes_body
    assert ".process-table {" in common_css
    assert ".process-table .cpu-col { width: 86px; }" in common_css
    assert ".process-table .memory-col { width: 92px; }" in common_css
    assert ".managed-process-table .actions-col { width: 274px; }" in common_css
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
    assert ".target-app-action-slot" in common_css
    assert ".target-app-action-hidden" in common_css
    instances_body = settings_tab_files["settings_instances"].split('<h3>Transfer Gaps</h3>', 1)[0]
    assert '<button class="secondary" id="s-project-sync-now">Trigger sync repo</button>' in instances_body
    assert instances_body.index('id="s-project-sync-now"') < instances_body.index('id="instance-add"')
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
    assert 'await syncProjectUpdates();' in settings_js
    assert 'sseSource.addEventListener("project_updated"' in (
        root / "refine_ui/static/js/common.js"
    ).read_text(encoding="utf-8")
    assert 'id="s-target-rebuild-command"' in settings_js
    assert 'id="s-target-auto-rebuild"' in settings_js
    assert 'target_app_rebuild_command: $("#s-target-rebuild-command").value' in settings_js
    assert 'target_app_auto_rebuild: $("#s-target-auto-rebuild").value' in settings_js
    assert '"on_worktree_merge", "On worktree merge"' in settings_js
    assert '"nightly", "Nightly (midnight)"' in settings_js
    assert "Nightly (12 PM)" not in settings_js
    assert "target_app_auto_rebuild" in (root / "refine_ui/api.py").read_text(encoding="utf-8")
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
    assert '"awaiting-rebuild",' in common_js
    assert "const orderedStatuses = WORKFLOW_STATUSES;" in dashboard_js
    assert "dashboard-status-grid" in dashboard_js
    assert "const AGENT_MANAGED_DASHBOARD_STATUSES = new Set([" in dashboard_js
    assert '"todo",' in dashboard_js
    assert '"in-progress",' in dashboard_js
    assert '"ready-merge",' in dashboard_js
    assert '"awaiting-rebuild",' in dashboard_js
    assert "dashboard-status-card-agent" in dashboard_js
    assert "dashboard-agent-indicator" in dashboard_js
    assert "Agent-managed automation" in dashboard_js
    assert "dashboard-collapsible-shell" in dashboard_js
    assert "dashboardRefreshInFlight" in dashboard_js
    assert "dashboardRefreshQueued" in dashboard_js
    assert "DASHBOARD_REFRESH_TIMEOUT_MS" in dashboard_js
    assert "state.dashboardReviewSnapshot" in dashboard_js
    assert "scheduleDashboardRetry()" in dashboard_js
    assert "function dashboardScopeFromHash()" in dashboard_js
    assert "function dashboardHash(scope)" in dashboard_js
    assert "`/api/dashboard?instance=${instanceParam}`" in dashboard_js
    assert "`&instance=${instanceParam}&limit=200`" in dashboard_js
    assert "dashboard-title-row" in dashboard_js
    assert "dashboard-scope-switch" in dashboard_js
    assert "function wireDashboardScopeSwitch()" in dashboard_js
    assert "function syncDashboardScopeSwitch(scope)" in dashboard_js
    assert 'aria-label="Dashboard instance scope"' in dashboard_js
    assert 'btn.setAttribute("aria-pressed", active ? "true" : "false")' in dashboard_js
    assert 'data-dashboard-scope="current"' in dashboard_js
    assert 'data-dashboard-scope="all"' in dashboard_js
    assert "Stats for" not in dashboard_js
    assert "scopeLabel" not in dashboard_js
    assert "Current instance" not in dashboard_js
    assert "All instances" not in dashboard_js
    assert "instance: x.filter?.instance || scope" in dashboard_js
    assert "gapsHash({ status: s, instance: scope })" in dashboard_js
    assert "gapsHash({ reporter: row.dataset.reporter, instance: scope })" in dashboard_js
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
    assert dashboard_js.index("Awaiting your review") < dashboard_js.index("Reporter stats")
    assert "repeat(9, minmax(0, 1fr))" in dashboard_css
    assert "repeat(auto-fit, minmax(78px, 1fr))" in dashboard_css
    assert "dashboard-status-label" in dashboard_css
    assert ".dashboard-status-card-agent" in dashboard_css
    assert ".dashboard-agent-indicator" in dashboard_css
    assert ".dashboard-title-row" in dashboard_css
    assert ".dashboard-scope-switch" in dashboard_css
    assert ".dashboard-scope-switch button.active" in dashboard_css
    assert "#dash > .dashboard-collapsible-shell" in dashboard_css
    assert "${STATUS_FILTER_OPTIONS" in gaps_list_js
    assert 'renderPaginationControls("gaps"' in gaps_list_js
    assert 'bindPaginationControls(root, "gaps"' in gaps_list_js
    assert '<table class="table gaps-table">' in gaps_list_js
    assert '<col class="gaps-col-name">' in gaps_list_js
    assert '<col class="gaps-col-status">' in gaps_list_js
    assert 'class="gaps-status-cell"' in gaps_list_js
    assert ".gaps-table" in gaps_css
    assert "table-layout: fixed;" in gaps_css
    assert ".gaps-col-status" in gaps_css
    assert "width: 140px;" in gaps_css
    assert ".gaps-status-cell" in gaps_css
    assert "white-space: nowrap;" in gaps_css
    assert "${STATUS_FILTER_OPTIONS" in changes_js
    assert "const CHANGES_DEFAULT_LIMIT = 100;" in changes_js
    assert "const CHANGES_LIMIT_OPTIONS = [50, 100, 250, 500, 1000];" in changes_js
    assert 'renderPaginationControls("changes"' in changes_js
    assert 'bindPaginationControls(root, "changes"' in changes_js
    assert 'const BULK_STATUS_OPTIONS = [' in gaps_bulk_js
    assert '"awaiting-rebuild", "review",' in gaps_bulk_js
    assert '"done", "failed", "cancelled"' in gaps_bulk_js
    assert "skip in-progress and ready-merge" in gaps_bulk_js
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
    assert '@route("GET", r"/api/guidance")' in server_js
    assert '@route("PUT", r"/api/guidance")' in server_js
    assert "def do_PUT" in server_js
    assert '"GET, POST, PATCH, PUT, DELETE, OPTIONS"' in server_js

    print("settings route tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

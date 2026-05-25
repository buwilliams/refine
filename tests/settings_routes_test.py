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
    chat_js = (root / "refine_ui/static/js/features/chat.js").read_text(
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
    gaps_bulk_js = (root / "refine_ui/static/js/features/gaps-bulk.js").read_text(
        encoding="utf-8",
    )
    gaps_detail_js = (root / "refine_ui/static/js/features/gaps-detail.js").read_text(
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
    dashboard_css = (root / "refine_ui/static/css/dashboard.css").read_text(
        encoding="utf-8",
    )
    base_css = (root / "refine_ui/static/css/base.css").read_text(
        encoding="utf-8",
    )
    chat_css = (root / "refine_ui/static/css/chat.css").read_text(
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
        "guidance", "governance", "quality", "application", "runtime",
    ], slugs

    assert 'return { route: "settings", tab: parts[1] || null };' in router_js
    assert 'gaps_plan: renderGapPlan' in router_js
    assert 'if (parts[1] === "plan") return { route: "gaps_plan" };' in router_js
    assert 'prevRoute === "settings" && r.route === "settings"' in router_js
    assert "refreshSettingsTab(slug).catch(showActionError);" in router_js
    assert 'parsed.route === "settings" && !parsed.tab' in settings_js
    assert 'return first;' in settings_js
    assert 'if (slug === "system" || slug === "project") return "application";' in settings_js
    assert 'if (slug === "agents") return "guidance";' in settings_js
    assert "function activeSettingsTabFromRoute()" in settings_js
    assert "let _targetAppDraftDirty = false;" in settings_js
    assert "async function refreshSettings(options = {})" in settings_js
    assert "async function refreshActiveSettingsTab(options = {})" in settings_js
    assert "function updateSettingsTabContent(slug, body, bind)" in settings_js
    assert "if (card.innerHTML === next.innerHTML) return;" in settings_js
    assert "function reconcileSettingsNode(current, next)" in settings_js
    assert "_targetAppDraftDirty &&" in settings_js
    assert 'document.querySelector(\'[data-tab-pane="application"].active\')' in settings_js
    assert 'href="#/system/${t.slug}"' in settings_js
    assert "<button class=\"settings-tab" not in settings_js
    assert '<div class="card settings-tab-card">${body}</div>' in settings_js
    assert 'input[type="text"], input[type="number"], input[type="url"], textarea, select' in common_css
    assert settings_js.count('<div class="card') == 1
    save_button_ids = re.findall(
        r'<button id="([^"]+)">Save[^<]*</button>',
        settings_js,
    )
    assert save_button_ids == [], save_button_ids
    assert "Feature flag changes are saved automatically." in settings_js
    assert "function createSettingsAutosave" in settings_js
    assert "function bindSettingsAutosave" in settings_js
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
    assert 'api("GET", typeof performanceApiPath === "function"' in settings_js
    assert 'slug: "processes"' in settings_js
    assert 'api("GET", "/api/processes")' in settings_js
    assert '@route("GET", r"/api/processes")' in server_py
    assert "def process_summary" in api_py
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
    assert "snapshot[\"backend\"] = runtime.backend_info()" in api_py
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
    assert 'await showActionError(e, "SQLite cache rebuild failed");' in settings_js
    assert 'await showActionError(e, "Target app action failed");' in target_app_js
    assert 'id="s-agent-limit-pause"' in settings_js
    assert "agent_limit_pause_seconds" in settings_js
    assert '"30",    "30 seconds"' in settings_js
    assert '"60",    "1 minute"' in settings_js
    assert '"3600",  "1 hour"' in settings_js
    assert '"10800", "3 hours"' in settings_js
    assert "project_update_pulse_interval_seconds" in api_py
    assert '${cliOption("copilot", "GitHub Copilot")}' in settings_js
    assert '"copilot": "copilot login"' in api_py
    runtime_save_body = settings_js.split("async function autosaveSettingsRuntime", 1)[1]
    runtime_save_body = runtime_save_body.split("\nfunction bindSettingsRuntimeTab", 1)[0]
    application_save_body = settings_js.split("async function autosaveSettingsApplication", 1)[1]
    application_save_body = application_save_body.split("\nfunction applyGeneratedTargetAppConfig", 1)[0]
    feature_toggle_body = settings_js.split('$$("[data-feature-cell]").forEach', 1)[1]
    feature_toggle_body = feature_toggle_body.split('$$("[data-feature-clear]").forEach', 1)[0]
    assert 'api("POST", "/api/features/override"' in runtime_save_body
    assert 'worker_memory_limit_mb: $("#s-worker-memory").value' in runtime_save_body
    assert 'ui_memory_limit_mb: $("#s-ui-memory").value' in runtime_save_body
    assert 'worker_cpu_priority: $("#s-worker-cpu-priority").value' in runtime_save_body
    assert 'resource_isolation_mode: $("#s-resource-isolation").value' in runtime_save_body
    assert 'agent_limit_pause_seconds: $("#s-agent-limit-pause").value' in runtime_save_body
    assert 'api("POST", "/api/features/override"' not in feature_toggle_body
    assert 'await api("PATCH", "/api/settings", collectSettingsApplicationPayload())' in application_save_body
    assert "_targetAppDraftDirty = false;" in application_save_body
    assert 'await refreshSettingsTab("application", { force: true });' in application_save_body
    assert "autosaveSettingsGovernance" in settings_js
    assert "autosaveSettingsQuality" in settings_js
    assert "autosaveSettingsApplication" in settings_js
    assert "autosaveSettingsRuntime" in settings_js
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
    assert "border: 0" in settings_tab_css.group(1)
    assert "text-decoration: none" in settings_tab_css.group(1)
    assert settings_tab_active_css and "box-shadow: var(--shadow-sm)" in settings_tab_active_css.group(1)
    assert settings_section_css and "border-top: 1px solid var(--border)" in settings_section_css.group(1)

    for slug in slugs:
        assert settings_js.count(f'pane("{slug}",') == 1
    assert 'pane("project",' not in settings_js

    assert '<a href="#/system/processes" data-route="settings">System</a>' in index_html
    assert 'class="nav-menu nav-context-menu" id="nav-context-menu"' in index_html
    assert 'id="nav-context-app-summary">Application</span>' in index_html
    assert 'id="nav-context-reporter-summary">No reporter</span>' in index_html
    assert '<select id="global-reporter" aria-label="Reporter"></select>' in index_html
    assert 'class="nav-create-group"' in index_html
    assert 'id="btn-new-gap">+ New Gap</a>' in index_html
    assert 'class="nav-menu nav-create-menu" id="nav-create-menu"' in index_html
    assert 'id="btn-plan">Plan</a>' in index_html
    assert 'id="btn-import">Import gaps</a>' in index_html
    assert index_html.index('id="btn-plan"') < index_html.index('id="btn-import"')
    assert 'id="target-app-indicator" class="target-app-indicator nav-context-status"' in index_html
    assert 'id="agent-status-indicator" class="agent-status-indicator nav-status-indicator"' in index_html
    assert '<span class="agent-status-label">0</span>' in index_html
    assert index_html.index('id="nav-context-menu"') < index_html.index('id="agent-status-indicator"')
    assert index_html.index('id="agent-status-indicator"') < index_html.index('class="nav-create-group"')
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
    assert "openPlanChatDock();" in common_js
    assert "function openPlanChatDock()" in chat_js
    assert "{ purpose: \"plan\" }" in chat_js
    assert "Draft Gaps" in chat_js
    assert "function planTranscriptText(tab)" in chat_js
    assert "function openPlanDraftModalFromText(text)" in import_js
    assert "drawImportDrafts(root, annotated, close, { clearSession: false });" in import_js
    assert 'const createMenu = document.getElementById("nav-create-menu");' in common_js
    assert ".nav-context-summary" in base_css
    assert ".nav-create-group" in base_css
    assert ".nav-menu-panel" in base_css
    assert '.agent-status-indicator[data-state="running"] .target-app-dot' in base_css
    assert '.agent-status-indicator[data-state="paused"] .target-app-dot' in base_css
    assert '.agent-status-indicator[data-state="down"] .target-app-dot' in base_css
    assert '.nav-context-menu[data-state="running"] .nav-context-summary-dot' in base_css
    assert 'data-rmerge="${r.id}"' in settings_js
    assert "function openReporterMergeModal(source)" in settings_js
    assert 'api("POST", `/api/reporters/${b.dataset.rmerge}/merge`' in settings_js
    assert "Merging a reporter moves its Gaps to another" in settings_js
    assert 'def merge_reporter(rid: int, body: dict)' in api_py
    assert 'M_MERGE_REPORTER' in api_py
    assert '@route("POST", r"/api/reporters/(\\d+)/merge")' in server_py
    for name in settings_tab_files:
        assert f'<script src="/static/js/features/{name}.js"></script>' in index_html
        assert index_html.index(f"/static/js/features/{name}.js") < index_html.index("/static/js/features/settings.js")
    assert 'slug: "instances"' in settings_js
    assert 'api("GET", "/api/instances")' in settings_js
    assert "const transferTargetInstances = instances.filter((inst) => !inst.archived);" in settings_js
    assert "Pause, cancel, and transfer" in settings_js
    assert "cancel_active: true" in settings_js
    assert "in-progress, qa, ready-merge, and awaiting-rebuild" in settings_js
    assert "stopped ${r.stopped_processes || 0} processes" in settings_js
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
    assert 'refreshAgentStatusIndicator === "function"' in processes_body
    assert "function targetAppShowsStopAction" in processes_body
    assert "function setTargetAppActionVisible" in processes_body
    assert 'id="btn-pause"' in processes_body
    assert "scheduleProcessesTabRefreshes()" in processes_body
    assert "function scheduleProcessesTabRefreshes()" in processes_body
    assert '[data-tab-pane="processes"].active' in processes_body
    assert "refreshCurrentSettingsSurface()" in common_js
    assert 'if (state.currentRoute === "settings") {' in common_js
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
    assert processes_body.index("<h3>Managed processes</h3>") < processes_body.index("<h3>Agents</h3>") < processes_body.index("<h3>Runner workers</h3>")
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
    assert "refreshProcessesTabForChatChange" in chat_js
    assert 'proc.mode === "plan" ? "Plan chat"' in processes_body
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
    assert "await withButtonBusy(btn, \"Syncing…\", async () => {" in processes_body
    assert "await syncProjectUpdates();" in processes_body
    assert "await refreshProcessesSettingsTab({ force: true });" in processes_body
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
    assert 'id="s-target-app-url"' in settings_js
    assert '<input type="url" id="s-target-app-url"' in settings_js
    assert 'target_app_url: $("#s-target-app-url").value' in settings_js
    application_refresh_body = settings_core_js.split(
        '} else if (activeSlug === "application") {',
        1,
    )[1].split('} else if (activeSlug === "runtime") {', 1)[0]
    assert 'api("GET", "/api/settings")' in application_refresh_body
    assert "renderSettingsApplicationTab({" in application_refresh_body
    assert "target_app_url" not in common_js
    assert 'id="s-target-auto-rebuild"' in settings_js
    assert 'id="s-quality-enabled"' not in application_body
    assert 'id="s-quality-enabled"' in settings_js
    assert 'const qualityEnabled = String(quality.enabled || "0") === "1";' in settings_js
    assert 'aria-pressed="${qualityEnabled ? "true" : "false"}"' in settings_js
    assert 'class="${qualityEnabled ? "" : "warn"}"' in settings_js
    assert 'QA ${qualityEnabled ? "enabled" : "disabled"}' in settings_js
    assert 'enabled: $("#s-quality-enabled").dataset.enabled === "1" ? "1" : "0"' in settings_js
    assert 'btn.classList.toggle("warn", !enabled);' in settings_js
    assert 'btn.textContent = enabled ? "QA enabled" : "QA disabled";' in settings_js
    assert ".toggle-button.on" not in common_css
    assert '"enabled"] = db.get_setting(conn, "quality_enabled", "0") or "0"' in api_py
    assert 'api("GET", "/api/quality")' in settings_js
    assert 'api("PATCH", "/api/quality"' in settings_js
    assert '@route("GET", r"/api/quality")' in server_py
    assert '@route("PATCH", r"/api/quality")' in server_py
    assert "def quality_get" in api_py
    assert "def quality_save" in api_py
    assert 'target_app_rebuild_command: $("#s-target-rebuild-command").value' in settings_js
    assert 'target_app_auto_rebuild: $("#s-target-auto-rebuild").value' in settings_js
    assert '"on_worktree_merge", "On worktree merge"' in settings_js
    assert '"nightly", "Nightly (midnight)"' in settings_js
    assert "Nightly (12 PM)" not in settings_js
    api_source = (root / "refine_ui/api.py").read_text(encoding="utf-8")
    assert "target_app_auto_rebuild" in api_source
    assert '"target_app_url"' in api_source
    assert '"app_url": settings.get("target_app_url") or ""' in api_source
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
    assert "const orderedStatuses = WORKFLOW_STATUSES;" in dashboard_js
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
    assert ".dashboard-title-row" in dashboard_css
    assert ".dashboard-scope-switch" in dashboard_css
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
    assert "${STATUS_FILTER_OPTIONS" in changes_js
    assert "const filterShellOpen = filterShell ? filterShell.open : false;" in changes_js
    assert "const logsFilterShellOpen = logsFilterShell ? logsFilterShell.open : false;" in logs_js
    assert "flex: 0 0 auto;" in common_css
    assert "button.warn, .btn.warn {" in common_css
    assert "border-color: var(--warn);" in common_css
    assert "button.danger, .btn.danger {" in common_css
    assert "border-color: var(--error);" in common_css
    assert "button.chat-tab:hover:not(:disabled)" in chat_css
    assert ".chat-dock .chat-dock-toggle:hover:not(:disabled)" in chat_css
    assert "color: white;" in chat_css
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
    assert '@route("GET", r"/api/guidance")' in server_js
    assert '@route("PUT", r"/api/guidance")' in server_js
    assert "def do_PUT" in server_js
    assert '"GET, POST, PATCH, PUT, DELETE, OPTIONS"' in server_js

    print("settings route tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

"""Static checks for Settings tab deep-link routes."""
from __future__ import annotations

import re
import sys
from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    settings_js = (root / "refine_ui/static/js/features/settings.js").read_text(
        encoding="utf-8",
    )
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
        "application", "reporters", "instances", "runtime", "guidance", "governance",
    ], slugs

    assert 'return { route: "settings", tab: parts[1] || null };' in router_js
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
    assert "project_update_pulse_interval_seconds" in (
        root / "refine_ui/api.py"
    ).read_text(encoding="utf-8")
    runtime_save_body = settings_js.split('$("#s-save-runtime")?.addEventListener', 1)[1]
    runtime_save_body = runtime_save_body.split("\n  });", 1)[0]
    application_save_body = settings_js.split('$("#s-save-application")?.addEventListener', 1)[1]
    application_save_body = application_save_body.split("\n  });", 1)[0]
    feature_toggle_body = settings_js.split('$$("[data-feature-cell]").forEach', 1)[1]
    feature_toggle_body = feature_toggle_body.split('$$("[data-feature-clear]").forEach', 1)[0]
    assert 'api("POST", "/api/features/override"' in runtime_save_body
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

    assert 'href="#/system/application"' in index_html
    assert 'slug: "instances"' in settings_js
    assert 'api("GET", "/api/instances")' in settings_js
    assert "const transferTargetInstances = instances.filter((inst) => !inst.archived);" in settings_js
    assert "Pause, cancel, and transfer" in settings_js
    assert "cancel_active: true" in settings_js
    assert "stopped ${r.stopped_processes || 0} processes" in settings_js
    assert 'id="s-target-run-rebuild"' in settings_js
    current_status_body = settings_js.split("<h3>Current status</h3>", 1)[1].split("<h3>Target application</h3>", 1)[0]
    assert 'id="s-project-sync-now"' not in current_status_body
    instances_body = settings_js.split('${pane("instances", `', 1)[1].split('<h3>Transfer Gaps</h3>', 1)[0]
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
    assert "repeat(9, minmax(0, 1fr))" in dashboard_css
    assert "repeat(auto-fit, minmax(78px, 1fr))" in dashboard_css
    assert "dashboard-status-label" in dashboard_css
    assert "${STATUS_FILTER_OPTIONS" in gaps_list_js
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
    assert "const BULK_STATUS_OPTIONS = WORKFLOW_STATUSES;" in gaps_bulk_js
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

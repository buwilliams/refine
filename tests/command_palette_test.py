"""Static checks for the browser command palette contract."""
from __future__ import annotations

from pathlib import Path


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    index_html = (root / "refine_ui/static/index.html").read_text(encoding="utf-8")
    registry_js = (root / "refine_ui/static/js/command-registry.js").read_text(
        encoding="utf-8",
    )
    palette_js = (root / "refine_ui/static/js/command-palette.js").read_text(
        encoding="utf-8",
    )
    commands_js = (root / "refine_ui/static/js/commands.js").read_text(
        encoding="utf-8",
    )
    init_js = (root / "refine_ui/static/js/init.js").read_text(encoding="utf-8")
    base_css = (root / "refine_ui/static/css/base.css").read_text(encoding="utf-8")
    modals_css = (root / "refine_ui/static/css/modals.css").read_text(
        encoding="utf-8",
    )
    common_js = (root / "refine_ui/static/js/common.js").read_text(encoding="utf-8")
    gaps_list_js = (root / "refine_ui/static/js/features/gaps-list.js").read_text(
        encoding="utf-8",
    )
    toolbar_js = (root / "refine_ui/static/js/features/toolbar.js").read_text(
        encoding="utf-8",
    )
    system_tab_js = {
        name: (root / f"refine_ui/static/js/features/{name}.js").read_text(
            encoding="utf-8",
        )
        for name in (
            "settings",
            "settings_application",
            "settings_reporters",
            "settings_instances",
            "settings_quality",
            "settings_governance",
            "settings_performance",
        )
    }

    assert '<script src="/static/js/command-registry.js"></script>' in index_html
    assert '<script src="/static/js/commands.js"></script>' in index_html
    assert '<script src="/static/js/command-palette.js"></script>' in index_html
    assert index_html.index("/static/js/command-registry.js") < index_html.index("/static/js/features/dashboard.js")
    assert index_html.index("/static/js/features/settings.js") < index_html.index("/static/js/commands.js")
    assert index_html.index("/static/js/commands.js") < index_html.index("/static/js/command-palette.js")
    assert index_html.index("/static/js/command-palette.js") < index_html.index("/static/js/init.js")
    assert 'id="btn-command-palette"' in index_html
    assert 'data-command-shortcut' in index_html
    assert 'class="nav-bug-icon"' in index_html
    assert (root / "refine_ui/static/images/bug.svg").exists()

    assert "function registerCommand(def)" in registry_js
    assert "function runCommand(id, options = {})" in registry_js
    assert "function bindCommand(target, id, options = {})" in registry_js
    assert "function searchCommands(query = \"\"" in registry_js
    assert "window.RefineCommands" in registry_js
    assert "const runParams = { ...ctx, ...params };" in registry_js
    assert "return await command.run(runParams, ctx);" in registry_js

    assert "function initCommandPalette()" in palette_js
    assert 'String(e.key || "").toLowerCase() === "k"' in palette_js
    assert "e.ctrlKey || e.metaKey" in palette_js
    assert "e.altKey" in palette_js
    assert "e.shiftKey" in palette_js
    assert '.modal-backdrop:not(.command-palette-backdrop)' in palette_js
    assert "openCommandPalette()" in palette_js
    assert "searchCommands(input.value)" in palette_js
    assert "runCommand(item.command.id" in palette_js
    assert 'addEventListener("mousemove"' not in palette_js
    assert "executeItem(currentResults[idx])" in palette_js

    for command_id in (
        "gap.new",
        "gap.import",
        "refine.issue.request",
        "plan.open",
        "toolbar.toggle",
        "toolbar.fullscreen",
        "files.open",
        "files.search",
        "gaps.bulk.move",
        "gaps.bulk.failed_back",
        "system.cache.rebuild",
        "target_app.generate",
        "quality.regression.new",
        "quality.regression.run",
        "runtime.recheck_auth",
    ):
        assert f'id: "{command_id}"' in commands_js
    assert 'aliases: ["bulk_move", "bulk-move", "move-gaps"]' in commands_js
    assert 'new URL("https://github.com/buwilliams/refine/issues/new")' in commands_js
    assert 'url.searchParams.set("title", title)' in commands_js
    assert 'url.searchParams.set("body", description)' in commands_js
    assert "function openRefineIssueRequestModal()" in commands_js
    assert 'This opens GitHub in a new tab with your title and description pre-filled.' in commands_js
    assert 'window.open(' in commands_js
    assert 'update: { status: "__last_workflow_state" }' in commands_js
    assert 'await openPlanChatDock({ initialPrompt: prompt || "" });' in commands_js
    assert 'id: "files.open"' in commands_js
    assert 'id: "files.search"' in commands_js
    assert 'group: "Toolbar"' in commands_js
    assert 'aliases: ["files", "open-files", "file-browser"]' in commands_js
    assert 'aliases: ["search-files", "find-file", "file-search"]' in commands_js
    assert 'openFilesToolbar({ path: path || "" })' in commands_js
    assert 'openFilesToolbar({ search: search || "", focusSearch: true })' in commands_js
    assert "enabled: () => planHasAgentResponse(chatState.tabs.plan)" in commands_js
    assert "Wait for the agent to respond before drafting Gaps." in commands_js
    assert 'aliases: ["regression_new", "new-regression", "create-regression"]' in commands_js
    assert 'openRegressionCreateModal(prompt || "", button)' in commands_js
    assert 'title: "Quality: run regressions on current checkout"' in commands_js
    assert 'button = null' in system_tab_js["settings"]
    assert 'await withButtonBusy(button, "Copying...", async () => {' in system_tab_js["settings"]

    assert 'if (typeof initCommandPalette === "function") initCommandPalette();' in init_js
    assert 'runCommand("gap.new")' in common_js
    assert 'runCommand("plan.open")' in common_js
    assert 'runCommand("gap.import")' in common_js
    assert 'runCommand("refine.issue.request")' in common_js
    assert 'bindCommand("#bulk-set-status", "gaps.bulk.status")' in gaps_list_js
    assert 'bindCommand("#s-target-generate-ai", "target_app.generate");' in system_tab_js["settings_application"]
    assert 'await withButtonBusy(button, "Generating...", async () => {' in commands_js
    assert 'api("POST", "/api/target-app/generate-instructions", { kind: "all" })' in commands_js
    assert 'await withButtonBusy(b, "Merging...", async () => {' in system_tab_js["settings_reporters"]
    assert 'await withButtonBusy(btn, "Adding...", async () => {' in system_tab_js["settings_reporters"]
    assert 'await withButtonBusy(b, "Activating...", async () => {' in system_tab_js["settings_instances"]
    assert 'await withButtonBusy(btn, "Transferring...", async () => {' in system_tab_js["settings_instances"]
    assert 'await withButtonBusy(btn, "Saving...", async () => {' in system_tab_js["settings_quality"]
    assert 'return await withButtonBusy(button, "Creating...", async () => {' in system_tab_js["settings_quality"]
    assert 'await withButtonBusy(btn, "Generating…", async () => {' in system_tab_js["settings_governance"]
    assert 'await withButtonBusy(e.currentTarget, "Refreshing…", async () => {' in system_tab_js["settings_performance"]
    assert 'withButtonBusy($("#' not in "\n".join(system_tab_js.values())
    assert "async function openPlanChatDock(options = {})" in toolbar_js
    assert 'label: "Files", mode: "files"' in toolbar_js
    assert '<span class="toolbar-dock-label">TOOLBAR</span>' in toolbar_js
    assert 'for="files-path-input" class="files-path-label">Path</label>' in toolbar_js
    assert 'id="files-path-input"' in toolbar_js
    assert 'data-files-copy' in toolbar_js
    assert 'data-files-clear' in toolbar_js
    assert 'data-files-paste' not in toolbar_js
    assert 'id="files-search-input"' in toolbar_js
    assert 'data-files-go' in toolbar_js
    assert 'data-files-refresh' in toolbar_js
    assert 'data-files-expand-all' in toolbar_js
    assert 'data-files-clear-tree' in toolbar_js
    assert 'data-files-collapse-all' in toolbar_js
    assert 'expand: \'<path d="m6 9 6 6 6-6"></path>\'' in toolbar_js
    assert 'collapse: \'<path d="m18 15-6-6-6 6"></path>\'' in toolbar_js
    assert "const FILES_TREE_MAX_DEPTH = 3;" in toolbar_js
    assert "const FILES_TREE_MAX_ENTRIES = 200;" in toolbar_js
    assert "const FILES_SEARCH_MAX_RESULTS = 20;" in toolbar_js
    assert 'treeRootPath: ""' in toolbar_js
    assert 'pathInputValue: ""' in toolbar_js
    assert 'class="files-tree"' in toolbar_js
    assert 'class="files-content"' in toolbar_js
    assert 'class="files-line-number"' in toolbar_js
    assert "async function expandAllFilesTree()" in toolbar_js
    assert "function collapseAllFilesTree()" in toolbar_js
    assert "async function runFilesSearch(query" in toolbar_js
    assert "function scheduleFilesSearch(query)" in toolbar_js
    assert "function normalizedFilesSearchSelectedIndex(" in toolbar_js
    assert "function moveFilesSearchSelection(delta)" in toolbar_js
    assert "function openSelectedFilesSearchResult()" in toolbar_js
    render_panel_body = toolbar_js.split("function renderFilesPanel()", 1)[1].split("function renderFilesTreePanel()", 1)[0]
    assert 'const inputPath = filesState.pathInputValue || "";' in render_panel_body
    assert "filesState.selectedPath || filesState.path" not in render_panel_body
    bind_panel_body = toolbar_js.split("function bindFilesPanel(root)", 1)[1].split("function scheduleFilesSearch", 1)[0]
    assert 'filesState.pathInputValue = e.target.value || "";' in bind_panel_body
    assert "clearFilesPathInput()" in bind_panel_body
    assert "clearFilesTreeView()" in bind_panel_body
    empty_search_block = toolbar_js.split("if (!query) {", 1)[1].split("return;", 1)[0]
    assert "drawToolbar();" in empty_search_block
    assert "if (refocus) focusFilesSearchInput();" in empty_search_block
    assert "function highlightFileLine" in toolbar_js
    tree_panel_body = toolbar_js.split("function renderFilesTreePanel()", 1)[1].split("function renderFilesSearchResults()", 1)[0]
    assert 'renderFilesTree(filesState.treeRootPath || "")' in tree_panel_body
    navigate_body = toolbar_js.split("async function navigateFilesPath(rawPath)", 1)[1].split("async function refreshFilesPanel()", 1)[0]
    assert 'filesState.pathInputValue = String(rawPath || "");' in navigate_body
    assert "await loadFilesDirectory(path, { expand: true, redraw: false });" in navigate_body
    assert 'filesState.treeRootPath = result.path || "";' in navigate_body
    assert "await loadFile(path);" in navigate_body
    assert navigate_body.index("await loadFilesDirectory") < navigate_body.index("await loadFile(path);")
    refresh_body = toolbar_js.split("async function refreshFilesPanel()", 1)[1].split("async function loadFilesDirectory", 1)[0]
    assert 'const dir = filesState.treeRootPath || "";' in refresh_body
    assert 'delete filesState.entriesByPath[dir];' in refresh_body
    assert 'await loadFilesDirectory(dir, { expand: true, redraw: false }).catch(() => {});' in refresh_body
    assert 'await loadFile(filesState.selectedPath, { redraw: false }).catch(() => {});' in refresh_body
    assert refresh_body.index("await loadFilesDirectory") < refresh_body.index("await loadFile(")
    clear_body = toolbar_js.split("async function clearFilesPathInput()", 1)[1].split("async function refreshFilesPanel()", 1)[0]
    assert 'filesState.pathInputValue = "";' in clear_body
    assert 'filesState.treeRootPath = "";' in clear_body
    assert 'filesState.expanded = new Set([""]);' in clear_body
    assert 'delete filesState.entriesByPath[""];' in clear_body
    assert 'await loadFilesDirectory("", { expand: true, redraw: true });' in clear_body
    expand_body = toolbar_js.split("async function expandAllFilesTree()", 1)[1].split("function collapseAllFilesTree()", 1)[0]
    assert 'const treeRoot = filesState.treeRootPath || "";' in expand_body
    assert '`path=${encodeURIComponent(treeRoot)}`' in expand_body
    assert 'filesState.treeRootPath = result.path || "";' in expand_body
    collapse_body = toolbar_js.split("function collapseAllFilesTree()", 1)[1].split("async function openFilesToolbar", 1)[0]
    assert 'filesState.expanded = new Set([filesState.treeRootPath || ""]);' in collapse_body
    clear_tree_body = toolbar_js.split("async function clearFilesTreeView()", 1)[1].split("async function openFilesToolbar", 1)[0]
    assert "clearTimeout(filesSearchTimer);" in clear_tree_body
    assert 'const treeRoot = filesState.treeRootPath || "";' in clear_tree_body
    assert 'filesState.searchQuery = "";' in clear_tree_body
    assert "filesState.searchResults = null;" in clear_tree_body
    assert "filesState.searchSelectedIndex = -1;" in clear_tree_body
    assert "filesState.searchLoading = false;" in clear_tree_body
    assert 'filesState.path = treeRoot;' in clear_tree_body
    assert 'filesState.selectedPath = treeRoot;' in clear_tree_body
    assert "filesState.file = null;" in clear_tree_body
    assert "filesState.expanded = new Set([treeRoot]);" in clear_tree_body
    assert "delete filesState.entriesByPath[treeRoot];" in clear_tree_body
    assert "await loadFilesDirectory(treeRoot, { expand: true, redraw: true });" in clear_tree_body
    normalize_body = toolbar_js.split("function normalizeFilesPath(path)", 1)[1].split("function parentPath(path)", 1)[0]
    assert '.filter((part) => part && part !== ".")' in normalize_body
    assert 'api("GET", `/api/files/tree?path=${encodeURIComponent(path)}`)' in toolbar_js
    assert '"recursive=1"' in toolbar_js
    assert "/api/files/search?q=${encodeURIComponent(query)}&max_entries=${FILES_SEARCH_MAX_RESULTS}" in toolbar_js
    assert "/api/files/read?path=${encodeURIComponent(path)}&offset=0&limit=${FILE_TEXT_CHUNK_BYTES}" in toolbar_js
    assert "async function sendChatText(text)" in toolbar_js
    assert "function planHasAgentResponse(tab)" in toolbar_js

    assert ".nav-command-button" in base_css
    assert ".nav-issue-button" in base_css
    assert ".nav-bug-icon" in base_css
    assert "color: var(--color-text-muted)" in base_css
    assert ".nav-issue-button:hover:not(:disabled)" in base_css
    assert "color: var(--color-text)" in base_css
    assert "width: 26px;" in base_css
    assert "height: 26px;" in base_css
    assert 'mask: url("/static/images/bug.svg") center / contain no-repeat;' in base_css
    assert ".nav-bug-icon::before" not in base_css
    assert 'fill="currentColor"' in (
        root / "refine_ui/static/images/bug.svg"
    ).read_text(encoding="utf-8")
    assert ".command-palette-backdrop" in modals_css
    assert ".refine-issue-modal" in modals_css
    assert ".regression-create-modal" in modals_css
    assert ".command-palette-row.selected" in modals_css
    assert ".command-palette-row:hover:not(:disabled)" in modals_css
    assert "color: var(--color-primary)" in modals_css
    assert "@media (max-width: 760px)" in modals_css

    print("command palette tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

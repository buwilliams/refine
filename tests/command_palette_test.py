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
    chat_js = (root / "refine_ui/static/js/features/chat.js").read_text(
        encoding="utf-8",
    )

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
        "gaps.bulk.move",
        "gaps.bulk.failed_back",
        "system.cache.rebuild",
        "target_app.generate",
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

    assert 'if (typeof initCommandPalette === "function") initCommandPalette();' in init_js
    assert 'runCommand("gap.new")' in common_js
    assert 'runCommand("plan.open")' in common_js
    assert 'runCommand("gap.import")' in common_js
    assert 'runCommand("refine.issue.request")' in common_js
    assert 'bindCommand("#bulk-set-status", "gaps.bulk.status")' in gaps_list_js
    assert "async function openPlanChatDock(options = {})" in chat_js
    assert "async function sendChatText(text)" in chat_js

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
    assert ".command-palette-row.selected" in modals_css
    assert ".command-palette-row:hover:not(:disabled)" in modals_css
    assert "color: var(--color-primary)" in modals_css
    assert "@media (max-width: 760px)" in modals_css

    print("command palette tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

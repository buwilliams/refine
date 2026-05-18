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
    index_html = (root / "refine_ui/static/index.html").read_text(
        encoding="utf-8",
    )
    common_css = (root / "refine_ui/static/css/common.css").read_text(
        encoding="utf-8",
    )

    settings_tab_block = re.search(
        r"const SETTINGS_TABS = \[(.*?)\];",
        settings_js,
        re.S,
    )
    assert settings_tab_block, "Settings tabs must be declared centrally"
    slugs = re.findall(r'slug:\s*"([^"]+)"', settings_tab_block.group(1))
    assert slugs == ["application", "reporters", "governance", "runtime"], slugs

    assert 'return { route: "settings", tab: parts[1] || null };' in router_js
    assert 'if (slug === "project") return "application";' in settings_js
    assert "function activeSettingsTabFromRoute()" in settings_js
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
    runtime_save_body = settings_js.split('$("#s-save-runtime")?.addEventListener', 1)[1]
    runtime_save_body = runtime_save_body.split("\n  });", 1)[0]
    feature_toggle_body = settings_js.split('$$("[data-feature-cell]").forEach', 1)[1]
    feature_toggle_body = feature_toggle_body.split('$$("[data-feature-clear]").forEach', 1)[0]
    assert 'api("POST", "/api/features/override"' in runtime_save_body
    assert 'api("POST", "/api/features/override"' not in feature_toggle_body
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

    assert 'href="#/system/application"' in index_html
    assert "#/system/project" not in index_html

    print("settings route tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

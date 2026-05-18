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
    assert slugs == ["project", "reporters", "governance", "runtime"], slugs

    assert 'return { route: "settings", tab: parts[1] || null };' in router_js
    assert "function activeSettingsTabFromRoute()" in settings_js
    assert 'href="#/system/${t.slug}"' in settings_js
    assert "<button class=\"settings-tab" not in settings_js
    assert '<div class="card settings-tab-card">${body}</div>' in settings_js
    assert settings_js.count('<div class="card') == 1
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

    assert 'href="#/system/project"' in index_html

    print("settings route tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

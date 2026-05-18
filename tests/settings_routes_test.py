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

    for slug in slugs:
        assert f'pane("{slug}",' in settings_js

    assert 'href="#/system/project"' in index_html

    print("settings route tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

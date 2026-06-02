"""Clone-local app registry behavior."""
from __future__ import annotations

import json
import shutil
import tempfile
from pathlib import Path

import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


def test_registry_omits_missing_paths_and_prunes_on_write() -> None:
    from refine_server import project_registry

    tmp = Path(tempfile.mkdtemp(prefix="refine-project-registry-"))
    try:
        clone = tmp / "refine"
        clone.mkdir()
        live = tmp / "live-client"
        live.mkdir()
        missing = tmp / "missing-client"
        registry = clone / "run" / "8081" / "apps.json"
        registry.parent.mkdir(parents=True)
        registry.write_text(
            json.dumps({
                "version": 1,
                "active_app": "",
                "apps": [
                    {"name": "missing-client", "path": str(missing)},
                    {"name": "live-client", "path": str(live)},
                ],
            }),
            encoding="utf-8",
        )

        assert [app["path"] for app in project_registry.list_apps(clone, port=8081)] == [
            str(live.resolve())
        ]

        current = tmp / "current-client"
        current.mkdir()
        apps = project_registry.upsert_app(clone, current, make_current=True, port=8081)
        assert [app["path"] for app in apps] == [
            str(live.resolve()),
            str(current.resolve()),
        ]
        assert project_registry.active_app(clone, port=8081) == current.resolve()
        assert project_registry.active_app(clone, port=8082) is None

        persisted = json.loads(registry.read_text(encoding="utf-8"))
        assert persisted["active_app"] == str(current.resolve())
        assert [app["path"] for app in persisted["apps"]] == [
            str(live.resolve()),
            str(current.resolve()),
        ]
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main() -> int:
    test_registry_omits_missing_paths_and_prunes_on_write()
    print("project registry tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

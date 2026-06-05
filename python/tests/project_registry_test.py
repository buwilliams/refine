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


def test_legacy_binding_migration_quarantines_old_run() -> None:
    from refine_server import config, project_registry

    tmp = Path(tempfile.mkdtemp(prefix="refine-project-registry-migrate-"))
    try:
        clone = tmp / "refine"
        clone.mkdir()
        client = tmp / "client"
        client.mkdir()
        config.write_defaults(client / ".refine")
        old_run = clone / "run"
        old_run.mkdir()
        (old_run / "ui-8080.pid").write_text("123\n", encoding="utf-8")
        (clone / ".refine-binding").write_text(f"{client}\n", encoding="utf-8")

        assert project_registry.active_app(clone, port=8080) == client.resolve()

        registry = clone / "run" / "8080" / "apps.json"
        assert registry.is_file()
        assert str(client.resolve()) in registry.read_text(encoding="utf-8")
        assert (clone / "run.bak" / "ui-8080.pid").read_text(encoding="utf-8") == "123\n"
        assert not (clone / "run" / "ui-8080.pid").exists()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def test_legacy_binding_does_not_repopulate_clean_run() -> None:
    from refine_server import config, project_registry

    tmp = Path(tempfile.mkdtemp(prefix="refine-project-registry-clean-"))
    try:
        clone = tmp / "refine"
        clone.mkdir()
        client = tmp / "client"
        client.mkdir()
        config.write_defaults(client / ".refine")
        (clone / ".refine-binding").write_text(f"{client}\n", encoding="utf-8")
        (clone / ".refine-apps.json").write_text(
            json.dumps({"apps": [{"name": "client", "path": str(client)}]}),
            encoding="utf-8",
        )

        assert project_registry.list_apps(clone, port=8080) == []
        assert project_registry.active_app(clone, port=8080) is None
        assert not (clone / "run" / "8080" / "apps.json").exists()

        (clone / "run").mkdir()
        assert project_registry.list_apps(clone, port=8080) == []
        assert project_registry.active_app(clone, port=8080) is None
        assert not (clone / "run.bak").exists()
        assert not (clone / "run" / "8080" / "apps.json").exists()

        (clone / "run" / "8080").mkdir()
        (clone / "run" / "8080" / "supervisor.log").write_text("", encoding="utf-8")
        assert project_registry.list_apps(clone, port=8080) == []
        assert project_registry.active_app(clone, port=8080) is None
        assert not (clone / "run.bak").exists()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def test_legacy_run_backup_collision_uses_numbered_backup() -> None:
    from refine_server import config, project_registry

    tmp = Path(tempfile.mkdtemp(prefix="refine-project-registry-migrate-bak-"))
    try:
        clone = tmp / "refine"
        clone.mkdir()
        client = tmp / "client"
        client.mkdir()
        config.write_defaults(client / ".refine")
        (clone / "run").mkdir()
        (clone / "run" / "ui-8080.pid").write_text("123\n", encoding="utf-8")
        (clone / "run.bak").mkdir()
        (clone / "run.bak" / "kept").write_text("kept\n", encoding="utf-8")
        (clone / ".refine-binding").write_text(f"{client}\n", encoding="utf-8")

        assert project_registry.active_app(clone, port=8080) == client.resolve()

        assert (clone / "run.bak" / "kept").read_text(encoding="utf-8") == "kept\n"
        assert (clone / "run.bak.1" / "ui-8080.pid").read_text(encoding="utf-8") == "123\n"
        assert (clone / "run" / "8080" / "apps.json").is_file()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def main() -> int:
    test_registry_omits_missing_paths_and_prunes_on_write()
    test_legacy_binding_migration_quarantines_old_run()
    test_legacy_binding_does_not_repopulate_clean_run()
    test_legacy_run_backup_collision_uses_numbered_backup()
    print("project registry tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

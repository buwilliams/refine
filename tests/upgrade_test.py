"""Upgrade tag discovery checks."""
from __future__ import annotations

import shutil
import subprocess
import tempfile
from pathlib import Path

from refine_server import upgrade


def _git(repo: Path, *args: str) -> None:
    subprocess.run(["git", "-C", str(repo), *args], check=True)


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="refine-upgrade-test-"))
    try:
        origin = tmp / "origin.git"
        source = tmp / "source"
        installed = tmp / "installed"
        subprocess.run(["git", "init", "--bare", "-q", str(origin)], check=True)
        subprocess.run(["git", "init", "-q", str(source)], check=True)
        _git(source, "config", "user.name", "Refine Test")
        _git(source, "config", "user.email", "refine@example.test")
        _git(source, "remote", "add", "origin", str(origin))

        (source / "marker.txt").write_text("1\n", encoding="utf-8")
        _git(source, "add", "marker.txt")
        _git(source, "commit", "-q", "-m", "one")
        _git(source, "tag", "1.0.0")
        _git(source, "tag", "v9.9.9")

        (source / "marker.txt").write_text("2\n", encoding="utf-8")
        _git(source, "commit", "-q", "-am", "two")
        _git(source, "tag", "1.2.0")
        _git(source, "push", "-q", "origin", "HEAD:main", "--tags")

        subprocess.run(["git", "clone", "-q", str(origin), str(installed)], check=True)
        _git(installed, "checkout", "-q", "--detach", "1.0.0")
        _git(installed, "config", "user.name", "Refine Test")
        _git(installed, "config", "user.email", "refine@example.test")

        original_release_tags = upgrade._release_tags
        try:
            upgrade._release_tags = lambda _remote_url: ["1.0.0", "1.2.0", "v9.9.9"]
            assert upgrade.current_version(installed) == "1.0.0"
            assert upgrade.latest_version(installed) == "1.2.0"
            info = upgrade.status(installed)
            assert info.current_version == "1.0.0"
            assert info.latest_version == "1.2.0"
            assert info.upgrade_available is True
            assert info.local_development is False
            assert info.command.endswith("scripts/install.sh | bash -s -- --yes")
            assert "--yes" in info.command
            assert "--upgrade" not in info.command

            (installed / "marker.txt").write_text("local dev\n", encoding="utf-8")
            _git(installed, "commit", "-q", "-am", "local dev")
            dev_info = upgrade.status(installed)
            assert dev_info.current_version == "1.0.0"
            assert dev_info.latest_version == "1.2.0"
            assert dev_info.local_development is True
            assert dev_info.upgrade_available is False
        finally:
            upgrade._release_tags = original_release_tags
        print("[ok] upgrade status handles semver tags and local development commits")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

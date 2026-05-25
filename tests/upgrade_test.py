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

        assert upgrade.current_version(installed) == "1.0.0"
        assert upgrade.latest_version(installed) == "1.2.0"
        info = upgrade.status(installed)
        assert info.current_version == "1.0.0"
        assert info.latest_version == "1.2.0"
        assert info.upgrade_available is True
        assert "--upgrade" in info.command
        print("[ok] upgrade status compares semver tags and ignores v-prefixed tags")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

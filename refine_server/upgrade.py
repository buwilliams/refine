"""Release tag discovery for Refine upgrades."""
from __future__ import annotations

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path


SEMVER_RE = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")
INSTALL_COMMAND = (
    "curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/"
    "scripts/install.sh | bash -s -- --upgrade"
)


@dataclass(frozen=True)
class UpgradeInfo:
    current_version: str
    latest_version: str
    upgrade_available: bool
    command: str
    error: str = ""
    local_development: bool = False

    def as_dict(self) -> dict[str, object]:
        return {
            "current_version": self.current_version,
            "latest_version": self.latest_version,
            "upgrade_available": self.upgrade_available,
            "command": self.command,
            "error": self.error,
            "local_development": self.local_development,
        }


def status(repo: Path | None = None) -> UpgradeInfo:
    repo = (repo or Path.cwd()).resolve()
    try:
        _git(repo, "fetch", "--tags", "--quiet", "origin")
    except RuntimeError as e:
        try:
            current = current_version(repo)
        except RuntimeError:
            current = ""
        return UpgradeInfo(
            current_version=current,
            latest_version="",
            upgrade_available=False,
            command=INSTALL_COMMAND,
            error=str(e),
        )
    current = current_version(repo)
    latest = latest_version(repo)
    local_dev = local_development(repo, current)
    return UpgradeInfo(
        current_version=current,
        latest_version=latest,
        upgrade_available=bool(
            current
            and latest
            and not local_dev
            and _semver_tuple(latest) > _semver_tuple(current)
        ),
        command=INSTALL_COMMAND,
        local_development=local_dev,
    )


def current_version(repo: Path) -> str:
    tags = _git(repo, "tag", "--merged", "HEAD").splitlines()
    return _latest_semver(tags)


def latest_version(repo: Path) -> str:
    tags = _git(repo, "tag").splitlines()
    return _latest_semver(tags)


def local_development(repo: Path, version: str | None = None) -> bool:
    version = version if version is not None else current_version(repo)
    return bool(
        version
        and _git(repo, "rev-parse", "HEAD")
        != _git(repo, "rev-parse", f"{version}^{{commit}}")
    )


def _latest_semver(tags: list[str]) -> str:
    versions = [tag.strip() for tag in tags if SEMVER_RE.match(tag.strip())]
    if not versions:
        return ""
    versions.sort(key=_semver_tuple)
    return versions[-1]


def _semver_tuple(version: str) -> tuple[int, int, int]:
    match = SEMVER_RE.match(version)
    if not match:
        return (-1, -1, -1)
    return tuple(int(part) for part in match.groups())  # type: ignore[return-value]


def _git(repo: Path, *args: str) -> str:
    out = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        text=True,
        timeout=15,
    )
    if out.returncode != 0:
        raise RuntimeError((out.stderr or out.stdout or "git command failed").strip())
    return out.stdout.strip()

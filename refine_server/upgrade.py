"""Release tag discovery for Refine upgrades."""
from __future__ import annotations

import json
import os
import re
import subprocess
import urllib.error
import urllib.request
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
    error = ""
    try:
        latest = latest_version(repo)
    except RuntimeError as e:
        latest = ""
        error = str(e)
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
        error=error,
        local_development=local_dev,
    )


def current_version(repo: Path) -> str:
    tags = _git(repo, "tag", "--merged", "HEAD").splitlines()
    return _latest_semver(tags)


def latest_version(repo: Path) -> str:
    remote = _git(repo, "config", "--get", "remote.origin.url")
    return _latest_semver(_release_tags(remote))


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


def _release_tags(remote_url: str) -> list[str]:
    slug = _github_repo_slug(remote_url)
    if not slug:
        raise RuntimeError("Refine release checks require a GitHub origin remote")
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "refine-upgrade-check",
    }
    token = os.environ.get("GITHUB_TOKEN")
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(
        f"https://api.github.com/repos/{slug}/releases?per_page=100",
        headers=headers,
    )
    try:
        with urllib.request.urlopen(req, timeout=15) as response:
            releases = json.loads(response.read().decode("utf-8"))
    except (OSError, urllib.error.URLError, json.JSONDecodeError) as e:
        raise RuntimeError(f"Could not fetch Refine releases: {e}") from e
    if not isinstance(releases, list):
        raise RuntimeError("Could not fetch Refine releases: unexpected response")
    tags = []
    for release in releases:
        if not isinstance(release, dict):
            continue
        if release.get("draft") or release.get("prerelease"):
            continue
        tag = release.get("tag_name")
        if isinstance(tag, str):
            tags.append(tag)
    return tags


def _github_repo_slug(remote_url: str) -> str:
    remote_url = remote_url.strip()
    patterns = (
        r"^https://github\.com/([^/]+/[^/.]+)(?:\.git)?/?$",
        r"^git@github\.com:([^/]+/[^/.]+)(?:\.git)?$",
        r"^ssh://git@github\.com/([^/]+/[^/.]+)(?:\.git)?$",
    )
    for pattern in patterns:
        match = re.match(pattern, remote_url)
        if match:
            return match.group(1)
    return ""


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

"""Configuration loaded from `refine.toml` at the volume root.

Discovery order:
1. Explicit path passed via `--config` (or `Config.load(path=...)`).
2. Walking up from cwd: each ancestor's `refine.toml` or `refine/refine.toml`.

The volume root is the directory containing `refine.toml`. Paths in the file
are resolved relative to that directory (unless absolute).

Schema (TOML):

    client_repo  = ".."
    runner_socket = "./run/runner.sock"

    [web]
    host = "0.0.0.0"
    port = 8080
"""
from __future__ import annotations

import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:
    tomllib = None  # type: ignore[assignment]


CONFIG_FILENAME = "refine.toml"


class ConfigError(Exception):
    """Raised when no config can be found or it is malformed."""


@dataclass(frozen=True)
class Config:
    config_path: Path        # absolute path to refine.toml
    volume_root: Path        # absolute path; directory containing refine.toml
    client_repo: Path        # absolute path
    runner_socket: Path      # absolute path
    web_host: str
    web_port: int

    @property
    def sqlite_path(self) -> Path:
        return self.volume_root / "index.sqlite"

    @property
    def gaps_dir(self) -> Path:
        return self.volume_root / "gaps"

    @classmethod
    def load(cls, path: Path | str | None = None) -> "Config":
        cp = Path(path) if path else find_config()
        if cp is None or not cp.is_file():
            raise ConfigError(
                f"No {CONFIG_FILENAME} found. Run `refine init` in the client repo, "
                "or pass --config /path/to/refine.toml."
            )
        cp = cp.resolve()
        text = cp.read_text(encoding="utf-8")
        try:
            raw = _parse_toml(text)
        except ValueError as e:
            raise ConfigError(f"Could not parse {cp}: {e}") from e
        return cls._from_raw(cp, raw)

    @classmethod
    def _from_raw(cls, path: Path, raw: dict) -> "Config":
        volume_root = path.parent
        client_repo_rel = raw.get("client_repo", "..")
        socket_rel = raw.get("runner_socket", "./run/runner.sock")
        web = raw.get("web") or {}
        web_host = str(web.get("host", "0.0.0.0"))
        web_port = int(web.get("port", 8080))
        return cls(
            config_path=path,
            volume_root=volume_root,
            client_repo=_resolve(volume_root, client_repo_rel),
            runner_socket=_resolve(volume_root, socket_rel),
            web_host=web_host,
            web_port=web_port,
        )


def _resolve(base: Path, p: str) -> Path:
    pp = Path(p)
    return pp if pp.is_absolute() else (base / pp).resolve()


def find_config(start: Path | None = None) -> Path | None:
    """Walk up from `start` (cwd) looking for refine.toml or refine/refine.toml.

    Also checks fixed fallback locations used by the Docker container.
    """
    start = (start or Path.cwd()).resolve()
    candidates = []
    seen = set()
    for d in [start, *start.parents]:
        for c in (d / CONFIG_FILENAME, d / "refine" / CONFIG_FILENAME):
            key = c.resolve(strict=False)
            if key in seen:
                continue
            seen.add(key)
            candidates.append(c)
    # Docker-conventional locations (kept stable across versions; not user-facing).
    for fixed in (Path("/refine-data") / CONFIG_FILENAME,):
        if fixed not in seen:
            candidates.append(fixed)
    for c in candidates:
        if c.is_file():
            return c
    return None


DEFAULT_TOML = """# refine — per-project configuration. Commit this file alongside the
# client repo's source. See README + spec.md for the conceptual model.

# Path to the client repo, relative to this file (the volume root sits
# inside the client repo).
client_repo = ".."

# Unix domain socket where the host runner listens. Both the host runner and
# the Docker webapp container access this same file via the volume-root
# bind mount, so the path must live under the volume root.
runner_socket = "./run/runner.sock"

[web]
host = "0.0.0.0"
port = 8080
"""


def write_defaults(volume_root: Path, *, force: bool = False) -> Path:
    """Write a default refine.toml to `volume_root` (creating the directory).

    Also creates the runner socket directory. Adds a `.gitignore` next to the
    config that excludes the SQLite + socket files (the gap JSON files and
    refine.toml itself stay committed).

    Returns the absolute path to the new config file.
    """
    volume_root = volume_root.resolve()
    volume_root.mkdir(parents=True, exist_ok=True)
    cfg = volume_root / CONFIG_FILENAME
    if cfg.exists() and not force:
        raise ConfigError(f"{cfg} already exists. Use --force to overwrite.")
    cfg.write_text(DEFAULT_TOML, encoding="utf-8")
    (volume_root / "run").mkdir(exist_ok=True)
    (volume_root / "gaps").mkdir(exist_ok=True)

    # Local .gitignore — excludes derived state but keeps gap.json + the config committed.
    gi = volume_root / ".gitignore"
    if not gi.exists():
        gi.write_text(
            "# refine state — derived from gap.json files; not worth committing.\n"
            "index.sqlite\n"
            "index.sqlite-wal\n"
            "index.sqlite-shm\n"
            "run/\n",
            encoding="utf-8",
        )
    return cfg


# ---- Optional helper used by code paths that want a Config they can rely on
# without thinking about discovery. Caches the result.

_cached: Config | None = None


def get(*, path: Path | str | None = None, reload: bool = False) -> Config:
    global _cached
    if _cached is None or reload or path is not None:
        _cached = Config.load(path)
    return _cached


def _parse_toml(text: str) -> dict:
    """Parse our (very small) refine.toml schema.

    Uses tomllib when available (Python 3.11+); falls back to a minimal hand
    parser handling: `key = "string"`, `key = number`, `key = true|false`,
    `[section]` headers, `#` line comments, and blank lines. No arrays, no
    multi-line strings, no nested tables — refine.toml never needs them.
    """
    if tomllib is not None:
        return tomllib.loads(text)
    out: dict = {}
    section: dict = out
    for lineno, raw in enumerate(text.splitlines(), 1):
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            name = line[1:-1].strip()
            if not name:
                raise ValueError(f"line {lineno}: empty section header")
            section = out.setdefault(name, {})
            continue
        if "=" not in line:
            raise ValueError(f"line {lineno}: expected `key = value`, got {raw!r}")
        key, _, value = line.partition("=")
        key = key.strip()
        value = value.strip()
        if (value.startswith('"') and value.endswith('"')) or (
            value.startswith("'") and value.endswith("'")
        ):
            section[key] = value[1:-1]
        elif value in ("true", "false"):
            section[key] = (value == "true")
        else:
            try:
                section[key] = int(value)
            except ValueError:
                try:
                    section[key] = float(value)
                except ValueError:
                    raise ValueError(
                        f"line {lineno}: cannot parse value {value!r}"
                    ) from None
    return out

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
BINDING_FILENAME = ".refine-binding"

# Marker line in the binding file recording the systemd --user unit name
# associated with this refine clone, so `refine start/stop/status` don't
# need to re-derive it (and so a future rename of the clone dir doesn't
# silently produce a second, orphan unit).
_BINDING_UNIT_MARKER = "# unit:"


def unit_name_for(clone_dir: Path) -> str:
    """Return the systemd --user unit name for a refine clone.

    Derived from the clone directory's basename, lowercased, with anything
    outside [a-z0-9._-] collapsed to '-'. The prefix `refine-` is added
    if the basename does not already start with it, so
    `systemctl --user list-units 'refine-*'` lists every refine instance.
    """
    import re
    base = clone_dir.resolve().name.lower()
    base = re.sub(r"[^a-z0-9._-]+", "-", base).strip("-") or "instance"
    if not base.startswith("refine-"):
        base = f"refine-{base}"
    return base


def read_binding_unit(binding_path: Path) -> str | None:
    """Return the unit name recorded in a binding file, if any."""
    try:
        text = binding_path.read_text(encoding="utf-8")
    except OSError:
        return None
    for line in text.splitlines():
        s = line.strip()
        if s.startswith(_BINDING_UNIT_MARKER):
            name = s[len(_BINDING_UNIT_MARKER):].strip()
            if name:
                return name
    return None


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
    """Discover refine.toml. Tries, in order:

    1. A `.refine-binding` file in cwd or any ancestor — its target client
       repo's `.refine/refine.toml`. This is the "run from /opt/refine
       targeting /srv/clients/<x>" workflow.
    2. Walking up from cwd looking for `refine.toml` or `.refine/refine.toml`.
       This is the "run from inside the client repo" workflow.
    3. The Docker-conventional `/refine-data/refine.toml`.
    """
    start = (start or Path.cwd()).resolve()

    # 1. Binding file
    binding = find_binding(start)
    if binding is not None:
        try:
            client_repo = read_binding(binding)
            cfg = client_repo / ".refine" / CONFIG_FILENAME
            if cfg.is_file():
                return cfg
        except (ConfigError, OSError):
            pass

    # 2. Walk up
    candidates: list[Path] = []
    seen: set[Path] = set()
    for d in [start, *start.parents]:
        for c in (d / CONFIG_FILENAME, d / ".refine" / CONFIG_FILENAME):
            key = c.resolve(strict=False)
            if key in seen:
                continue
            seen.add(key)
            candidates.append(c)

    # 3. Docker-conventional fallback
    fixed = Path("/refine-data") / CONFIG_FILENAME
    if fixed.resolve(strict=False) not in seen:
        candidates.append(fixed)

    for c in candidates:
        if c.is_file():
            return c
    return None


# ---- Binding ----------------------------------------------------------------

def find_binding(start: Path | None = None) -> Path | None:
    """Look for `.refine-binding` in cwd (or `start`) and its ancestors."""
    start = (start or Path.cwd()).resolve()
    for d in [start, *start.parents]:
        b = d / BINDING_FILENAME
        if b.is_file():
            return b
    return None


def read_binding(binding_path: Path) -> Path:
    """Read `.refine-binding` and return the bound client repo path.

    File format: first non-empty, non-comment line is the path (absolute or
    relative to the binding file's directory).
    """
    text = binding_path.read_text(encoding="utf-8")
    for line in text.splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        p = Path(s).expanduser()
        if not p.is_absolute():
            p = (binding_path.parent / p).resolve()
        return p
    raise ConfigError(f"{binding_path} contains no client-repo path")


def write_binding(refine_source_dir: Path, client_repo: Path) -> Path:
    """Write `.refine-binding` in the refine source dir pointing at a client.

    Also records the systemd --user unit name so `refine start/stop/status`
    can look it up without re-deriving from the directory basename (which
    might drift if the clone is later renamed).

    Returns the absolute path to the written binding file.
    """
    refine_source_dir = refine_source_dir.resolve()
    binding = refine_source_dir / BINDING_FILENAME
    unit = unit_name_for(refine_source_dir)
    binding.write_text(
        f"# refine binding — this refine clone targets the client repo below.\n"
        f"# Created by `refine init`. Re-run `refine init <other_path>` to rebind.\n"
        f"{_BINDING_UNIT_MARKER} {unit}\n"
        f"{client_repo.resolve()}\n",
        encoding="utf-8",
    )
    return binding


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

"""Configuration loaded from `refine.toml` at the volume root.

Discovery order:
1. Explicit path passed via `--config` (or `Config.load(path=...)`).
2. Walking up from cwd: each ancestor's `refine.toml` or `refine/refine.toml`.

The volume root is the directory containing `refine.toml`. Paths in the file
are resolved relative to that directory (unless absolute).

Schema (TOML):

    client_repo  = ".."

    [web]
    host = "0.0.0.0"
    port = 8080
"""
from __future__ import annotations

import hashlib
import os
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:
    tomllib = None  # type: ignore[assignment]


CONFIG_FILENAME = "refine.toml"
BINDING_FILENAME = ".refine-binding"
ENV_CONFIG_PATH = "REFINE_CONFIG_PATH"
ENV_UI_SCOPE = "REFINE_UI_SCOPE"
ENV_UI_PORT = "REFINE_UI_PORT"

# Marker line in the binding file recording the systemd service base name
# associated with this refine checkout, so `refine start/stop/status` don't
# need to re-derive it (and so a future rename of the checkout dir doesn't
# silently produce a second, orphan unit).
_BINDING_UNIT_MARKER = "# unit:"


def unit_name_for(clone_dir: Path) -> str:
    """Return the systemd unit name for a refine checkout.

    Derived from the checkout directory's basename, lowercased, with anything
    outside [a-z0-9._-] collapsed to '-'. The prefix `refine-` is added
    if the basename does not already start with it, so
    `systemctl list-units 'refine*'` lists every refine node.
    """
    import re
    base = clone_dir.resolve().name.lower()
    base = re.sub(r"[^a-z0-9._-]+", "-", base).strip("-") or "node"
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
    web_host: str
    web_port: int

    @property
    def sqlite_path(self) -> Path:
        return sqlite_path_for(self.volume_root)

    @property
    def gaps_dir(self) -> Path:
        return self.volume_root / "gaps"

    @classmethod
    def load(cls, path: Path | str | None = None) -> "Config":
        cp = Path(path) if path else find_config()
        if cp is None or not cp.is_file():
            raise ConfigError(
                f"No {CONFIG_FILENAME} found. Run `refine init <app-path>` "
                "from the refine checkout, or pass --config /path/to/refine.toml."
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
        web = raw.get("web") or {}
        web_host = str(web.get("host", "0.0.0.0"))
        web_port = int(web.get("port", 8080))
        return cls(
            config_path=path,
            volume_root=volume_root,
            client_repo=_resolve(volume_root, client_repo_rel),
            web_host=web_host,
            web_port=web_port,
        )


def _resolve(base: Path, p: str) -> Path:
    pp = Path(p)
    return pp if pp.is_absolute() else (base / pp).resolve()


def find_config(start: Path | None = None) -> Path | None:
    """Discover refine.toml. Tries, in order:

    1. A `.refine-binding` file in cwd or any ancestor — its target app's
       `.refine/refine.toml`. This is the "run from /opt/refine targeting
       /srv/clients/<x>" workflow.
    2. Walking up from cwd looking for `refine.toml` or `.refine/refine.toml`.
       This is the "run from inside the target app repo" workflow.
    """
    start = (start or Path.cwd()).resolve()

    env_path = os.environ.get(ENV_CONFIG_PATH)
    if env_path:
        cfg = Path(env_path).expanduser()
        if not cfg.is_absolute():
            cfg = (start / cfg).resolve()
        if cfg.is_file():
            return cfg

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

    for c in candidates:
        if c.is_file():
            return c
    return None


# ---- Binding ----------------------------------------------------------------

def find_binding(start: Path | None = None) -> Path | None:
    """Look for `.refine-binding` in cwd (or `start`) and its ancestors."""
    if start is None:
        try:
            start = Path.cwd()
        except FileNotFoundError:
            return None
    start = start.resolve()
    for d in [start, *start.parents]:
        b = d / BINDING_FILENAME
        if b.is_file():
            return b
    return None


def local_run_dir(start: Path | None = None) -> Path:
    """Checkout-local runtime directory for process/session state.

    Bound Refine checkouts keep local state next to `.refine-binding`, so it
    survives target-app switching without writing host state into the target
    app's `.refine/` directory. If no binding is in scope, fall back to cwd/run
    for setup/debug flows that do not have a target app attached yet.
    """
    binding = find_binding(start)
    if binding is not None:
        return binding.parent.resolve() / "run"
    if start is not None:
        return start.resolve() / "run"
    cached = globals().get("_cached")
    if cached is not None:
        return cached.client_repo / "run"
    try:
        return Path.cwd().resolve() / "run"
    except FileNotFoundError:
        return Path(tempfile.gettempdir()) / "refine-run"


def runtime_scope() -> str:
    """Stable local scope for one UI backend process.

    Detached UI backends set this from the port they serve. Without a scope,
    CLI/test/foreground paths keep the historical shared cache behavior.
    """
    raw = os.environ.get(ENV_UI_SCOPE) or os.environ.get(ENV_UI_PORT) or ""
    return "".join(ch if ch.isalnum() or ch in "._-" else "-" for ch in raw.strip())


def sqlite_path_for(volume_root: Path) -> Path:
    scope = runtime_scope()
    if not scope:
        return volume_root / "index.sqlite"
    digest = hashlib.sha1(str(volume_root.resolve()).encode("utf-8")).hexdigest()[:12]
    cache_dir = local_run_dir() / "cache"
    return cache_dir / f"index-{scope}-{digest}.sqlite"


def read_binding(binding_path: Path) -> Path:
    """Read `.refine-binding` and return the active target app path.

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
    raise ConfigError(f"{binding_path} contains no target app path")


def write_binding(refine_source_dir: Path, client_repo: Path) -> Path:
    """Write `.refine-binding` in the refine source dir pointing at the active app.

    Also records the systemd service base name so `refine start/stop/status`
    can look it up without re-deriving from the directory basename (which
    might drift if the checkout is later renamed).

    Returns the absolute path to the written binding file.
    """
    refine_source_dir = refine_source_dir.resolve()
    binding = refine_source_dir / BINDING_FILENAME
    unit = unit_name_for(refine_source_dir)
    binding.write_text(
        f"# refine binding — this checkout's active target app.\n"
        f"# Created by `refine init`. Use Settings > Project or `refine init <path> --force` to switch.\n"
        f"{_BINDING_UNIT_MARKER} {unit}\n"
        f"{client_repo.resolve()}\n",
        encoding="utf-8",
    )
    return binding

DEFAULT_TOML = """# refine — per-project configuration. Commit this file alongside the
# target app's source. See README + docs/spec.md for the conceptual model.

# Path to the target app repo, relative to this file (the volume root sits
# inside the target app repo).
client_repo = ".."

[web]
host = "0.0.0.0"
port = 8080
"""

REFINE_GITIGNORE_LINES = [
    "# refine runtime state - local, derived, or high-churn.",
    "index.sqlite",
    "index.sqlite-shm",
    "index.sqlite-wal",
    "app.log",
    "app.pid",
    "logs/",
    "regressions/runs/",
    "gaps/**/logs.jsonl",
]


def ensure_refine_gitignore(volume_root: Path) -> Path:
    """Ensure generated Refine files under .refine are ignored."""
    volume_root = volume_root.resolve()
    volume_root.mkdir(parents=True, exist_ok=True)
    path = volume_root / ".gitignore"
    existing = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    next_lines = list(existing)
    for line in REFINE_GITIGNORE_LINES:
        if line not in next_lines:
            next_lines.append(line)
    if next_lines != existing or not path.exists():
        path.write_text("\n".join(next_lines).rstrip() + "\n", encoding="utf-8")
    return path


def ensure_runtime_gitignore(client_repo: Path) -> Path:
    """Ensure checkout-local runtime sockets/logs are not stashed or committed."""
    client_repo = client_repo.resolve()
    git_dir = client_repo / ".git"
    path = (
        git_dir / "info" / "exclude"
        if git_dir.is_dir()
        else client_repo / ".gitignore"
    )
    existing = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    next_lines = list(existing)
    if "/run/" not in next_lines:
        next_lines.append("/run/")
    if next_lines != existing or not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(next_lines).rstrip() + "\n", encoding="utf-8")
    return path


def write_defaults(volume_root: Path, *, force: bool = False) -> Path:
    """Write a default refine.toml to `volume_root` (creating the directory).

    Also creates the gaps directory. Adds a `.gitignore` next to the config
    that excludes SQLite state (the gap JSON files and refine.toml itself stay
    committed).

    Returns the absolute path to the new config file.
    """
    volume_root = volume_root.resolve()
    volume_root.mkdir(parents=True, exist_ok=True)
    cfg = volume_root / CONFIG_FILENAME
    if cfg.exists() and not force:
        raise ConfigError(f"{cfg} already exists. Use --force to overwrite.")
    cfg.write_text(DEFAULT_TOML, encoding="utf-8")
    (volume_root / "gaps").mkdir(exist_ok=True)

    ensure_refine_gitignore(volume_root)
    ensure_runtime_gitignore(volume_root.parent)
    return cfg


# ---- Optional helper used by code paths that want a Config they can rely on
# without thinking about discovery. Caches the result.

_cached: Config | None = None


def get(*, path: Path | str | None = None, reload: bool = False) -> Config:
    global _cached
    if _cached is None or reload or path is not None:
        _cached = Config.load(path)
    return _cached


def clear_cache() -> None:
    """Forget the process-local config cache."""
    global _cached
    _cached = None


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

"""Configuration loaded from `refine.toml` at the volume root.

Discovery order:
1. Explicit path passed via `--config` (or `Config.load(path=...)`).
2. Port-local app binding in the Refine checkout's `run/<port>/apps.json`.
3. Direct target-app cwd fallback for developer/test commands already inside a
   target repo.

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
import json
import os
import sys
import tempfile
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

try:
    import tomllib  # Python 3.11+
except ModuleNotFoundError:
    tomllib = None  # type: ignore[assignment]


CONFIG_FILENAME = "refine.toml"
BINDING_FILENAME = ".refine-binding"
ENV_CONFIG_PATH = "REFINE_CONFIG_PATH"
ENV_LOCAL_NODE_ID = "REFINE_LOCAL_NODE_ID"
ENV_RUN_DIR = "REFINE_RUN_DIR"
ENV_TEST_RUN_ROOT = "REFINE_TEST_RUN_ROOT"
ENV_UI_SCOPE = "REFINE_UI_SCOPE"
ENV_UI_PORT = "REFINE_UI_PORT"
DEFAULT_UI_PORT = 8080
DOTENV_FILENAME = ".env"
PRIMARY_FILENAME = "primary.json"

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


def load_dotenv(
    start: Path | str | None = None,
    *,
    override: bool = False,
) -> dict[str, str]:
    """Load checkout-local `.env` values into `os.environ`.

    Refine intentionally keeps this small and dependency-free. The loader
    accepts ordinary `KEY=value` lines, optional `export KEY=value`, comments,
    and quoted values. Existing exported environment variables win unless
    `override=True` is passed.
    """
    path = find_dotenv(Path(start) if start is not None else None)
    if path is None:
        return {}
    loaded: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return {}
    for line in lines:
        parsed = _parse_dotenv_line(line)
        if parsed is None:
            continue
        key, value = parsed
        if not _valid_env_name(key):
            continue
        if override or key not in os.environ:
            os.environ[key] = value
            loaded[key] = value
    return loaded


def find_dotenv(start: Path | None = None) -> Path | None:
    """Return the `.env` file for the Refine checkout in scope, if any."""
    if start is None:
        try:
            start = Path.cwd()
        except FileNotFoundError:
            return None
    start = start.resolve()
    if start.is_file():
        start = start.parent

    binding = find_binding(start)
    if binding is not None:
        candidate = binding.parent / DOTENV_FILENAME
        return candidate if candidate.is_file() else None

    for d in [start, *start.parents]:
        if _looks_like_refine_checkout(d):
            candidate = d / DOTENV_FILENAME
            return candidate if candidate.is_file() else None
    candidate = start / DOTENV_FILENAME
    return candidate if candidate.is_file() else None


def _looks_like_refine_checkout(path: Path) -> bool:
    return (
        ((path / "python" / "pyproject.toml").is_file()
         and (path / "python" / "refine_cli").is_dir())
        or ((path / "pyproject.toml").is_file() and (path / "refine_cli").is_dir())
    )


def _has_refine_checkout_ancestor(path: Path) -> bool:
    return any(_looks_like_refine_checkout(d) for d in [path, *path.parents])


def _refine_source_checkout() -> Path | None:
    python_root = Path(__file__).resolve().parents[1]
    repo_root = python_root.parent
    if _looks_like_refine_checkout(repo_root):
        return repo_root
    return python_root if _looks_like_refine_checkout(python_root) else None


def _is_source_path(path: Path, source: Path | None) -> bool:
    if source is None:
        return False
    resolved = path.resolve()
    source = source.resolve()
    return resolved == source or resolved == source / "python"


def _parse_dotenv_line(line: str) -> tuple[str, str] | None:
    text = line.strip()
    if not text or text.startswith("#"):
        return None
    if text.startswith("export "):
        text = text[len("export "):].lstrip()
    if "=" not in text:
        return None
    key, raw_value = text.split("=", 1)
    key = key.strip()
    value = _strip_dotenv_value(raw_value.strip())
    return key, value


def _strip_dotenv_value(value: str) -> str:
    if not value:
        return ""
    if value[0] in {"'", '"'}:
        quote = value[0]
        out: list[str] = []
        escaped = False
        for ch in value[1:]:
            if escaped:
                out.append(_dotenv_escape(ch) if quote == '"' else ch)
                escaped = False
                continue
            if ch == "\\" and quote == '"':
                escaped = True
                continue
            if ch == quote:
                return "".join(out)
            out.append(ch)
        return "".join(out)
    hash_at = value.find("#")
    if hash_at > 0 and value[hash_at - 1].isspace():
        value = value[:hash_at].rstrip()
    return value


def _dotenv_escape(ch: str) -> str:
    return {"n": "\n", "r": "\r", "t": "\t", "\\": "\\", '"': '"'}.get(ch, ch)


def _valid_env_name(name: str) -> bool:
    if not name:
        return False
    first = name[0]
    if not (first.isalpha() or first == "_"):
        return False
    return all(ch.isalnum() or ch == "_" for ch in name)


@dataclass(frozen=True)
class Config:
    config_path: Path        # absolute path to refine.toml
    volume_root: Path        # absolute path; directory containing refine.toml
    client_repo: Path        # absolute path
    web_host: str
    web_port: int

    @property
    def sqlite_path(self) -> Path:
        return sqlite_path_for(self.volume_root, port=self.web_port)

    @property
    def gaps_dir(self) -> Path:
        return self.volume_root / "gaps"

    @classmethod
    def load(cls, path: Path | str | None = None, *, port: int | str | None = None) -> "Config":
        allow_walk = (
            path is None
            and port is None
            and not os.environ.get(ENV_UI_SCOPE)
            and not os.environ.get(ENV_UI_PORT)
        )
        cp = Path(path) if path else find_config(port=port, allow_walk=allow_walk)
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
        return cls._from_raw(cp, raw, port=port)

    @classmethod
    def _from_raw(cls, path: Path, raw: dict, *, port: int | str | None = None) -> "Config":
        volume_root = path.parent
        client_repo_rel = raw.get("client_repo", "..")
        web = raw.get("web") or {}
        web_host = str(web.get("host", "0.0.0.0"))
        web_port = _config_runtime_port(port, int(web.get("port", DEFAULT_UI_PORT)))
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


def find_config(
    start: Path | None = None,
    *,
    port: int | str | None = None,
    allow_walk: bool = False,
) -> Path | None:
    """Discover refine.toml for the current port.

    Runtime discovery is intentionally port-scoped: the selected app lives in
    `run/<port>/apps.json`. Explicit paths via REFINE_CONFIG_PATH still win for
    child-process handoff and tests.
    """
    start = (start or Path.cwd()).resolve()

    env_path = os.environ.get(ENV_CONFIG_PATH)
    if env_path:
        cfg = Path(env_path).expanduser()
        if not cfg.is_absolute():
            cfg = (start / cfg).resolve()
        if cfg.is_file():
            return cfg

    try:
        from refine_server import project_registry

        client_repo = project_registry.active_app(start, port=port)
        if client_repo is not None:
            cfg = client_repo / ".refine" / CONFIG_FILENAME
            if cfg.is_file():
                return cfg
    except Exception:
        pass

    if not allow_walk and _has_refine_checkout_ancestor(start):
        return None

    for c in _walk_config_candidates(start):
        if c.is_file():
            return c
    return None


def _walk_config_candidates(start: Path) -> list[Path]:
    candidates: list[Path] = []
    seen: set[Path] = set()
    for d in [start, *start.parents]:
        for c in (d / CONFIG_FILENAME, d / ".refine" / CONFIG_FILENAME):
            key = c.resolve(strict=False)
            if key in seen:
                continue
            seen.add(key)
            candidates.append(c)
    return candidates


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


def local_run_root(start: Path | None = None) -> Path:
    """Checkout-local runtime root for host state."""
    raw_test_root = os.environ.get(ENV_TEST_RUN_ROOT)
    test_root = Path(raw_test_root).expanduser().resolve() if raw_test_root else None
    source = _refine_source_checkout()
    if raw_test_root:
        if start is not None:
            try:
                resolved_start = start.resolve()
            except OSError:
                resolved_start = start
            if _is_source_path(resolved_start, source):
                return test_root
    if start is not None:
        return start.resolve() / "run"
    binding = find_binding()
    if binding is not None:
        return binding.parent.resolve() / "run"
    try:
        cwd = Path.cwd().resolve()
    except FileNotFoundError:
        cwd = None
    if cwd is not None:
        registry = cwd / "run" / str(runtime_port()) / "apps.json"
        if registry.is_file():
            return cwd / "run"
        for d in [cwd, *cwd.parents]:
            if _looks_like_refine_checkout(d):
                if test_root is not None and _is_source_path(d, source):
                    return test_root
                return d / "run"
    if test_root is not None:
        return test_root
    if source is not None:
        return source / "run"
    return Path(tempfile.gettempdir()) / "refine-run"


def primary_path(start: Path | None = None) -> Path:
    """Checkout-local primary Refine instance metadata path."""
    return local_run_root(start) / PRIMARY_FILENAME


def _read_primary_payload(start: Path | None = None) -> dict[str, object]:
    try:
        raw = json.loads(primary_path(start).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    return raw if isinstance(raw, dict) else {}


def _write_primary_payload(
    start: Path | None,
    payload: dict[str, object],
    *,
    source: str,
) -> Path:
    path = primary_path(start)
    path.parent.mkdir(parents=True, exist_ok=True)
    payload["version"] = 1
    payload["source"] = str(source or "manual")
    payload["updated_at"] = datetime.now(timezone.utc).isoformat()
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    tmp.replace(path)
    return path


def primary_port(start: Path | None = None) -> int | None:
    """Return the checkout's primary port if it is recorded and valid."""
    raw = _read_primary_payload(start)
    try:
        port = int(raw.get("port"))
    except (TypeError, ValueError):
        return None
    return port if 0 < port <= 65535 else None


def primary_active_node(start: Path | None = None) -> str | None:
    """Return the checkout's most recently activated node id, if recorded."""
    raw = _read_primary_payload(start)
    node_id = str(raw.get("active_node_id") or "").strip()
    return node_id or None


def write_primary_port(
    start: Path | None,
    port: int | str,
    *,
    source: str = "manual",
) -> Path:
    """Persist the checkout's primary Refine port under run/primary.json."""
    selected = int(port)
    if selected <= 0 or selected > 65535:
        raise ValueError(f"invalid port: {port!r}")
    payload = _read_primary_payload(start)
    payload["port"] = selected
    return _write_primary_payload(start, payload, source=source)


def write_primary_active_node(
    start: Path | None,
    node_id: str,
    *,
    source: str = "node-activate",
) -> Path:
    """Persist the checkout's most recently activated node under run/primary.json."""
    selected = str(node_id or "").strip()
    if not selected:
        raise ValueError("node_id is required")
    payload = _read_primary_payload(start)
    payload["active_node_id"] = selected
    return _write_primary_payload(start, payload, source=source)


def clear_primary_port(start: Path | None = None, *, port: int | str | None = None) -> bool:
    """Remove run/primary.json, optionally only when it matches `port`."""
    path = primary_path(start)
    if port is not None:
        current = primary_port(start)
        try:
            selected = int(port)
        except (TypeError, ValueError):
            return False
        if current != selected:
            return False
    try:
        path.unlink()
        return True
    except FileNotFoundError:
        return False
    except OSError:
        return False


def runtime_port(default: int = DEFAULT_UI_PORT) -> int:
    raw = os.environ.get(ENV_UI_SCOPE) or os.environ.get(ENV_UI_PORT) or ""
    try:
        port = int(str(raw).strip()) if str(raw).strip() else int(default)
    except (TypeError, ValueError):
        port = int(default)
    if port <= 0 or port > 65535:
        return int(default)
    return port


def _config_runtime_port(port: int | str | None, default: int = DEFAULT_UI_PORT) -> int:
    if port is not None:
        raw = port
    else:
        raw = os.environ.get(ENV_UI_SCOPE) or os.environ.get(ENV_UI_PORT) or ""
    try:
        selected = int(str(raw).strip()) if str(raw).strip() else int(default)
    except (TypeError, ValueError):
        selected = int(default)
    if selected <= 0 or selected > 65535:
        return int(default)
    return selected


def local_run_dir(start: Path | None = None, *, port: int | str | None = None) -> Path:
    """Port-scoped checkout-local runtime directory."""
    if start is None and port is None:
        raw_run_dir = os.environ.get(ENV_RUN_DIR)
        if raw_run_dir:
            return Path(raw_run_dir).expanduser().resolve()
    raw_port = runtime_port() if port is None else int(port)
    return local_run_root(start) / str(raw_port)


def runtime_scope() -> str:
    """Stable local scope for one Refine supervisor process."""
    return str(runtime_port())


def sqlite_path_for(volume_root: Path, *, port: int | str | None = None) -> Path:
    digest = hashlib.sha1(str(volume_root.resolve()).encode("utf-8")).hexdigest()[:12]
    cache_dir = local_run_dir(port=port) / "cache"
    return cache_dir / f"index-{digest}.sqlite"


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
    """Compatibility helper: write this port's active app state."""
    refine_source_dir = refine_source_dir.resolve()
    from refine_server import project_registry

    project_registry.set_active_app(refine_source_dir, client_repo)
    return project_registry.registry_path(refine_source_dir)

DEFAULT_TOML = """# refine — per-project configuration. Commit this file alongside the
# target app's source. See README + docs/spec.md for the conceptual model.

# Path to the target app repo, relative to this file (the volume root sits
# inside the target app repo).
client_repo = ".."

[web]
host = "0.0.0.0"
port = 8080
"""


def default_toml(*, port: int | str | None = None) -> str:
    selected = _config_runtime_port(port, DEFAULT_UI_PORT)
    return DEFAULT_TOML.replace("port = 8080", f"port = {selected}", 1)

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


def write_defaults(
    volume_root: Path,
    *,
    force: bool = False,
    port: int | str | None = None,
) -> Path:
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
    cfg.write_text(default_toml(port=port), encoding="utf-8")
    (volume_root / "gaps").mkdir(exist_ok=True)

    ensure_refine_gitignore(volume_root)
    return cfg


# ---- Optional helper used by code paths that want a Config they can rely on
# without thinking about discovery. Caches the result.

_cached: Config | None = None


def get(
    *,
    path: Path | str | None = None,
    reload: bool = False,
    port: int | str | None = None,
) -> Config:
    global _cached
    if _cached is None or reload or path is not None or port is not None:
        _cached = Config.load(path, port=port)
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

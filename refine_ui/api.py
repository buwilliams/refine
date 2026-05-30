"""JSON API endpoint handlers.

Returns (status_code, body_dict) tuples. The server module wraps these.
"""
from __future__ import annotations

import base64
import csv
import difflib
import fnmatch
import io
import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
from collections import Counter
from datetime import datetime, timedelta, timezone
from functools import wraps
from pathlib import Path
from typing import Any, Callable
from urllib.parse import urlparse

from refine_server import activity, config, db, gap_writer, gaps as shared_gaps, governance, project_registry, project_state, quality, regressions, reporters, round_logs, search_index, upgrade
from refine_server import perf_metrics
from refine_server.gaps import now_iso
from refine_server.backend_protocol import (
    M_APPEND_ROUND, M_BACKGROUND_PROCESSES_SET, M_CANCEL, M_CANCEL_ALL, M_CHAT_INPUT, M_CHAT_READ, M_CHAT_START,
    M_CHAT_RESET_ALL, M_CHAT_STOP, M_CREATE_GAP, M_DELETE_GAP, M_DIAGNOSTICS, M_EDIT_ROUND,
    M_BULK_DELETE_GAPS, M_BULK_UPDATE_GAPS, M_ENFORCE_SCHEDULING, M_EXTRACT_GAPS, M_LAUNCH, M_LIST_CHANGES, M_LOG_APPEND, M_PREFLIGHT,
    M_GOVERNANCE_GENERATE_RULES, M_GOVERNANCE_WAKE, M_HARD_RESET_WORKTREE, M_PROJECT_SYNC, M_REGRESSION_RUN,
    M_MERGE_REPORTER, M_RENAME_REPORTER, M_RETRY_MERGE, M_RETRY_QA, M_SET_NOTES, M_TARGET_APP_GENERATE,
    M_TARGET_APP_HEALTH, M_TARGET_APP_REBUILD_PENDING, M_TARGET_APP_REBUILD_QUEUE, M_TARGET_APP_RUN, M_UNDO_GAP, M_VERIFY,
)
from refine_server.ulid import new_ulid
from refine_runtime import resources as runtime_resources

from .backend_client import BackendError, get_client
from . import background_jobs, runtime


# --- error helpers ------------------------------------------------------------

def err(
    code: int,
    message: str,
    details: str | None = None,
    *,
    error_code: str | None = None,
) -> tuple[int, dict]:
    body: dict[str, Any] = {"error": {"message": message}}
    if details is not None:
        body["error"]["details"] = details
    if error_code is not None:
        body["error"]["code"] = error_code
    return code, body


IMPORT_BACKGROUND_THRESHOLD = 100
BULK_UPDATE_BACKGROUND_THRESHOLD = 100
FILE_PREVIEW_MAX_BYTES = 1_000_000
FILE_TEXT_CHUNK_BYTES = 128_000
IMAGE_PREVIEW_MAX_BYTES = 5_000_000
FILES_TREE_MAX_DEPTH = 3
FILES_TREE_MAX_ENTRIES = 200
FILES_SEARCH_MAX_SCAN = 20_000
FILE_BROWSER_IGNORE_DEFAULT = "node_modules, .git, .refine"
IMAGE_MIME_BY_EXT = {
    ".gif": "image/gif",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".webp": "image/webp",
}


def _conn(*, ensure_cache: bool = True) -> sqlite3.Connection:
    conn = db.connect()
    if ensure_cache:
        project_state.ensure_sqlite_cache_current(conn)
    return conn


def _schema_block_response() -> tuple[int, dict] | None:
    try:
        cfg = config.get(reload=True)
    except config.ConfigError:
        return None
    schema = project_state.schema_status(cfg.volume_root)
    if schema.get("compatible"):
        return None
    if schema.get("migration_required"):
        return err(
            409,
            "Project schema migration required.",
            "Open this app from the browser and choose Migrate and open.",
        )
    return err(
        409,
        "Project schema is not supported by this Refine version.",
        schema.get("reason") or "",
    )


def _background_processes_stopped() -> bool:
    try:
        conn = _conn()
    except sqlite3.Error:
        return False
    try:
        return (db.get_setting(conn, "paused") or "0") == "1"
    finally:
        conn.close()


def _agents_paused(conn: sqlite3.Connection | None = None) -> bool:
    close_conn = False
    if conn is None:
        try:
            conn = _conn()
        except sqlite3.Error:
            return False
        close_conn = True
    try:
        return (
            (db.get_setting(conn, "paused") or "0") == "1"
            or (db.get_setting(conn, "agents_paused") or "0") == "1"
        )
    finally:
        if close_conn:
            conn.close()


def _background_processes_stopped_response() -> tuple[int, dict] | None:
    if not _background_processes_stopped():
        return None
    return err(
        409,
        "Background processes are stopped.",
        "Start Background before running worker actions.",
        error_code="background_processes_stopped",
    )


def _background_job_conflict_response(
    conflict: background_jobs.BackgroundJobConflict,
) -> tuple[int, dict]:
    job = conflict.job
    return err(
        409,
        "A background job is already running.",
        details=f"{job.get('label') or job.get('kind')} ({job.get('status')})",
        error_code="background_job_active",
    )


def _exclusive_mutation(
    label: str,
    *,
    allow_active_kinds: set[str] | None = None,
    allow_busy_when: Callable[[dict[str, Any]], bool] | None = None,
) -> Callable:
    def decorator(fn: Callable) -> Callable:
        @wraps(fn)
        def wrapped(*args, **kwargs):
            try:
                with background_jobs.exclusive_operation(
                    label,
                    allow_active_kinds=allow_active_kinds,
                ):
                    return fn(*args, **kwargs)
            except background_jobs.BackgroundJobConflict as e:
                if allow_busy_when is not None and allow_busy_when(e.job):
                    return fn(*args, **kwargs)
                return _background_job_conflict_response(e)
        return wrapped
    return decorator


# --- Files --------------------------------------------------------------------

def _target_repo_root() -> Path:
    return config.get(reload=True).client_repo.resolve()


def _resolve_repo_path(raw_path: str | None) -> tuple[Path, Path, str] | tuple[None, None, tuple[int, dict]]:
    root = _target_repo_root()
    raw = str(raw_path or "").replace("\\", "/").strip()
    while raw.startswith("/"):
        raw = raw[1:]
    parts = [part for part in raw.split("/") if part]
    if any(part == ".." for part in parts):
        return None, None, err(403, "path must stay inside the target repo")
    target = (root / "/".join(parts)).resolve()
    try:
        rel = target.relative_to(root)
    except ValueError:
        return None, None, err(403, "path must stay inside the target repo")
    return root, target, rel.as_posix() if rel.as_posix() != "." else ""


def _file_entry(root: Path, path: Path) -> dict:
    try:
        stat = path.stat()
    except OSError:
        stat = path.lstat()
    try:
        rel = path.resolve().relative_to(root).as_posix()
    except ValueError:
        rel = path.relative_to(root).as_posix()
    is_dir = path.is_dir()
    is_file = path.is_file()
    return {
        "name": path.name,
        "path": "" if rel == "." else rel,
        "type": "directory" if is_dir else "file" if is_file else "other",
        "size": stat.st_size,
        "modified": datetime.fromtimestamp(
            stat.st_mtime, timezone.utc,
        ).isoformat(),
        "symlink": path.is_symlink(),
    }


def _fuzzy_path_score(query: str, rel_path: str, name: str) -> tuple[int, int, int, str] | None:
    haystack = rel_path.lower()
    basename = name.lower()
    compact_haystack = re.sub(r"[^a-z0-9]+", "", haystack)
    compact_query = re.sub(r"[^a-z0-9]+", "", query.lower())
    if not compact_query:
        return None

    score = 0
    if basename == query:
        score += 2500
    if haystack == query:
        score += 2400
    if basename.startswith(query):
        score += 1800
    if haystack.startswith(query):
        score += 1500
    if query in basename:
        score += 1300 - basename.index(query)
    if query in haystack:
        score += 1000 - haystack.index(query)
    if compact_query in compact_haystack:
        score += 650 - compact_haystack.index(compact_query)

    pos = -1
    gap_penalty = 0
    boundary_bonus = 0
    for ch in compact_query:
        next_pos = compact_haystack.find(ch, pos + 1)
        if next_pos < 0:
            return (score, len(rel_path), rel_path.count("/"), rel_path) if score > 0 else None
        gap_penalty += max(0, next_pos - pos - 1)
        if next_pos == 0 or compact_haystack[next_pos - 1] in "/_-.":
            boundary_bonus += 25
        pos = next_pos
    score += 500 + boundary_bonus - gap_penalty
    score -= min(len(rel_path), 240)
    return (score, len(rel_path), rel_path.count("/"), rel_path)


def _file_browser_ignore_patterns() -> list[str]:
    raw = db.DEFAULT_SETTINGS.get("file_browser_ignore_patterns", FILE_BROWSER_IGNORE_DEFAULT)
    try:
        conn = db.connect()
        try:
            raw = db.get_setting(conn, "file_browser_ignore_patterns", raw) or raw
        finally:
            conn.close()
    except Exception:
        pass
    return [
        item.strip().replace("\\", "/").strip("/")
        for item in str(raw or "").split(",")
        if item.strip().strip("/")
    ]


def _rel_matches_file_browser_ignore(rel_path: str, patterns: list[str]) -> bool:
    rel = rel_path.strip("/")
    if not rel:
        return False
    parts = rel.split("/")
    for pattern in patterns:
        if "/" in pattern:
            if fnmatch.fnmatchcase(rel, pattern):
                return True
            continue
        if any(fnmatch.fnmatchcase(part, pattern) for part in parts):
            return True
    return False


def _repo_rel_path(root: Path, path: Path) -> str | None:
    try:
        return path.resolve().relative_to(root).as_posix()
    except (OSError, ValueError):
        return None


def _is_ignored_file_browser_entry(root: Path, path: Path, patterns: list[str]) -> bool:
    rel = _repo_rel_path(root, path)
    if rel is None:
        return False
    return _rel_matches_file_browser_ignore(rel, patterns)


def _list_file_entries(
    root: Path,
    target: Path,
    *,
    max_entries: int,
    ignore_patterns: list[str],
    apply_ignore: bool = True,
) -> tuple[list[dict], bool]:
    entries = []
    truncated = False
    try:
        for child in target.iterdir():
            if apply_ignore and _is_ignored_file_browser_entry(root, child, ignore_patterns):
                continue
            if len(entries) >= max_entries:
                truncated = True
                break
            entries.append(_file_entry(root, child))
    except OSError as e:
        raise PermissionError(str(e)) from e
    entries.sort(key=lambda item: (
        0 if item["type"] == "directory" else 1,
        item["name"].lower(),
    ))
    return entries, truncated


def files_tree(
    path: str | None = None,
    *,
    recursive: bool = False,
    max_depth: int = FILES_TREE_MAX_DEPTH,
    max_entries: int = FILES_TREE_MAX_ENTRIES,
) -> tuple[int, dict]:
    resolved = _resolve_repo_path(path)
    if resolved[0] is None:
        return resolved[2]
    root, target, rel = resolved
    if not target.exists():
        return err(404, "path not found")
    if not target.is_dir():
        return err(400, "path is not a directory")
    max_depth = max(0, min(FILES_TREE_MAX_DEPTH, int(max_depth)))
    max_entries = max(1, min(FILES_TREE_MAX_ENTRIES, int(max_entries)))
    ignore_patterns = _file_browser_ignore_patterns()
    apply_ignore = not _rel_matches_file_browser_ignore(rel, ignore_patterns)
    try:
        entries, truncated = _list_file_entries(
            root,
            target,
            max_entries=max_entries,
            ignore_patterns=ignore_patterns,
            apply_ignore=apply_ignore,
        )
    except PermissionError as e:
        return err(403, "directory cannot be read", str(e))
    body = {
        "root": str(root),
        "path": rel,
        "entries": entries,
        "truncated": truncated,
        "max_depth": max_depth,
        "max_entries": max_entries,
    }
    if not recursive:
        return 200, body

    entries_by_path: dict[str, list[dict]] = {rel: entries}
    meta_by_path: dict[str, dict] = {
        rel: {"truncated": truncated, "depth": 0},
    }
    total = len(entries)
    global_truncated = truncated

    def walk(dir_path: Path, dir_rel: str, depth: int) -> None:
        nonlocal total, global_truncated
        if depth >= max_depth or total >= max_entries:
            return
        current = entries_by_path.get(dir_rel, [])
        for entry in current:
            if total >= max_entries:
                global_truncated = True
                return
            if entry.get("type") != "directory":
                continue
            child = dir_path / entry["name"]
            try:
                child.resolve().relative_to(root)
            except ValueError:
                continue
            remaining = max_entries - total
            try:
                child_entries, child_truncated = _list_file_entries(
                    root,
                    child,
                    max_entries=remaining,
                    ignore_patterns=ignore_patterns,
                    apply_ignore=apply_ignore,
                )
            except PermissionError:
                entries_by_path[entry["path"]] = []
                meta_by_path[entry["path"]] = {
                    "truncated": False,
                    "depth": depth + 1,
                    "error": "directory cannot be read",
                }
                continue
            entries_by_path[entry["path"]] = child_entries
            meta_by_path[entry["path"]] = {
                "truncated": child_truncated,
                "depth": depth + 1,
            }
            total += len(child_entries)
            if child_truncated or total >= max_entries:
                global_truncated = True
            walk(child, entry["path"], depth + 1)

    walk(target, rel, 0)
    body.update({
        "entries_by_path": entries_by_path,
        "meta_by_path": meta_by_path,
        "total_entries": total,
        "truncated": global_truncated,
    })
    return 200, body


def files_search(
    query: str | None = None,
    *,
    max_entries: int = FILES_TREE_MAX_ENTRIES,
) -> tuple[int, dict]:
    root = _target_repo_root()
    q = str(query or "").strip().lower()
    max_entries = max(1, min(FILES_TREE_MAX_ENTRIES, int(max_entries)))
    if not q:
        return 200, {
            "root": str(root),
            "query": "",
            "entries": [],
            "truncated": False,
            "max_entries": max_entries,
        }
    matches = []
    scanned = 0
    truncated = False
    ignore_patterns = _file_browser_ignore_patterns()
    try:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [
                name for name in dirnames
                if not _rel_matches_file_browser_ignore(
                    _repo_rel_path(root, Path(dirpath) / name) or "",
                    ignore_patterns,
                )
            ]
            current = Path(dirpath)
            for name in [*dirnames, *filenames]:
                path = current / name
                scanned += 1
                if scanned > FILES_SEARCH_MAX_SCAN:
                    truncated = True
                    break
                try:
                    resolved = path.resolve()
                    rel = resolved.relative_to(root).as_posix()
                except (OSError, ValueError):
                    continue
                if _rel_matches_file_browser_ignore(rel, ignore_patterns):
                    continue
                score = _fuzzy_path_score(q, rel, path.name)
                if score is None:
                    continue
                entry = _file_entry(root, path)
                matches.append((score, entry))
            if truncated:
                break
    except OSError as e:
        return err(403, "file search cannot be completed", str(e))
    matches.sort(key=lambda item: (
        -item[0][0],
        0 if item[1]["type"] == "file" else 1,
        item[0][1],
        item[0][2],
        item[0][3],
    ))
    if len(matches) > max_entries:
        truncated = True
    entries = [entry for _score, entry in matches[:max_entries]]
    return 200, {
        "root": str(root),
        "query": q,
        "entries": entries,
        "truncated": truncated,
        "max_entries": max_entries,
        "scanned": scanned,
    }


def _count_lines_before(path: Path, offset: int) -> int:
    if offset <= 0:
        return 1
    count = 1
    remaining = offset
    with path.open("rb") as f:
        while remaining > 0:
            data = f.read(min(64 * 1024, remaining))
            if not data:
                break
            count += data.count(b"\n")
            remaining -= len(data)
    return count


def _decoded_text_looks_textual(text: str) -> bool:
    if not text:
        return True
    control = sum(
        1
        for ch in text
        if ord(ch) < 32 and ch not in "\t\n\f\r"
    )
    return control / len(text) <= 0.05


def _null_pattern_text_encoding(data: bytes) -> str | None:
    sample = data[:4096]
    if len(sample) < 4:
        return None
    odd = sample[1::2]
    even = sample[0::2]
    if odd and odd.count(0) / len(odd) > 0.45:
        return "utf-16-le"
    if even and even.count(0) / len(even) > 0.45:
        return "utf-16-be"
    return None


def _text_encoding_for_data(data: bytes) -> str | None:
    if not data:
        return "utf-8"
    if data.startswith(b"\xff\xfe\x00\x00"):
        return "utf-32-le"
    if data.startswith(b"\x00\x00\xfe\xff"):
        return "utf-32-be"
    if data.startswith(b"\xff\xfe"):
        return "utf-16-le"
    if data.startswith(b"\xfe\xff"):
        return "utf-16-be"
    if b"\0" in data:
        encoding = _null_pattern_text_encoding(data)
        if not encoding:
            return None
        try:
            text = data.decode(encoding)
        except UnicodeDecodeError:
            return None
        return encoding if _decoded_text_looks_textual(text) else None
    return "utf-8"


def _looks_binary_data(data: bytes) -> bool:
    encoding = _text_encoding_for_data(data)
    if encoding is None:
        return True
    if not data:
        return False
    if encoding != "utf-8":
        return False
    control = sum(
        1
        for byte in data
        if byte < 32 and byte not in (9, 10, 12, 13)
    )
    return control / len(data) > 0.30


def files_read(
    path: str | None = None,
    *,
    offset: int = 0,
    limit: int = FILE_TEXT_CHUNK_BYTES,
) -> tuple[int, dict]:
    resolved = _resolve_repo_path(path)
    if resolved[0] is None:
        return resolved[2]
    root, target, rel = resolved
    if not target.exists():
        return err(404, "path not found")
    if not target.is_file():
        return err(400, "path is not a file")
    try:
        stat = target.stat()
    except OSError as e:
        return err(403, "file cannot be read", str(e))
    offset = max(0, int(offset))
    limit = max(1, min(FILE_PREVIEW_MAX_BYTES, int(limit)))
    image_mime = IMAGE_MIME_BY_EXT.get(target.suffix.lower())
    base = {
        "root": str(root),
        "path": rel,
        "name": target.name,
        "size": stat.st_size,
        "modified": datetime.fromtimestamp(
            stat.st_mtime, timezone.utc,
        ).isoformat(),
        "kind": "text",
        "offset": offset,
        "limit": limit,
        "next_offset": None,
        "has_more": False,
        "start_line": 1,
        "large": stat.st_size > limit,
        "previewable": False,
        "content": "",
    }
    if image_mime:
        if stat.st_size > IMAGE_PREVIEW_MAX_BYTES:
            return 200, {
                **base,
                "kind": "image",
                "reason": "Image is too large to preview.",
            }
        try:
            data = target.read_bytes()
        except OSError as e:
            return err(403, "file cannot be read", str(e))
        encoded = base64.b64encode(data).decode("ascii")
        return 200, {
            **base,
            "kind": "image",
            "mime": image_mime,
            "previewable": True,
            "data_url": f"data:{image_mime};base64,{encoded}",
        }
    try:
        with target.open("rb") as f:
            head = f.read(4096)
            if _looks_binary_data(head):
                return 200, {**base, "kind": "binary", "reason": "Binary data"}
            f.seek(offset)
            data = f.read(limit)
    except OSError as e:
        return err(403, "file cannot be read", str(e))
    encoding = _text_encoding_for_data(head) or "utf-8"
    text = data.decode(encoding, errors="replace")
    if offset == 0 and text.startswith("\ufeff"):
        text = text[1:]
    next_offset = offset + len(data)
    has_more = next_offset < stat.st_size
    try:
        start_line = _count_lines_before(target, offset)
    except OSError:
        start_line = 1
    return 200, {
        **base,
        "previewable": True,
        "content": text,
        "offset": offset,
        "next_offset": next_offset if has_more else None,
        "has_more": has_more,
        "start_line": start_line,
        "large": stat.st_size > len(data),
    }


def _instance_owner(instance_id: str | None) -> str:
    return str(instance_id or project_state.DEFAULT_INSTANCE_ID)


def _ownership_error(
    owner_id: str | None,
    *,
    active_id: str | None = None,
    count: int = 1,
) -> tuple[int, dict]:
    owner = _instance_owner(owner_id)
    active = active_id or project_state.active_instance_id()
    owner_name = project_state.gap_instance_display(owner)
    active_name = project_state.gap_instance_display(active)
    subject = "Gap is" if count == 1 else f"{count} Gaps are"
    return err(
        409,
        (
            f"Action not allowed: {subject} owned by another instance "
            f"({owner_name}). Transfer to {active_name} before making changes."
        ),
        error_code="instance_ownership",
    )


def _require_active_gap(
    conn: sqlite3.Connection,
    gap_id: str,
    *,
    columns: str = "status, branch_name, instance_id",
) -> tuple[sqlite3.Row | None, tuple[int, dict] | None]:
    row = conn.execute(
        f"SELECT {columns} FROM gaps_index WHERE id = ?",
        (gap_id,),
    ).fetchone()
    if not row:
        return None, err(404, "Gap not found")
    active = project_state.active_instance_id()
    if _instance_owner(row["instance_id"]) != active:
        return None, _ownership_error(row["instance_id"], active_id=active)
    return row, None


def _require_active_gap_ids(gap_ids: list[str]) -> tuple[bool, tuple[int, dict] | None]:
    if not gap_ids:
        return True, None
    conn = _conn()
    try:
        active = project_state.active_instance_id()
        rows = []
        for chunk in _id_chunks(gap_ids):
            placeholders = ",".join("?" * len(chunk))
            rows.extend(conn.execute(
                f"SELECT id, instance_id FROM gaps_index WHERE id IN ({placeholders})",
                chunk,
            ).fetchall())
    finally:
        conn.close()
    violations = [
        _instance_owner(row["instance_id"])
        for row in rows
        if _instance_owner(row["instance_id"]) != active
    ]
    if violations:
        return False, _ownership_error(
            sorted(set(violations))[0],
            active_id=active,
            count=len(violations),
        )
    return True, None


def _id_chunks(values: list[str], size: int = 500) -> list[list[str]]:
    return [values[idx:idx + size] for idx in range(0, len(values), size)]


def _selected_gap_ids(body: dict[str, Any]) -> list[str] | None:
    raw = body.get("selected_ids")
    if raw is None:
        raw = body.get("gap_ids")
    if raw is None:
        return None
    if not isinstance(raw, list):
        return []
    ids: list[str] = []
    seen: set[str] = set()
    for item in raw:
        gap_id = str(item or "").strip()
        if not gap_id or gap_id in seen:
            continue
        ids.append(gap_id)
        seen.add(gap_id)
    return ids


def _append_gap_workflow_log(
    gap_id: str,
    message: str,
    *,
    severity: str = "info",
    actor: str = "refine",
    details: str | None = None,
) -> None:
    try:
        gap_writer.append_latest_round_log(
            gap_id=gap_id,
            severity=severity,
            category="state",
            actor=actor,
            message=message,
            details=details,
        )
    except Exception:
        pass


# --- Project attach/setup -----------------------------------------------------

def project_status() -> tuple[int, dict]:
    """Return whether this UI process is attached to a refine project."""
    clone_dir = Path.cwd().resolve()
    registry_enabled = _project_registry_enabled(clone_dir)
    apps = project_registry.list_apps(clone_dir) if registry_enabled else []
    cfg_path = config.find_config()
    if cfg_path is None:
        return 200, {
            "attached": False,
            "apps": apps,
            "registry_enabled": registry_enabled,
            "message": "No refine project is attached.",
        }
    try:
        cfg = config.get(reload=True)
    except config.ConfigError as e:
        return 200, {
            "attached": False,
            "apps": apps,
            "registry_enabled": registry_enabled,
            "config_path": str(cfg_path),
            "message": str(e),
        }
    if registry_enabled:
        apps = project_registry.upsert_app(clone_dir, cfg.client_repo, make_current=True)
    else:
        apps = _ensure_current_app(apps, cfg.client_repo)
    schema = project_state.schema_status(cfg.volume_root)
    instance_summary = _instance_summary() if schema.get("compatible") else {
        "instances": [],
        "active_instance_id": "",
    }
    return 200, {
        "attached": True,
        "apps": apps,
        "registry_enabled": registry_enabled,
        "client_repo": str(cfg.client_repo),
        "volume_root": str(cfg.volume_root),
        "config_path": str(cfg.config_path),
        "schema": schema,
        **instance_summary,
    }


def project_list() -> tuple[int, dict]:
    clone_dir = Path.cwd().resolve()
    current = ""
    apps = project_registry.list_apps(clone_dir) if _project_registry_enabled(clone_dir) else []
    try:
        current_repo = config.get(reload=True).client_repo
        current = str(current_repo)
        apps = _ensure_current_app(apps, current_repo)
    except config.ConfigError:
        pass
    return 200, {
        "apps": apps,
        "current": current,
    }


def project_remove(body: dict[str, Any]) -> tuple[int, dict]:
    raw_path = (body.get("path") or "").strip()
    if not raw_path:
        return err(400, "Choose an app to remove.")
    clone_dir = Path.cwd().resolve()
    if not _project_registry_enabled(clone_dir):
        return err(409, "Known-apps list is only available from the host refine source checkout.")
    target = Path(raw_path).expanduser().resolve()
    try:
        current = config.get(reload=True).client_repo
    except config.ConfigError:
        current = None
    apps = project_registry.remove_app(clone_dir, target)
    if current is not None and current == target:
        _detach_current_project(clone_dir, target)
        status, body = project_status()
        body["removed_path"] = str(target)
        return status, body
    return 200, {"apps": apps}


@_exclusive_mutation("Sync project")
def project_sync(_: dict[str, Any] | None = None) -> tuple[int, dict]:
    block = _schema_block_response()
    if block:
        return block
    try:
        result = get_client().call(M_PROJECT_SYNC, {}, timeout=120.0)
    except BackendError as e:
        return _backend_err(e)
    if not result.get("ok"):
        return err(
            409,
            result.get("message") or "Could not sync latest target-app updates.",
            result.get("details") or "",
        )
    return 200, result


# --- Instances ---------------------------------------------------------------

def list_instances() -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    conn = _conn()
    try:
        counts = {}
        for row in conn.execute(
            "SELECT instance_id, status, COUNT(*) AS n "
            "FROM gaps_index GROUP BY instance_id, status"
        ):
            counts.setdefault(row["instance_id"] or "", {})[row["status"]] = row["n"]
    finally:
        conn.close()
    instances = project_state.list_instances()
    known = {i.get("id") for i in instances}
    unknown_ids = [iid for iid in counts if iid and iid not in known]
    return 200, {
        "instances": instances,
        "active_instance_id": project_state.active_instance_id(),
        "counts": counts,
        "unknown_instance_ids": unknown_ids,
    }


@_exclusive_mutation("Create instance")
def create_instance(body: dict[str, Any]) -> tuple[int, dict]:
    name = (body.get("display_name") or body.get("name") or "").strip()
    if not name:
        return err(400, "display_name is required")
    entry = project_state.create_instance(name)
    _rebuild_cache()
    return 201, {"instance": entry, **_instance_summary()}


@_exclusive_mutation("Update instance")
def update_instance(instance_id: str, body: dict[str, Any]) -> tuple[int, dict]:
    try:
        entry = project_state.update_instance(
            instance_id,
            display_name=body.get("display_name") if "display_name" in body else body.get("name"),
            archived=body.get("archived") if "archived" in body else None,
        )
    except ValueError as e:
        return err(400, str(e))
    _rebuild_cache()
    return 200, {"instance": entry, **_instance_summary()}


@_exclusive_mutation("Copy instance settings")
def copy_instance_settings(body: dict[str, Any]) -> tuple[int, dict]:
    source = (body.get("source_instance_id") or body.get("instance_id") or "").strip()
    section = (body.get("section") or "").strip()
    if not source:
        return err(400, "source_instance_id is required")
    try:
        result = project_state.copy_instance_settings(source, section)
    except ValueError as e:
        return err(400, str(e))
    _rebuild_cache()
    conn = _conn()
    try:
        settings = db.list_settings(conn)
        activity.append(
            conn,
            message=(
                f"Copied {section} settings from "
                f"{project_state.gap_instance_display(source)}"
            ),
            severity="info",
            category="settings",
            actor="refine",
        )
    finally:
        conn.close()
    return 200, {"ok": True, "settings": settings, **result}


@_exclusive_mutation("Activate instance")
def activate_instance(body: dict[str, Any]) -> tuple[int, dict]:
    instance_id = (body.get("instance_id") or body.get("id") or "").strip()
    if not instance_id:
        return err(400, "instance_id is required")
    try:
        project_state.set_active_instance(instance_id)
    except ValueError as e:
        return err(400, str(e))
    _rebuild_cache()
    try:
        get_client().call(
            M_CHAT_RESET_ALL,
            {"reason": "instance activated"},
            timeout=10.0,
        )
    except BackendError:
        pass
    try:
        get_client().call(M_ENFORCE_SCHEDULING, {}, timeout=10.0)
    except BackendError:
        pass
    return 200, {"ok": True, **_instance_summary()}


@_exclusive_mutation("Transfer Gaps between instances")
def transfer_instance_gaps(body: dict[str, Any]) -> tuple[int, dict]:
    target = (body.get("target_instance_id") or "").strip()
    source = (body.get("source_instance_id") or "").strip() or None
    if not target:
        return err(400, "target_instance_id is required")
    target_instance = project_state.instance_by_id(target)
    if target_instance is None:
        return err(400, f"unknown target instance: {target}")
    if target_instance.get("archived"):
        return err(400, f"archived target instance: {target}")
    statuses = body.get("statuses")
    allowed = None
    if statuses is not None:
        if not isinstance(statuses, list):
            return err(400, "statuses must be a list")
        allowed = {str(s) for s in statuses if str(s) in _VALID_STATUSES}
    selected_ids = _selected_gap_ids(body)
    gap_ids = set(selected_ids) if selected_ids is not None else None
    if selected_ids == []:
        return 200, {
            "updated": 0,
            "ids": [],
            "skipped": 0,
            "skipped_details": [],
        }
    filt = body.get("filter")
    if gap_ids is None and isinstance(filt, dict):
        excluded = set(body.get("exclude_ids") or [])
        code, listing = list_gaps(
            status=filt.get("status") or None,
            q=filt.get("q") or None,
            severity=filt.get("severity") or None,
            category=filt.get("category") or None,
            actor=filt.get("actor") or None,
            reporter=filt.get("reporter") or None,
            instance=filt.get("instance") or None,
            limit=10_000,
        )
        if code != 200:
            return code, listing
        gap_ids = {
            g["id"] for g in (listing.get("gaps") or [])
            if g["id"] not in excluded
        }
        if not gap_ids:
            return 200, {
                "updated": 0,
                "ids": [],
                "skipped": 0,
                "skipped_details": [],
            }
    try:
        cancelled = (
            _cancel_active_transfer_gaps(source, gap_ids)
            if body.get("cancel_active")
            else {
                "paused": False,
                "stopped_processes": 0,
                "cancelled": 0,
                "cancelled_ids": [],
            }
        )
    except BackendError as e:
        return _backend_err(e)
    try:
        result = project_state.transfer_gaps(
            source, target, statuses=allowed, gap_ids=gap_ids,
        )
    except ValueError as e:
        return err(400, str(e))
    _rebuild_cache()
    if _should_enforce_after_instance_transfer(result.get("ids") or []):
        try:
            get_client().call(M_ENFORCE_SCHEDULING, {}, timeout=10.0)
        except BackendError:
            pass
    result.update(cancelled)
    return 200, result


def _should_enforce_after_instance_transfer(gap_ids: list[str]) -> bool:
    if not gap_ids:
        return False
    active = project_state.active_instance_id()
    placeholders = ",".join("?" * len(gap_ids))
    conn = _conn()
    try:
        if _agents_paused(conn):
            return False
        row = conn.execute(
            "SELECT COUNT(*) AS n FROM gaps_index "
            f"WHERE id IN ({placeholders}) "
            "AND instance_id = ? AND status = 'todo'",
            [*gap_ids, active],
        ).fetchone()
        return bool(row and int(row["n"] or 0) > 0)
    finally:
        conn.close()


def list_guidance() -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    return 200, {"guidance": project_state.list_guidance()}


def update_guidance(body: dict[str, Any]) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    items = body.get("guidance")
    if not isinstance(items, list):
        return err(400, "guidance must be a list")
    normalized = []
    for item in items:
        if not isinstance(item, dict):
            return err(400, "each guidance item must be an object")
        normalized.append(project_state.normalize_guidance_item(item))
    saved = project_state.write_guidance(normalized)
    return 200, {"guidance": saved}


def _cancel_active_transfer_gaps(
    source_instance_id: str | None,
    gap_ids: set[str] | None,
) -> dict[str, Any]:
    conn = _conn()
    try:
        project_state.set_setting("agents_paused", "1")
        db.set_setting(conn, "agents_paused", "1")
        where = ["status IN ('in-progress', 'qa', 'ready-merge', 'awaiting-rebuild')"]
        args: list[Any] = []
        if source_instance_id:
            where.append("instance_id = ?")
            args.append(source_instance_id)
        if gap_ids is not None:
            if not gap_ids:
                return {
                    "paused": True,
                    "stopped_processes": 0,
                    "cancelled": 0,
                    "cancelled_ids": [],
                }
            where.append("id IN (" + ",".join("?" * len(gap_ids)) + ")")
            args.extend(sorted(gap_ids))
        rows = conn.execute(
            "SELECT id FROM gaps_index WHERE " + " AND ".join(where),
            args,
        ).fetchall()
        active_ids = [r["id"] for r in rows]
        activity.append(
            conn,
            message="Agents paused for instance transfer",
            severity="warn",
            category="state",
            actor="refine",
        )
    finally:
        conn.close()

    stopped = 0
    result = get_client().call(M_CANCEL_ALL, {"reason": "paused"}, timeout=10.0)
    stopped = int(result.get("killed_subprocesses") or 0)
    cancelled_ids: list[str] = []
    for gid in active_ids:
        get_client().call(M_CANCEL, {"gap_id": gid}, timeout=30.0)
        cancelled_ids.append(gid)
    return {
        "paused": True,
        "stopped_processes": stopped,
        "cancelled": len(cancelled_ids),
        "cancelled_ids": cancelled_ids,
    }


def _rebuild_cache() -> None:
    conn = _conn(ensure_cache=False)
    try:
        project_state.rebuild_sqlite_cache(conn)
    finally:
        conn.close()


def _sqlite_cache_files(sqlite_file: Path) -> list[Path]:
    return [
        sqlite_file,
        Path(f"{sqlite_file}-wal"),
        Path(f"{sqlite_file}-shm"),
    ]


def _unlink_sqlite_cache_files(sqlite_file: Path) -> list[str]:
    removed: list[str] = []
    for path in _sqlite_cache_files(sqlite_file):
        try:
            path.unlink()
            removed.append(path.name)
        except FileNotFoundError:
            continue
    return removed


def _sqlite_cache_counts(conn: sqlite3.Connection) -> dict[str, int]:
    return {
        "gaps": int(
            conn.execute("SELECT COUNT(*) AS n FROM gaps_index").fetchone()["n"] or 0,
        ),
        "reporters": int(
            conn.execute("SELECT COUNT(*) AS n FROM reporters").fetchone()["n"] or 0,
        ),
    }


def background_job(job_id: str) -> tuple[int, dict]:
    job = background_jobs.snapshot(job_id)
    if job is None:
        return err(404, "Background job not found")
    return 200, {"job": job}


def cancel_background_job(job_id: str) -> tuple[int, dict]:
    job = background_jobs.cancel(job_id)
    if job is None:
        return err(404, "Background job not found")
    return 200, {"job": job}


def _cancel_active_background_jobs() -> list[dict[str, Any]]:
    try:
        conn = _conn()
        try:
            rows = conn.execute(
                "SELECT id FROM background_jobs "
                "WHERE status IN ('queued', 'running') "
                "ORDER BY started_at DESC LIMIT 100",
            ).fetchall()
        finally:
            conn.close()
    except Exception:
        rows = []
    cancelled: list[dict[str, Any]] = []
    seen: set[str] = set()
    for row in rows:
        job_id = str(row["id"])
        if job_id in seen:
            continue
        seen.add(job_id)
        job = background_jobs.cancel(job_id)
        if job:
            cancelled.append(job)
    return cancelled


def performance_summary(*, operation: str | None = None,
                        success: str | None = None,
                        limit: int = 50,
                        offset: int = 0) -> tuple[int, dict]:
    conn = _conn()
    try:
        success_filter: bool | None = None
        if success in ("1", "true", "ok", "success"):
            success_filter = True
        elif success in ("0", "false", "failed", "failure"):
            success_filter = False
        snapshot = perf_metrics.snapshot(
            conn,
            days=perf_metrics.RETENTION_DAYS,
            limit=limit,
            offset=offset,
            operation=operation or None,
            success=success_filter,
        )
        snapshot["backend"] = runtime.backend_info()
        return 200, snapshot
    finally:
        conn.close()


def performance_cleanup(body: dict | None = None) -> tuple[int, dict]:
    body = body or {}
    conn = _conn()
    try:
        if body.get("clear"):
            deleted = perf_metrics.clear(conn)
            return 200, {
                "deleted": deleted,
                "retention_days": perf_metrics.RETENTION_DAYS,
            }
        deleted = perf_metrics.prune(conn, days=perf_metrics.RETENTION_DAYS)
        return 200, {
            "deleted": deleted,
            "retention_days": perf_metrics.RETENTION_DAYS,
        }
    finally:
        conn.close()


@_exclusive_mutation("Rebuild SQLite cache")
def rebuild_sqlite_cache(body: dict | None = None) -> tuple[int, dict]:
    """Operator recovery path for a stale or corrupted SQLite cache."""
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped

    body = body or {}
    if body.get("background"):
        restart_services = body.get("restart_services") is not False

        def run_job(progress=None) -> dict[str, Any]:
            def report(completed: int, total: int, message: str) -> None:
                if progress is not None:
                    progress(completed=completed, total=total, message=message)

            status, result = _rebuild_sqlite_cache_sync(
                {"restart_services": restart_services},
                progress=report,
            )
            return {"http_status": status, **result}

        try:
            job = background_jobs.start(
                "sqlite_cache_rebuild",
                "Rebuild SQLite cache",
                run_job,
            )
        except background_jobs.BackgroundJobConflict as e:
            return _background_job_conflict_response(e)
        return 202, {"queued": True, "job": job}

    return _rebuild_sqlite_cache_sync(body)


def _rebuild_sqlite_cache_sync(
    body: dict,
    *,
    progress: project_state.ProgressCallback | None = None,
) -> tuple[int, dict]:
    """Force-rebuild SQLite projections from canonical .refine JSON."""
    restart_services = body.get("restart_services") is not False
    cfg = config.get(reload=True)
    sqlite_file = cfg.sqlite_path
    backend = runtime.backend_info()
    controls_runner_lifecycle = bool(backend.get("ui_controls_runner_lifecycle"))
    mode = "rebuilt"
    details = ""
    removed: list[str] = []

    if restart_services and controls_runner_lifecycle:
        runtime.stop_all()
    elif restart_services:
        runtime.stop_poller()
    try:
        try:
            db.init_db(sqlite_file)
            conn = db.connect(sqlite_file)
            try:
                integrity = conn.execute("PRAGMA integrity_check").fetchone()
                if integrity is None or str(integrity[0]).lower() != "ok":
                    detail = str(integrity[0]) if integrity is not None else "no integrity result"
                    raise sqlite3.DatabaseError(f"integrity_check failed: {detail}")
                project_state.rebuild_sqlite_cache(conn, force=True, progress=progress)
            finally:
                conn.close()
        except sqlite3.Error as e:
            mode = "recreated"
            details = str(e)
            if progress is not None:
                progress(0, 0, "Recreating corrupted SQLite cache")
            removed = _unlink_sqlite_cache_files(sqlite_file)
            db.init_db()

        conn = db.connect(sqlite_file)
        try:
            project_state.ensure_sqlite_cache_current(conn)
            counts = _sqlite_cache_counts(conn)
            activity.append(
                conn,
                message=(
                    "SQLite cache recreated from canonical JSON"
                    if mode == "recreated"
                    else "SQLite cache rebuilt from canonical JSON"
                ),
                severity="info",
                category="state",
                actor="refine",
                details=details or None,
            )
        finally:
            conn.close()
    finally:
        if restart_services:
            runtime.ensure_poller()
            if controls_runner_lifecycle:
                runtime.ensure_runner()

    return 200, {
        "ok": True,
        "mode": mode,
        "path": str(sqlite_file),
        "removed": removed,
        "backend": backend,
        "runner_restarted": bool(restart_services and controls_runner_lifecycle),
        "poller_restarted": bool(restart_services),
        **counts,
    }


def project_attach(body: dict[str, Any]) -> tuple[int, dict]:
    """Create or attach a target app path and make it active."""
    raw_path = (body.get("path") or "").strip()
    if not raw_path:
        return err(400, "Enter a project path or Git remote.")

    clone_dir = Path.cwd().resolve()

    try:
        from refine_cli.cli import (
            _InitError, _is_refine_source_dir, bootstrap_client_repo,
        )

        if not _is_refine_source_dir(clone_dir):
            return err(
                409,
                "Project setup must run from the host refine source directory.",
                (
                    f"The UI process is running in {clone_dir}. Start refine from "
                    "the source checkout with `uv run refine start` so it can "
                    "create host directories and write the binding."
                ),
        )

        client_repo = (
            _clone_project_remote(raw_path, clone_dir)
            if _looks_like_git_remote(raw_path)
            else Path(raw_path).expanduser()
        )
        current_before = _current_client_repo()
        switching = current_before is not None and current_before != client_repo.resolve()
        _validate_target_schema_before_switch(client_repo.resolve(), body)
        backend = runtime.backend_info()
        supervised_restart = (
            backend.get("process_model") == "supervisor"
            and (switching or current_before is None)
        )
        prep = (
            _prepare_current_project_for_switch(clone_dir)
            if switching else {"warnings": []}
        )

        install_unit = body.get("install_unit") is True
        result = bootstrap_client_repo(
            client_repo,
            clone_dir=clone_dir,
            force=True,
            create=True,
            init_git=True,
            reuse_existing_config=True,
            install_unit=install_unit,
        )
        if supervised_restart:
            cfg = config.Config.load(result["config_path"])
            schema = _prepare_supervised_switch_target(
                cfg,
                migrate=bool(body.get("migrate")),
            )
            if body.get("migrate"):
                _commit_refine_state(cfg.client_repo)
            restart = _schedule_supervisor_restart(clone_dir, cfg)
            return 200, {
                "attached": True,
                "client_repo": str(cfg.client_repo),
                "volume_root": str(cfg.volume_root),
                "config_path": str(cfg.config_path),
                "binding_path": str(result["binding_path"]) if result.get("binding_path") else "",
                "unit_path": str(result["unit_path"]) if result.get("unit_path") else "",
                "ui_unit_path": str(result["ui_unit_path"]) if result.get("ui_unit_path") else "",
                "git_initialized": bool(result.get("git_initialized")),
                "config_created": bool(result.get("config_created")),
                "apps": project_registry.list_apps(clone_dir),
                "registry_enabled": True,
                "schema": schema,
                "active_instance_id": "",
                "active_instance": None,
                "instances": [],
                "switch_warnings": prep.get("warnings", []),
                "restart_pending": True,
                "restart": restart,
                "runner": {
                    "started": False,
                    "message": "Refine is restarting for the selected app.",
                },
            }
        cfg = runtime.load_configured(
            result["config_path"],
            start_poller=body.get("start_poller") is not False,
            start_runner=body.get("start_runner") is not False,
            migrate=bool(body.get("migrate")),
        )
        if body.get("migrate"):
            _commit_refine_state(cfg.client_repo)
    except (config.ConfigError, _InitError, OSError, TimeoutError) as e:
        return err(400, str(e))
    except _SwitchBlocked as e:
        return err(409, str(e), e.details)

    runner = {"started": False, "message": ""}
    if body.get("start_runner") is not False:
        runner = {"started": True, "message": "Backend runner started."}

    return 200, {
        "attached": True,
        "client_repo": str(cfg.client_repo),
        "volume_root": str(cfg.volume_root),
        "config_path": str(cfg.config_path),
        "binding_path": str(result["binding_path"]) if result.get("binding_path") else "",
        "unit_path": str(result["unit_path"]) if result.get("unit_path") else "",
        "ui_unit_path": str(result["ui_unit_path"]) if result.get("ui_unit_path") else "",
        "git_initialized": bool(result.get("git_initialized")),
        "config_created": bool(result.get("config_created")),
        "apps": project_registry.list_apps(clone_dir),
        "registry_enabled": True,
        "schema": project_state.schema_status(cfg.volume_root),
        **_instance_summary(),
        "switch_warnings": prep.get("warnings", []),
        "runner": runner,
    }


def _project_registry_enabled(clone_dir: Path) -> bool:
    return (clone_dir / "pyproject.toml").is_file() and (clone_dir / "refine_cli" / "cli.py").is_file()


def _looks_like_git_remote(value: str) -> bool:
    text = str(value or "").strip()
    if text.startswith(("git@", "ssh://", "git://")):
        return True
    parsed = urlparse(text)
    return parsed.scheme in {"http", "https", "file"} and bool(parsed.netloc or parsed.scheme == "file")


def _clone_project_remote(remote: str, clone_dir: Path) -> Path:
    git = shutil.which("git")
    if git is None:
        raise config.ConfigError("could not find `git` on PATH; install git or use a local app path")
    target = _default_project_clone_path(remote, clone_dir)
    if target.exists():
        if (target / ".git").exists():
            return target
        try:
            has_entries = any(target.iterdir())
        except OSError as e:
            raise config.ConfigError(f"cannot inspect clone target {target}: {e}") from e
        if has_entries:
            raise config.ConfigError(
                f"clone target already exists and is not empty: {target}"
            )
    target.parent.mkdir(parents=True, exist_ok=True)
    result = subprocess.run(
        [git, "clone", remote, str(target)],
        cwd=str(clone_dir),
        capture_output=True,
        text=True,
        timeout=600,
    )
    if result.returncode != 0:
        detail = (result.stderr or result.stdout or "git clone failed").strip()
        raise config.ConfigError(
            "could not clone target app. Check the Git remote and host credentials: "
            + detail
        )
    return target


def _default_project_clone_path(remote: str, clone_dir: Path) -> Path:
    parsed = urlparse(remote)
    raw_name = Path(parsed.path or remote.rstrip("/")).name or "target-app"
    if raw_name.endswith(".git"):
        raw_name = raw_name[:-4]
    name = re.sub(r"[^A-Za-z0-9._-]+", "-", raw_name).strip(".-") or "target-app"
    base = clone_dir.parent
    candidate = base / name
    if not candidate.exists() or (candidate / ".git").exists():
        return candidate
    i = 2
    while True:
        numbered = base / f"{name}-{i}"
        if not numbered.exists() or (numbered / ".git").exists():
            return numbered
        i += 1


def _validate_target_schema_before_switch(client_repo: Path, body: dict[str, Any]) -> None:
    existing_refine = client_repo / ".refine"
    existing_cfg = existing_refine / config.CONFIG_FILENAME
    if not existing_cfg.exists():
        return
    schema = project_state.schema_status(existing_refine)
    migrate_requested = bool(body.get("migrate"))
    if schema.get("compatible"):
        return
    if (
        schema.get("migration_required")
        and not migrate_requested
        and not project_state.empty_refine_state(existing_refine)
    ):
        raise _SwitchBlocked(
            "Project schema migration required.",
            "Open this app with migrate=true to upgrade .refine state before switching.",
        )
    if not schema.get("migration_required"):
        raise _SwitchBlocked(
            "Project schema is not supported by this Refine version.",
            schema.get("reason") or "",
        )


def _ensure_current_app(apps: list[dict[str, str]], client_repo: Path) -> list[dict[str, str]]:
    """Always include the active app, even when the clone-local registry is unavailable."""
    current = str(client_repo.resolve())
    if any(app.get("path") == current for app in apps):
        return apps
    return [
        *apps,
        {
            "name": client_repo.name or current,
            "path": current,
            "added_at": "",
            "last_used_at": "",
        },
    ]


def _detach_current_project(clone_dir: Path, target: Path) -> None:
    binding = clone_dir / config.BINDING_FILENAME
    if binding.exists():
        try:
            bound = config.read_binding(binding)
        except config.ConfigError:
            bound = None
        if bound is None or bound == target.resolve():
            try:
                binding.unlink()
            except FileNotFoundError:
                pass
    runtime.detach_configured()


def _instance_summary() -> dict[str, Any]:
    try:
        instances = project_state.list_instances()
        active = project_state.active_instance_id()
    except Exception:
        return {"instances": [], "active_instance_id": ""}
    return {
        "instances": instances,
        "active_instance_id": active,
        "active_instance": next(
            (i for i in instances if i.get("id") == active),
            None,
        ),
    }


class _SwitchBlocked(Exception):
    def __init__(self, message: str, details: str | None = None) -> None:
        super().__init__(message)
        self.details = details


def _current_client_repo() -> Path | None:
    try:
        return config.get(reload=True).client_repo
    except config.ConfigError:
        return None


def _prepare_current_project_for_switch(clone_dir: Path) -> dict[str, Any]:
    """Stop active agents and leave the current target app clean before switching."""
    warnings: list[str] = []
    cfg = config.get(reload=True)
    if runtime.backend_info().get("process_model") == "supervisor":
        warnings.append(
            "Refine will restart so the UI and runner worker use the selected app."
        )
    else:
        runtime.stop_runner()

    _commit_refine_state(cfg.client_repo)
    dirty = _git_stdout(cfg.client_repo, ["status", "--porcelain"])
    if dirty.strip():
        raise _SwitchBlocked(
            "Current app has uncommitted changes.",
            (
                "Commit, stash, or discard changes in the current app before switching:\n"
                + dirty.strip()
            ),
        )
    return {"warnings": warnings}


def _prepare_supervised_switch_target(
    cfg: config.Config,
    *,
    migrate: bool,
) -> dict[str, Any]:
    """Prepare target .refine state without hot-loading it in this UI process."""
    schema = project_state.schema_status(cfg.volume_root)
    if schema.get("compatible"):
        return project_state.ensure_initialized(
            migrate=False,
            root=cfg.volume_root,
        )
    if not schema.get("migration_required"):
        return schema

    conn: sqlite3.Connection | None = None
    try:
        if project_state.empty_refine_state(cfg.volume_root):
            db.init_db(cfg.sqlite_path)
            conn = db.connect(cfg.sqlite_path)
        elif not migrate:
            return schema
        return project_state.ensure_initialized(
            conn,
            migrate=True,
            root=cfg.volume_root,
        )
    finally:
        if conn is not None:
            conn.close()


def _schedule_supervisor_restart(clone_dir: Path, cfg: config.Config) -> dict[str, Any]:
    try:
        port = int(os.environ.get("REFINE_UI_PORT") or cfg.web_port)
    except ValueError:
        port = cfg.web_port
    run_dir = config.local_run_dir(clone_dir)
    run_dir.mkdir(parents=True, exist_ok=True)
    log_path = run_dir / f"restart-{port}.log"
    command = [
        sys.executable,
        "-m",
        "refine_cli",
        "--config",
        str(cfg.config_path),
        "restart",
        str(port),
    ]
    launcher = [
        sys.executable,
        "-c",
        (
            "import os, sys, time; "
            "time.sleep(0.5); "
            "os.execvpe(sys.argv[1], sys.argv[1:], os.environ)"
        ),
        *command,
    ]
    env = os.environ.copy()
    env.pop("REFINE_RUNNER_SOCKET", None)
    env.pop("REFINE_NO_INPROCESS_RUNNER", None)
    env.pop("REFINE_SUPERVISOR_PID", None)
    env[config.ENV_CONFIG_PATH] = str(cfg.config_path)
    env["REFINE_UI_PORT"] = str(port)
    with log_path.open("ab") as log:
        proc = subprocess.Popen(
            launcher,
            cwd=str(clone_dir),
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=subprocess.STDOUT,
            env=env,
            start_new_session=True,
        )
    return {
        "scheduled": True,
        "pid": proc.pid,
        "port": port,
        "log_path": str(log_path),
    }


def _commit_refine_state(repo: Path) -> None:
    from refine_server import git_ops, push_ops

    config.ensure_refine_gitignore(repo / ".refine")
    dirty_refine = _git_stdout(repo, ["status", "--porcelain", "--", ".refine"])
    if not dirty_refine.strip():
        return
    paths = git_ops.dirty_paths_under(".refine", cwd=repo)
    result = git_ops.commit_refine_sync_state(
        paths,
        state_message="refine: sync project state before switch",
        cwd=repo,
    )
    if result.ok and result.stderr == "(nothing to commit)":
        return
    if result.ok:
        repo_cfg = config.Config.load(repo / ".refine" / config.CONFIG_FILENAME)
        db.init_db(repo_cfg.sqlite_path)
        conn = db.connect(repo_cfg.sqlite_path)
        try:
            push = push_ops.push_current_after_pull(
                conn,
                actor="refine",
                cwd=repo,
                merge_message="Merge upstream before pushing Refine project state",
                prompt_context=(
                    "A pull is in progress before pushing Refine project state.\n"
                    "HEAD contains local `.refine/` state commits created by Refine.\n"
                    "The incoming side contains newer upstream commits.\n"
                    "Preserve durable `.refine/` state from both sides. If JSON files "
                    "conflict, keep valid JSON and include all non-duplicate entries."
                ),
            )
        finally:
            conn.close()
        if push.get("ok"):
            return
        raise _SwitchBlocked(
            "Could not push current app Refine state.",
            str(push.get("details") or push.get("message") or "git push failed").strip(),
        )
    raise _SwitchBlocked(
        "Could not commit current app Refine state.",
        (result.stderr or result.stdout or "git commit failed").strip(),
    )


def _git_stdout(repo: Path, args: list[str]) -> str:
    out = subprocess.run(
        ["git", *args], cwd=str(repo), capture_output=True, text=True, timeout=30,
    )
    if out.returncode != 0:
        raise _SwitchBlocked(
            "Could not inspect current app git state.",
            (out.stderr or out.stdout or f"git {' '.join(args)} failed").strip(),
        )
    return out.stdout


def _git_checked(repo: Path, args: list[str]) -> None:
    out = subprocess.run(
        ["git", *args], cwd=str(repo), capture_output=True, text=True, timeout=30,
    )
    if out.returncode != 0:
        raise _SwitchBlocked(
            "Could not prepare current app for switching.",
            (out.stderr or out.stdout or f"git {' '.join(args)} failed").strip(),
        )


# --- Gap endpoints ------------------------------------------------------------

_VALID_PRIORITIES = ("low", "medium", "high")
_VALID_STATUSES = (
    "backlog", "todo", "in-progress", "qa", "ready-merge", "awaiting-rebuild",
    "review", "done", "failed", "cancelled",
)
_USER_STATUS_TRANSITIONS = {
    "backlog": {"todo"},
    "todo": {"backlog"},
    "review": {"todo"},
    "done": {"review"},
    "failed": {"todo"},
    "cancelled": {"todo"},
}
_BULK_STATUS_AUTOMATED_VALUES = {"in-progress", "qa", "ready-merge"}
_BULK_STATUS_VALUES = set(_VALID_STATUSES) - _BULK_STATUS_AUTOMATED_VALUES
_BULK_STATUS_SOURCE_VALUES = _BULK_STATUS_VALUES
_BULK_LAST_WORKFLOW_STATUS = "__last_workflow_state"
_DUPLICATE_DECISION_IGNORE = "duplicate"
_DUPLICATE_DECISION_IMPORT = "original"
_DUPLICATE_DECISION_MOVE_ORIGINAL = "move_original_to_backlog"
_DUPLICATE_UPDATE_PREFIX = "update_original_"
_DUPLICATE_UPDATE_FIELDS = {"actual", "target", "reporter", "priority"}
_DUPLICATE_BACKLOG_PROTECTED_STATUSES = {
    "todo",
    "in-progress",
    "qa",
    "ready-merge",
    "awaiting-rebuild",
    "awaiting-review",
}

# Map a public sort key to a SQL expression. Whitelisted to prevent SQL
# injection from the query string. `id` doubles as a chronological sort
# because we mint Gap ids as ULIDs.
_GAPS_SORT_EXPRESSIONS: dict[str, str] = {
    "name":     "name COLLATE NOCASE",
    "status":   "status",
    "priority": "CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 ELSE 2 END",
    "reporter": "reporter COLLATE NOCASE",
    "instance": "instance_id COLLATE NOCASE",
    "updated":  "updated",
    "created":  "created",
    "id":       "id",
}
# Default direction per column when one isn't supplied.
_GAPS_DEFAULT_DIR: dict[str, str] = {
    "name":     "ASC",
    "status":   "ASC",
    "priority": "ASC",   # CASE maps high=0, so ASC = high first
    "reporter": "ASC",
    "instance": "ASC",
    "updated":  "DESC",
    "created":  "DESC",
    "id":       "DESC",
}


def _gaps_order_clause(sort: str | None, direction: str | None) -> str:
    key = (sort or "updated").lower()
    if key not in _GAPS_SORT_EXPRESSIONS:
        key = "updated"
    expr = _GAPS_SORT_EXPRESSIONS[key]
    d = (direction or "").upper()
    if d not in ("ASC", "DESC"):
        d = _GAPS_DEFAULT_DIR[key]
    # Tiebreaker by updated so the order is deterministic when the primary
    # key is equal across rows.
    tiebreaker = "" if key == "updated" else ", updated DESC"
    return f"{expr} {d}{tiebreaker}"


def _validate_user_status_transition(previous: str | None,
                                     next_status: str) -> tuple[int, dict] | None:
    if previous == next_status:
        return None
    allowed = _USER_STATUS_TRANSITIONS.get(previous or "", set())
    if next_status in allowed:
        return None
    return err(
        409,
        (
            f"Invalid workflow transition: {previous or 'unknown'} → {next_status}. "
            "Use the dedicated workflow action for system-owned states."
        ),
    )


def _page_bounds(limit: int, offset: int = 0) -> tuple[int, int]:
    return max(1, int(limit)), max(0, int(offset))


def list_gaps(*, status: str | None = None, q: str | None = None,
              severity: str | None = None,
              category: str | None = None,
              actor: str | None = None,
              reporter: str | None = None,
              instance: str | None = None,
              limit: int = 50,
              offset: int = 0,
              sort: str | None = None,
              direction: str | None = None,
              include_facets: bool = False) -> tuple[int, dict]:
    """List Gaps. `severity` / `category` / `actor` filter to Gaps that
    have at least one activity entry matching. `reporter` filters by
    the indexed `gaps_index.reporter` column, which the runner keeps in
    sync with the latest round's reporter on every write.
    """
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    metric_start = perf_metrics.now()
    page_limit, page_offset = _page_bounds(limit, offset)
    fts_match = search_index.fts_query(q)
    sql = [
        "SELECT id, name, status, priority, reporter, "
        "created, updated, branch_name, instance_id "
        "FROM gaps_index"
    ]
    args: list[Any] = []
    where: list[str] = []
    if status:
        where.append("status = ?")
        args.append(status)
    if q:
        if fts_match is None:
            return 200, {
                "gaps": [],
                "page": {
                    "limit": page_limit,
                    "offset": page_offset,
                    "has_more": False,
                },
            }
        where.append(
            "id IN ("
            "SELECT gap_id FROM gap_search_docs "
            "WHERE rowid IN ("
            "SELECT rowid FROM gap_search_fts "
            "WHERE gap_search_fts MATCH ?"
            "))"
        )
        args.append(fts_match)
    if reporter:
        where.append("reporter = ?")
        args.append(reporter)
    if instance:
        if instance == "current":
            where.append("instance_id = ?")
            args.append(project_state.active_instance_id())
        elif instance == "unknown":
            known = [i.get("id") for i in project_state.list_instances()]
            if known:
                where.append(
                    "(instance_id = '' OR instance_id NOT IN ("
                    + ",".join("?" * len(known)) + "))"
                )
                args.extend(known)
            else:
                where.append("1 = 1")
        elif instance != "all":
            where.append("instance_id = ?")
            args.append(instance)
    # Activity-derived filters: gap must have at least one matching entry.
    if severity or category or actor:
        sub_where = ["gap_id IS NOT NULL"]
        sub_args: list[Any] = []
        if severity:
            sub_where.append("severity = ?")
            sub_args.append(severity)
        if category:
            sub_where.append("category = ?")
            sub_args.append(category)
        if actor:
            sub_where.append("actor = ?")
            sub_args.append(actor)
        where.append(
            "id IN (SELECT DISTINCT gap_id FROM activity WHERE "
            + " AND ".join(sub_where) + ")"
        )
        args.extend(sub_args)
    if where:
        sql.append("WHERE " + " AND ".join(where))
    sql.append("ORDER BY " + _gaps_order_clause(sort, direction))
    sql.append("LIMIT ? OFFSET ?")
    args.extend([page_limit + 1, page_offset])
    conn = _conn()
    try:
        rows = [_enrich_gap_row(dict(r)) for r in conn.execute(" ".join(sql), args)]
        facets: dict | None = None
        if include_facets:
            facets = {
                "categories": activity.distinct_categories(conn),
                "actors": activity.distinct_actors(conn),
            }
    finally:
        conn.close()
    rows_scanned = len(rows)
    has_more = len(rows) > page_limit
    rows = rows[:page_limit]
    body: dict = {
        "gaps": rows,
        "page": {
            "limit": page_limit,
            "offset": page_offset,
            "has_more": has_more,
        },
    }
    if facets is not None:
        body["facets"] = facets
    perf_metrics.record(
        "api.list_gaps",
        elapsed_ms=perf_metrics.elapsed_ms(metric_start),
        query_mode="search_index" if q else "indexed",
        rows_scanned=rows_scanned,
        rows_returned=len(rows),
        details={
            "status": status or "",
            "q": bool(q),
            "severity": severity or "",
            "category": category or "",
            "actor": actor or "",
            "reporter": reporter or "",
            "instance": instance or "",
            "limit": page_limit,
            "offset": page_offset,
            "sort": sort or "",
            "direction": direction or "",
        },
    )
    return 200, body


def _select_bulk_update_candidates(
    filt: dict[str, Any],
    excluded: set[str],
    *,
    skip_automated: bool,
    selected_ids: list[str] | None = None,
) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    conn = _conn()
    try:
        if selected_ids is not None:
            if not selected_ids:
                return 200, {"gaps": [], "skipped_details": []}
            found: dict[str, dict[str, Any]] = {}
            for chunk in _id_chunks(selected_ids):
                placeholders = ",".join("?" * len(chunk))
                rows = conn.execute(
                    "SELECT id, name, status, priority, reporter, "
                    "created, updated, branch_name, instance_id "
                    f"FROM gaps_index WHERE id IN ({placeholders})",
                    chunk,
                ).fetchall()
                for row in rows:
                    found[row["id"]] = dict(row)
            rows = [found[gap_id] for gap_id in selected_ids if gap_id in found]
            return _filter_bulk_candidate_rows(
                rows,
                skip_automated=skip_automated,
            )
    finally:
        conn.close()
    q = str(filt.get("q") or "").strip()
    fts_match = search_index.fts_query(q)
    severity = filt.get("severity") or None
    category = filt.get("category") or None
    actor = filt.get("actor") or None
    reporter = filt.get("reporter") or None
    sql = [
        "SELECT id, name, status, priority, reporter, "
        "created, updated, branch_name, instance_id "
        "FROM gaps_index"
    ]
    args: list[Any] = []
    where: list[str] = []
    status = filt.get("status") or None
    if status:
        where.append("status = ?")
        args.append(status)
    if q:
        if fts_match is None:
            return 200, {"gaps": [], "skipped_details": []}
        where.append(
            "id IN ("
            "SELECT gap_id FROM gap_search_docs "
            "WHERE rowid IN ("
            "SELECT rowid FROM gap_search_fts "
            "WHERE gap_search_fts MATCH ?"
            "))"
        )
        args.append(fts_match)
    if reporter:
        where.append("reporter = ?")
        args.append(reporter)
    instance = filt.get("instance") or None
    if instance:
        if instance == "current":
            where.append("instance_id = ?")
            args.append(project_state.active_instance_id())
        elif instance == "unknown":
            known = [i.get("id") for i in project_state.list_instances()]
            if known:
                where.append(
                    "(instance_id = '' OR instance_id NOT IN ("
                    + ",".join("?" * len(known)) + "))"
                )
                args.extend(known)
            else:
                where.append("1 = 1")
        elif instance != "all":
            where.append("instance_id = ?")
            args.append(instance)
    if severity or category or actor:
        sub_where = ["gap_id IS NOT NULL"]
        sub_args: list[Any] = []
        if severity:
            sub_where.append("severity = ?")
            sub_args.append(severity)
        if category:
            sub_where.append("category = ?")
            sub_args.append(category)
        if actor:
            sub_where.append("actor = ?")
            sub_args.append(actor)
        where.append(
            "id IN (SELECT DISTINCT gap_id FROM activity WHERE "
            + " AND ".join(sub_where) + ")"
        )
        args.extend(sub_args)
    if where:
        sql.append("WHERE " + " AND ".join(where))
    sql.append("ORDER BY " + _gaps_order_clause(None, None))
    conn = _conn()
    try:
        rows = [dict(r) for r in conn.execute(" ".join(sql), args)]
    finally:
        conn.close()
    rows = [r for r in rows if r["id"] not in excluded]
    return _filter_bulk_candidate_rows(rows, skip_automated=skip_automated)


def _filter_bulk_candidate_rows(
    rows: list[dict[str, Any]],
    *,
    skip_automated: bool,
) -> tuple[int, dict]:
    skipped: list[dict[str, str]] = []
    if skip_automated:
        status_order = {status: idx for idx, status in enumerate(_VALID_STATUSES)}
        skipped = [
            {"id": r["id"], "reason": f"status:{r.get('status')}"}
            for r in rows
            if str(r.get("status") or "") in _BULK_STATUS_AUTOMATED_VALUES
        ]
        skipped.sort(key=lambda item: (
            status_order.get(item["reason"].split(":", 1)[1], 999),
            item["id"],
        ))
        rows = [
            r for r in rows
            if str(r.get("status") or "") in _BULK_STATUS_SOURCE_VALUES
        ]
    return 200, {"gaps": rows, "skipped_details": skipped}


def get_gap(gap_id: str) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    metric_start = perf_metrics.now()
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT id, name, status, priority, created, updated, branch_name, instance_id "
            "FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if not row:
            return err(404, "Gap not found")
    finally:
        conn.close()
    gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {
        "id": gap_id, "name": row["name"], "rounds": [],
        "created": row["created"], "updated": row["updated"],
    }
    gap.pop("_refine_embedded_round_logs", None)
    # SQLite is the source of truth for `status` and `priority` — overlay
    # them onto the response.
    gap = dict(gap)
    gap["status"] = row["status"]
    gap["priority"] = row["priority"] or "low"
    gap["branch_name"] = row["branch_name"]
    gap["instance_id"] = row["instance_id"]
    gap["instance_display_name"] = project_state.gap_instance_display(row["instance_id"])
    rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
    log_counts = round_logs.count_by_round(gap_id, len(rounds))
    for idx, round_obj in enumerate(rounds):
        round_obj["log_count"] = log_counts.get(idx, 0)
        latest_log, latest_error_log = round_logs.latest_for_round(gap_id, idx)
        latest_workflow_log = round_logs.latest_workflow_for_round(gap_id, idx)
        if latest_log:
            round_obj["latest_log"] = _compact_log(latest_log)
        if latest_error_log:
            round_obj["latest_error_log"] = _compact_log(latest_error_log)
        if latest_workflow_log:
            round_obj["latest_workflow_log"] = _compact_log(latest_workflow_log)
    log_count = sum(log_counts.values())
    gap["rounds"] = rounds
    perf_metrics.record(
        "api.get_gap",
        elapsed_ms=perf_metrics.elapsed_ms(metric_start),
        gap_id=gap_id,
        rows_returned=1,
        details={
            "round_count": len(rounds),
            "log_count": log_count,
        },
    )
    return 200, {"gap": gap}


def _compact_log(log: dict[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(log, dict):
        return None
    out: dict[str, Any] = {}
    for key in ("id", "datetime", "severity", "category", "message", "actor", "gap_id"):
        if key in log and log[key] is not None:
            out[key] = log[key]
    return out


def _round_metadata(round_obj: dict[str, Any]) -> dict[str, Any]:
    meta = dict(round_obj)
    logs = meta.pop("logs", [])
    if not isinstance(logs, list):
        logs = []
    meta["log_count"] = len(logs)
    if logs:
        meta["latest_log"] = _compact_log(logs[-1])
        for log in reversed(logs):
            if isinstance(log, dict) and log.get("severity") == "error":
                meta["latest_error_log"] = _compact_log(log)
                break
    return meta


def get_gap_logs(
    gap_id: str,
    *,
    round_idx: int,
    limit: int = 50,
    offset: int = 0,
) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    limit = max(1, min(int(limit), 200))
    offset = max(0, int(offset))
    metric_start = perf_metrics.now()
    page_limit, page_offset = _page_bounds(limit, offset)
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT id FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if not row:
            return err(404, "Gap not found")
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            return err(404, "Gap not found")
        rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
        if round_idx < 0 or round_idx >= len(rounds):
            return err(404, "Round not found")
        round_log_count = round_logs.count_by_round(gap_id, len(rounds)).get(round_idx, 0)
        entries, has_more = round_logs.page_round_logs(
            gap_id,
            round_idx,
            limit=page_offset + page_limit,
            offset=0,
        )
        activity_logs = _activity_for_round(conn, gap_id, rounds, round_idx)
    finally:
        conn.close()

    merged = [
        *_mark_log_source(entries, "round"),
        *_mark_log_source(activity_logs, "activity"),
    ]
    merged.sort(key=lambda log: (str(log.get("datetime") or ""), str(log.get("id") or "")))
    total = round_log_count + len(activity_logs)
    page = merged[page_offset:page_offset + page_limit]
    perf_metrics.record(
        "api.get_gap_logs",
        elapsed_ms=perf_metrics.elapsed_ms(metric_start),
        gap_id=gap_id,
        rows_returned=len(page),
        details={
            "round_idx": round_idx,
            "limit": page_limit,
            "offset": page_offset,
            "total": total,
            "round_log_count": round_log_count,
            "activity_count": len(activity_logs),
        },
    )
    return 200, {
        "gap_id": gap_id,
        "round_idx": round_idx,
        "logs": page,
        "pagination": {
            "limit": page_limit,
            "offset": page_offset,
            "total": total,
            "has_more": page_offset + len(page) < total or has_more,
        },
        "round_log_count": round_log_count,
        "activity_count": len(activity_logs),
    }


def _mark_log_source(logs: list[dict[str, Any]], source: str) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for log in logs:
        item = dict(log)
        item.setdefault("source", source)
        out.append(item)
    return out


def _activity_for_round(
    conn: sqlite3.Connection,
    gap_id: str,
    rounds: list[dict[str, Any]],
    round_idx: int,
) -> list[dict[str, Any]]:
    current = rounds[round_idx]
    lower = str(current.get("created") or "")
    upper = ""
    for later in rounds[round_idx + 1:]:
        upper = str(later.get("created") or "")
        if upper:
            break
    sql = [
        "SELECT id, datetime, severity, category, gap_id, actor, message, "
        "       details, actions_json FROM activity WHERE gap_id = ?"
    ]
    args: list[Any] = [gap_id]
    if lower:
        sql.append("AND datetime >= ?")
        args.append(lower)
    if upper:
        sql.append("AND datetime < ?")
        args.append(upper)
    sql.append("ORDER BY datetime ASC, id ASC")
    return [activity._row_to_entry(row) for row in conn.execute(" ".join(sql), args)]


def _enrich_gap_row(row: dict[str, Any]) -> dict[str, Any]:
    row["instance_display_name"] = project_state.gap_instance_display(
        row.get("instance_id"),
    )
    return row


_VALID_REPORTER = re.compile(r"^[^\x00-\x1f]{1,80}$")


@_exclusive_mutation(
    "Create Gap",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def create_gap(body: dict) -> tuple[int, dict]:
    reporter = (body.get("reporter") or "").strip()
    actual = (body.get("actual") or "").strip()
    target = (body.get("target") or "").strip()
    name = (body.get("name") or "").strip() or _autoname(actual, target)
    priority = (body.get("priority") or "low").strip().lower()
    duplicate_decision = str(body.get("duplicate_decision") or "").strip()
    if priority not in _VALID_PRIORITIES:
        return err(400, "priority must be one of low/medium/high")
    if not reporter:
        return err(400, "reporter is required")
    if not actual and not target:
        return err(400, "actual or target must be non-empty")
    if not _VALID_REPORTER.match(reporter):
        return err(400, "invalid reporter name")
    duplicate = _find_import_duplicate(actual, target)
    if duplicate and duplicate_decision == _DUPLICATE_DECISION_IGNORE:
        return 200, {
            "ok": True,
            "created": False,
            "duplicate_action": "ignored",
            "duplicate": duplicate,
        }
    if duplicate and duplicate_decision == _DUPLICATE_DECISION_MOVE_ORIGINAL:
        move = _move_duplicate_original_to_backlog(duplicate["match"]["id"])
        return 200, {
            "ok": True,
            "created": False,
            "duplicate_action": "move_original_to_backlog",
            "duplicate": duplicate,
            "move": move,
        }
    if duplicate and duplicate_decision != _DUPLICATE_DECISION_IMPORT:
        return 409, {
            "error": {
                "message": "Possible duplicate Gap found",
                "code": "duplicate_gap",
                "duplicate": duplicate,
            }
        }
    gap_id = new_ulid()
    try:
        result = get_client().call(M_CREATE_GAP, {
            "gap_id": gap_id, "name": name, "priority": priority,
            "reporter": reporter, "actual": actual, "target": target,
        })
    except BackendError as e:
        return _backend_err(e)
    return 201, result


def _autoname(actual: str, target: str) -> str:
    """Cheap, deterministic name from the first sentence of target (or actual)."""
    text = (target or actual or "Untitled Gap").strip()
    text = text.split("\n", 1)[0]
    # first sentence-ish
    m = re.split(r"[.!?]", text, maxsplit=1)
    short = (m[0] if m else text).strip()
    if len(short) > 80:
        short = short[:77].rstrip() + "..."
    return short or "Untitled Gap"


@_exclusive_mutation(
    "Update Gap",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def update_gap_name(gap_id: str, body: dict) -> tuple[int, dict]:
    """PATCH handler: accepts `name`, `priority`, and/or `notes`.

    Name and priority are SQLite-only fields — we write the index row
    directly and nudge gap.json so its mtime matches. Notes live in
    gap.json (gap-level metadata that should travel with the file), so
    we route those writes through the runner via M_SET_NOTES.
    """
    sql_fields: dict[str, str] = {}
    if "name" in body:
        new_name = (body.get("name") or "").strip()
        if not new_name:
            return err(400, "name is required")
        sql_fields["name"] = new_name
    if "priority" in body:
        p = (body.get("priority") or "").strip().lower()
        if p not in _VALID_PRIORITIES:
            return err(400, "priority must be one of low/medium/high")
        sql_fields["priority"] = p
    if "status" in body:
        # Per-Gap status updates power the workflow back/forward buttons
        # on the detail page. The transitions are bookkeeping-only — they
        # don't directly touch worktrees. The runner is nudged after the
        # write so priority gates and `todo` pickup are enforced promptly.
        s = (body.get("status") or "").strip().lower()
        if s not in _VALID_STATUSES:
            return err(400, "invalid status")
        sql_fields["status"] = s
    notes_change = "notes" in body
    if not sql_fields and not notes_change:
        return err(400, "expected `name`, `priority`, `status`, and/or `notes`")
    conn = _conn()
    try:
        row, ownership_err = _require_active_gap(conn, gap_id)
    finally:
        conn.close()
    if ownership_err is not None:
        return ownership_err
    previous_status = row["status"] if row is not None else None
    next_status = sql_fields.get("status")
    if next_status is not None:
        transition_err = _validate_user_status_transition(
            previous_status,
            next_status,
        )
        if transition_err is not None:
            return transition_err
    paused_after_update = False
    if sql_fields:
        active = project_state.active_instance_id()
        updated_at = now_iso()
        set_clause = ", ".join(f"{k} = ?" for k in sql_fields) + ", updated = ?"
        args = list(sql_fields.values()) + [updated_at, gap_id, active]
        conn = _conn()
        try:
            with db.transaction(conn):
                cur = conn.execute(
                    f"UPDATE gaps_index SET {set_clause} "
                    "WHERE id = ? AND instance_id = ?",
                    args,
                )
                paused_after_update = _agents_paused(conn)
        finally:
            conn.close()
        if not cur.rowcount:
            return _ownership_error(None, active_id=active)
        try:
            from refine_server import gap_writer

            gap = gap_writer.update_fields(gap_id, **sql_fields)
            conn = _conn()
            try:
                with db.transaction(conn):
                    search_index.upsert_gap(conn, gap)
                    if "name" in sql_fields:
                        conn.execute(
                            "DELETE FROM guidance_decisions WHERE gap_id = ?",
                            (gap_id,),
                        )
            finally:
                conn.close()
            next_status = sql_fields.get("status")
            if next_status is not None and previous_status != next_status:
                _append_gap_workflow_log(
                    gap_id,
                    f"Workflow status changed: {previous_status} → {next_status}",
                )
        except Exception:
            pass
    if notes_change:
        notes = body.get("notes")
        if not isinstance(notes, list):
            return err(400, "notes must be a list of {id, author, body, ...} objects")
        try:
            get_client().call(M_SET_NOTES, {"gap_id": gap_id, "notes": notes})
        except BackendError as e:
            return _backend_err(e)
    elif sql_fields:
        # nudge gap.json's mtime to match the index update.
        try:
            get_client().call(M_EDIT_ROUND, {
                "gap_id": gap_id, "actual": None, "target": None, "reporter": None,
            })
        except BackendError:
            pass
    if ("priority" in sql_fields or "status" in sql_fields) and not paused_after_update:
        try:
            get_client().call(M_ENFORCE_SCHEDULING, {}, timeout=10.0)
        except BackendError:
            pass
    return 200, {"ok": True}


@_exclusive_mutation(
    "Delete Gap",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def delete_gap(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        _, ownership_err = _require_active_gap(conn, gap_id)
    finally:
        conn.close()
    if ownership_err is not None:
        return ownership_err
    try:
        result = get_client().call(M_DELETE_GAP, {"gap_id": gap_id})
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def bulk_update_gaps(body: dict) -> tuple[int, dict]:
    allow_active_kinds = (
        {"merge_agent"} if _is_last_workflow_bulk_update(body) else None
    )
    try:
        with background_jobs.exclusive_operation(
            "Bulk update Gaps",
            allow_active_kinds=allow_active_kinds,
        ):
            return _bulk_update_gaps_impl(body)
    except background_jobs.BackgroundJobConflict as e:
        return _background_job_conflict_response(e)


def _bulk_update_gaps_impl(body: dict) -> tuple[int, dict]:
    """Apply a single field update to every Gap matching the supplied filter.

    Body shape:

        {"filter": {"status": "...", "q": "..."},
         "update": {"priority": "high"} | {"status": "cancelled"} | {"reporter": "alice"}}

    Exactly one update key is honored per call so the action is unambiguous
    to confirm in the UI. `priority` and `status` are SQL-index fields and
    are selected in the UI process, then applied by one runner protocol
    request that carries the ordered Gap ids. Status changes here are
    bookkeeping-only except for the special `__last_workflow_state`
    status operation, which asks the runner to reopen safe retry queues
    (`todo` or `ready-merge`) from each Gap's latest workflow history.
    """
    allow_active_kinds = (
        {"merge_agent"} if _is_last_workflow_bulk_update(body) else None
    )
    update = body.get("update") or {}
    update = {k: v for k, v in update.items()
              if k in ("priority", "status", "reporter")}
    if len(update) != 1:
        return err(400,
                   "update must contain exactly one of "
                   "`priority`, `status`, or `reporter`")
    field, raw = next(iter(update.items()))
    value = (raw or "").strip()
    if field == "priority":
        value = value.lower()
        if value not in _VALID_PRIORITIES:
            return err(400, "priority must be one of low/medium/high")
    elif field == "status":
        value = value.lower()
        if value == _BULK_LAST_WORKFLOW_STATUS:
            pass
        elif value not in _VALID_STATUSES:
            return err(400, "invalid status")
        elif value not in _BULK_STATUS_VALUES:
            return err(
                409,
                (
                    "Bulk status updates cannot set in-progress, qa, or ready-merge. "
                    "Use per-Gap workflow actions for automated states."
                ),
            )
    else:  # reporter
        if not value or not _VALID_REPORTER.match(value):
            return err(400, "invalid reporter name")

    filt = body.get("filter") or {}
    excluded = set(body.get("exclude_ids") or [])
    selected_ids = _selected_gap_ids(body)
    code, selected = _select_bulk_update_candidates(
        filt,
        excluded,
        skip_automated=(
            field == "status" and value != _BULK_LAST_WORKFLOW_STATUS
        ),
        selected_ids=selected_ids,
    )
    if code != 200:
        return code, selected
    selected_gaps = selected["gaps"]
    skipped_status_ids = selected["skipped_details"]
    gap_ids = [g["id"] for g in selected_gaps]
    if not gap_ids:
        return 200, {
            "updated": 0,
            "ids": [],
            "skipped": len(skipped_status_ids),
            "skipped_details": skipped_status_ids,
        }
    ok, ownership_err = _require_active_gap_ids(gap_ids)
    if not ok and ownership_err is not None:
        return ownership_err

    if (
        len(gap_ids) >= BULK_UPDATE_BACKGROUND_THRESHOLD
        and body.get("background") is not False
    ):
        stopped = _background_processes_stopped_response()
        if stopped is not None:
            return stopped
        job_data = json.loads(json.dumps({
            "field": field,
            "value": value,
            "gaps": selected_gaps,
            "skipped_details": skipped_status_ids,
        }))

        def run_job(progress=None) -> dict[str, Any]:
            if progress is not None:
                progress(
                    completed=0,
                    total=len(job_data["gaps"]),
                    message="Bulk update queued",
                )
            result = _bulk_update_selected_gaps(
                job_data["field"],
                job_data["value"],
                job_data["gaps"],
                job_data["skipped_details"],
            )
            if progress is not None:
                runner_progress = result.get("progress") or {}
                progress(
                    completed=runner_progress.get("completed", result["updated"]),
                    total=runner_progress.get("total", len(job_data["gaps"])),
                    message="Bulk update complete",
                )
            return {"http_status": 200, **result}

        try:
            job = background_jobs.start(
                "bulk_update_gaps",
                f"Bulk update {len(gap_ids)} Gaps",
                run_job,
                allow_active_kinds=allow_active_kinds,
            )
        except background_jobs.BackgroundJobConflict as e:
            return _background_job_conflict_response(e)
        return 202, {
            "queued": True,
            "job": job,
            "matched": len(gap_ids),
            "skipped": len(skipped_status_ids),
            "skipped_details": skipped_status_ids,
        }

    result = _bulk_update_selected_gaps(
        field,
        value,
        selected_gaps,
        skipped_status_ids,
    )
    if int(result.get("http_status") or 200) >= 400:
        return int(result["http_status"]), result
    return 200, result


def _is_last_workflow_bulk_update(body: dict) -> bool:
    update = body.get("update") or {}
    raw = update.get("status")
    return (
        isinstance(raw, str)
        and raw.strip().lower() == _BULK_LAST_WORKFLOW_STATUS
    )


def _bulk_update_selected_gaps(
    field: str,
    value: str,
    selected_gaps: list[dict[str, Any]],
    skipped_status_ids: list[dict[str, str]],
) -> dict:
    gap_ids = [g["id"] for g in selected_gaps]
    try:
        result = get_client().call(
            M_BULK_UPDATE_GAPS,
            {"field": field, "value": value, "gap_ids": gap_ids},
            timeout=max(30.0, min(300.0, len(gap_ids) / 10)),
        )
    except BackendError as e:
        code, body = _backend_err(e)
        return {
            "updated": 0,
            "ids": [],
            "field": field,
            "value": value,
            "skipped": len(skipped_status_ids),
            "skipped_details": skipped_status_ids,
            "failed": len(gap_ids),
            "failures": [{"id": gid, "error": body["error"]["message"]} for gid in gap_ids],
            "error": body["error"],
            "http_status": code,
        }
    runner_skipped_details = result.get("skipped_details") or []
    if not isinstance(runner_skipped_details, list):
        runner_skipped_details = []
    all_skipped_details = [*skipped_status_ids, *runner_skipped_details]
    return {
        "updated": int(result.get("updated") or 0),
        "ids": result.get("ids") or [],
        "field": field, "value": value,
        "skipped": len(all_skipped_details),
        "skipped_details": all_skipped_details,
        "failed": int(result.get("failed") or 0),
        "failures": result.get("failures") or [],
        "todo": int(result.get("todo") or 0),
        "ready_merge": int(result.get("ready_merge") or 0),
        "progress": result.get("progress") or {
            "completed": int(result.get("updated") or 0),
            "total": len(gap_ids),
        },
    }


@_exclusive_mutation("Bulk delete Gaps")
def bulk_delete_gaps(body: dict) -> tuple[int, dict]:
    """Delete every Gap matching the supplied filter.

    One runner request carries the ordered id list. The runner then cancels
    any running subprocess, tears down worktrees + branches for non-done
    gaps, erases gap.json, and cleans the index in runner-owned order.
    Per-Gap failures don't abort the run — we collect them in the response.
    """
    filt = body.get("filter") or {}
    excluded = set(body.get("exclude_ids") or [])
    selected_ids = _selected_gap_ids(body)
    code, selected = _select_bulk_update_candidates(
        filt,
        excluded,
        skip_automated=False,
        selected_ids=selected_ids,
    )
    if code != 200:
        return code, selected
    gap_ids = [g["id"] for g in selected["gaps"]]
    if not gap_ids:
        return 200, {"deleted": 0, "ids": [], "failures": []}
    ok, ownership_err = _require_active_gap_ids(gap_ids)
    if not ok and ownership_err is not None:
        return ownership_err

    try:
        result = get_client().call(
            M_BULK_DELETE_GAPS,
            {"gap_ids": gap_ids},
            timeout=max(60.0, min(300.0, len(gap_ids) / 5)),
        )
    except BackendError as e:
        code, body = _backend_err(e)
        return code, {
            "deleted": 0,
            "ids": [],
            "failures": [{"id": gid, "error": body["error"]["message"]} for gid in gap_ids],
            "error": body["error"],
        }
    return 200, {
        "deleted": int(result.get("deleted") or 0),
        "ids": result.get("ids") or [],
        "failures": result.get("failures") or [],
        "failed": int(result.get("failed") or 0),
        "progress": result.get("progress") or {
            "completed": int(result.get("deleted") or 0),
            "total": len(gap_ids),
        },
    }


@_exclusive_mutation(
    "Append Gap round",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def append_round(gap_id: str, body: dict) -> tuple[int, dict]:
    reporter = (body.get("reporter") or "").strip()
    actual = (body.get("actual") or "").strip()
    target = (body.get("target") or "").strip()
    if not reporter:
        return err(400, "reporter is required")
    if not actual and not target:
        return err(400, "actual or target must be non-empty")
    # Guard: only allowed from review or failed (or todo, treated as edit of latest).
    conn = _conn()
    try:
        row, ownership_err = _require_active_gap(
            conn, gap_id, columns="status, instance_id",
        )
    finally:
        conn.close()
    if ownership_err is not None:
        return ownership_err
    if row["status"] != "review":
        return err(
            409,
            "New rounds may only be appended from `review` "
            f"(status={row['status']}). From `todo` or `failed`, edit the "
            "latest round instead."
        )
    try:
        result = get_client().call(M_APPEND_ROUND, {
            "gap_id": gap_id, "reporter": reporter,
            "actual": actual, "target": target,
        })
    except BackendError as e:
        return _backend_err(e)
    return 201, result


@_exclusive_mutation(
    "Edit Gap round",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def edit_latest_round(gap_id: str, body: dict) -> tuple[int, dict]:
    conn = _conn()
    try:
        row, ownership_err = _require_active_gap(
            conn, gap_id, columns="status, instance_id",
        )
    finally:
        conn.close()
    if ownership_err is not None:
        return ownership_err
    if row["status"] not in ("backlog", "todo", "failed"):
        return err(409, "Only the latest unaddressed round can be edited "
                        f"(status={row['status']})")
    try:
        result = get_client().call(M_EDIT_ROUND, {
            "gap_id": gap_id,
            "actual": body.get("actual"),
            "target": body.get("target"),
            "reporter": body.get("reporter"),
        })
    except BackendError as e:
        return _backend_err(e)
    return 200, result


@_exclusive_mutation("Verify Gap")
def verify(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        _, ownership_err = _require_active_gap(conn, gap_id)
    finally:
        conn.close()
    if ownership_err is not None:
        return ownership_err
    try:
        result = get_client().call(M_VERIFY, {"gap_id": gap_id}, timeout=120.0)
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def list_changes(*, limit: int = 50, offset: int = 0,
                 q: str | None = None, status: str | None = None,
                 priority: str | None = None) -> tuple[int, dict]:
    """List refine merge commits on the target branch (plus the Gap
    metadata for each). Used by the Changes screen."""
    page_limit, page_offset = _page_bounds(limit, offset)
    try:
        result = get_client().call(
            M_LIST_CHANGES,
            {
                "limit": page_limit,
                "offset": page_offset,
                "q": q or "",
                "status": status or "",
                "priority": priority or "",
            },
            timeout=15.0,
        )
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def undo_change(body: dict) -> tuple[int, dict]:
    """Revert a refine merge commit. The runner derives the Gap id from
    the commit's `Refine Gap:` trailer, switches branches if needed,
    runs `git revert -m 1`, pushes when an upstream exists, and moves
    the Gap to `cancelled` with a log entry."""
    commit = (body.get("commit") or "").strip()
    if not commit:
        return err(400, "commit is required")
    from refine_server import git_ops

    gap_id = git_ops.gap_id_from_commit(commit)
    if gap_id:
        conn = _conn()
        try:
            _, ownership_err = _require_active_gap(conn, gap_id)
        finally:
            conn.close()
        if ownership_err is not None:
            return ownership_err
    try:
        result = get_client().call(
            M_UNDO_GAP, {"commit": commit}, timeout=120.0,
        )
    except BackendError as e:
        return _backend_err(e)
    code = 200 if result.get("ok") else 409
    return code, result


def retry(gap_id: str) -> tuple[int, dict]:
    """Reopen a terminal Gap by transitioning it back to `todo` so the
    dispatcher picks it up again. Allowed from `failed`, `done`, or
    `cancelled`. (Webapp writes `status=todo` directly per the write-
    ownership split.)
    """
    conn = _conn()
    try:
        row, ownership_err = _require_active_gap(
            conn, gap_id, columns="status, instance_id",
        )
        if ownership_err is not None:
            return ownership_err
        prev_status = row["status"]
        if prev_status not in ("failed", "done", "cancelled"):
            return err(
                409,
                f"Reopen only valid from failed/done/cancelled (status={prev_status})",
            )
        # If the most recent failure was an auth issue, re-run pre-flight
        # before reopening so we don't immediately fail again.
        last = conn.execute(
            "SELECT failure_category FROM runs WHERE gap_id = ? "
            "ORDER BY id DESC LIMIT 1", (gap_id,),
        ).fetchone()
        if last and last["failure_category"] == "auth":
            try:
                pf = get_client().call(M_PREFLIGHT, {})
                if not pf.get("ok"):
                    return err(409, "Auth pre-flight still failing — Reopen blocked",
                               pf.get("message"))
            except BackendError as e:
                return _backend_err(e)
        with db.transaction(conn):
            cur = conn.execute(
                "UPDATE gaps_index SET status = 'todo', updated = ? "
                "WHERE id = ? AND instance_id = ?",
                (now_iso(), gap_id, project_state.active_instance_id()),
            )
        if not cur.rowcount:
            return _ownership_error(None)
        try:
            from refine_server import gap_writer

            gap_writer.update_fields(gap_id, status="todo")
            _append_gap_workflow_log(
                gap_id,
                f"Workflow status changed: {prev_status} → todo; reopened",
            )
        except Exception:
            pass
        activity.append(
            conn, message=f"Reopened from {prev_status} → todo",
            severity="info", category="state",
            gap_id=gap_id, actor="refine",
        )
    finally:
        conn.close()
    try:
        get_client().call(M_ENFORCE_SCHEDULING, {}, timeout=10.0)
    except BackendError:
        pass
    return 200, {"ok": True}


@_exclusive_mutation("Retry Merge", allow_active_kinds={"merge_agent"})
def retry_merge(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        _, ownership_err = _require_active_gap(conn, gap_id)
    finally:
        conn.close()
    if ownership_err is not None:
        return ownership_err
    try:
        result = get_client().call(
            M_RETRY_MERGE,
            {"gap_id": gap_id},
            timeout=10.0,
        )
    except BackendError as e:
        return _backend_err(e)
    return (200 if result.get("ok") else 409), result


@_exclusive_mutation("Retry QA")
def retry_qa(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        _, ownership_err = _require_active_gap(conn, gap_id)
    finally:
        conn.close()
    if ownership_err is not None:
        return ownership_err
    try:
        result = get_client().call(
            M_RETRY_QA,
            {"gap_id": gap_id},
            timeout=10.0,
        )
    except BackendError as e:
        return _backend_err(e)
    return (200 if result.get("ok") else 409), result


def cancel(gap_id: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        row, ownership_err = _require_active_gap(
            conn, gap_id, columns="status, instance_id",
        )
    finally:
        conn.close()
    if ownership_err is not None:
        return ownership_err
    if row["status"] in ("done", "cancelled"):
        return err(409, f"Already terminal (status={row['status']})")
    try:
        result = get_client().call(M_CANCEL, {"gap_id": gap_id})
    except BackendError as e:
        return _backend_err(e)
    return 200, result


# --- Reporters ----------------------------------------------------------------

def list_reporters() -> tuple[int, dict]:
    conn = _conn()
    try:
        return 200, {"reporters": reporters.list_all(conn)}
    finally:
        conn.close()


def create_reporter(body: dict) -> tuple[int, dict]:
    name = (body.get("name") or "").strip()
    if not name:
        return err(400, "name is required")
    conn = _conn()
    try:
        rep = reporters.add(conn, name)
    finally:
        conn.close()
    return 201, {"reporter": rep}


def rename_reporter(rid: int, body: dict) -> tuple[int, dict]:
    name = (body.get("name") or "").strip()
    if not name:
        return err(400, "name is required")
    if not _VALID_REPORTER.match(name):
        return err(400, "invalid reporter name")
    # Route through the runner so the rename cascades through every Gap's
    # `rounds[].reporter` strings — keeping the dropdown and historical
    # data in sync. (Deletes deliberately don't cascade; see delete_reporter.)
    try:
        result = get_client().call(
            M_RENAME_REPORTER, {"rid": rid, "new_name": name}, timeout=60.0,
        )
    except BackendError as e:
        return _backend_err(e)
    return 200, {"ok": True, **result}


def merge_reporter(rid: int, body: dict) -> tuple[int, dict]:
    try:
        target_rid = int(body.get("target_id"))
    except (TypeError, ValueError):
        return err(400, "target_id is required")
    if target_rid == rid:
        return err(400, "cannot merge a reporter into itself")
    try:
        result = get_client().call(
            M_MERGE_REPORTER,
            {"rid": rid, "target_rid": target_rid},
            timeout=60.0,
        )
    except BackendError as e:
        return _backend_err(e)
    return 200, {"ok": True, **result}


def delete_reporter(rid: int) -> tuple[int, dict]:
    conn = _conn()
    try:
        reporters.remove(conn, rid)
    finally:
        conn.close()
    return 200, {"ok": True}


# --- Settings -----------------------------------------------------------------

def list_settings() -> tuple[int, dict]:
    conn = _conn()
    try:
        _cleanup_legacy_target_app_settings(conn)
        return 200, {"settings": db.list_settings(conn)}
    finally:
        conn.close()


def upgrade_status() -> tuple[int, dict]:
    return 200, {"upgrade": upgrade.status(Path.cwd()).as_dict()}


@_exclusive_mutation(
    "Update settings",
    allow_busy_when=lambda _owner: _background_processes_stopped(),
)
def update_settings(body: dict) -> tuple[int, dict]:
    if not isinstance(body, dict) or not body:
        return err(400, "expected an object of {key: value}")
    allowed = {
        "parallel_run_cap", "branch_name_pattern",
        "agent_idle_timeout_seconds", "agent_hard_cap_seconds",
        "agent_limit_pause_seconds",
        "worker_memory_limit_mb", "ui_memory_limit_mb",
        "worker_cpu_priority", "resource_isolation_mode",
        "chat_idle_timeout_seconds",
        "backlog_promote_after_seconds",
        "project_update_pulse_interval_seconds",
        "file_browser_ignore_patterns",
        "agent_subpath", "merge_target_branch",
        "quality_enabled",
        "quality_timing",
        "quality_regressions_enabled",
        "agent_cli",
        "paused",
        # Target-app configuration. The state fields
        # (target_app_state etc.) are owned by the system and are
        # mutated via the /api/target-app/* endpoints, not Settings.
        "target_app_start_instructions",
        "target_app_stop_instructions",
        "target_app_health_url",
        "target_app_url",
        "target_app_start_command",
        "target_app_stop_command",
        "target_app_rebuild_command",
        "target_app_status_command",
        "target_app_cwd",
        "target_app_env_json",
        "target_app_start_timeout_seconds",
        "target_app_stop_timeout_seconds",
        "target_app_rebuild_timeout_seconds",
        "target_app_status_timeout_seconds",
        "target_app_log_path",
        "target_app_http_check_url",
        "target_app_tcp_check_host",
        "target_app_tcp_check_port",
        "target_app_process_check_command",
        "target_app_auto_rebuild",
    }
    valid_agent_clis = ("claude", "codex", "gemini", "copilot")
    normalized: dict[str, str] = {}
    for k, v in body.items():
        if k not in allowed:
            return err(400, f"unknown setting: {k}")
        if k == "merge_target_branch":
            br = str(v or "").strip()
            # Empty means "follow host's current branch". Validate format
            # only — existence is checked at the time it's used so the
            # operator can pre-configure before the branch exists.
            if br:
                if any(c.isspace() for c in br):
                    return err(400, "merge_target_branch may not contain whitespace")
                if br.startswith("-") or "\0" in br:
                    return err(400, "merge_target_branch contains an invalid character")
            normalized[k] = br
        elif k == "agent_subpath":
            sub = str(v or "").strip()
            # Reject absolute paths, `..` traversal, and any embedded NUL.
            if sub:
                if sub.startswith("/") or sub.startswith("~"):
                    return err(400, "agent_subpath must be relative to the repo root")
                if "\0" in sub:
                    return err(400, "agent_subpath contains an invalid character")
                parts = [p for p in sub.replace("\\", "/").split("/") if p]
                if any(p == ".." for p in parts):
                    return err(400, "agent_subpath must not contain `..` components")
                sub = "/".join(parts)
            normalized[k] = sub
        elif k == "agent_cli":
            choice = str(v or "").strip().lower()
            if choice not in valid_agent_clis:
                return err(
                    400,
                    f"agent_cli must be one of {', '.join(valid_agent_clis)}",
                )
            normalized[k] = choice
        elif k in {"quality_enabled", "quality_regressions_enabled"}:
            normalized[k] = (
                "1"
                if str(v).strip().lower() in {"1", "true", "yes", "on"}
                else "0"
            )
        elif k == "quality_timing":
            choice = quality.normalize_timing(v)
            if str(v or "").strip() not in quality.QUALITY_TIMING_VALUES:
                return err(
                    400,
                    "quality_timing must be one of pre_merge, post_rebuild",
                )
            normalized[k] = choice
        elif k == "parallel_run_cap":
            try:
                n = int(v)
            except (TypeError, ValueError):
                return err(400, "parallel_run_cap must be an integer")
            if n < 1 or n > 100:
                return err(400, "parallel_run_cap must be between 1 and 100")
            normalized[k] = str(n)
        elif k in {
            "worker_memory_limit_mb",
            "ui_memory_limit_mb",
            "worker_cpu_priority",
            "resource_isolation_mode",
        }:
            try:
                normalized[k] = runtime_resources.validate_setting(k, v)
            except ValueError as e:
                return err(400, str(e))
        elif k == "target_app_cwd":
            cwd = str(v or "").strip()
            if cwd and "\0" in cwd:
                return err(400, "target_app_cwd contains an invalid character")
            if cwd.startswith("~"):
                return err(400, "target_app_cwd must be absolute or relative to the repo root")
            if cwd and not cwd.startswith("/"):
                parts = [p for p in cwd.replace("\\", "/").split("/") if p]
                if any(p == ".." for p in parts):
                    return err(400, "target_app_cwd must not contain `..` components")
                cwd = "/".join(parts)
            normalized[k] = cwd
        elif k == "target_app_env_json":
            raw = str(v or "{}").strip() or "{}"
            try:
                env_obj = json.loads(raw)
            except json.JSONDecodeError:
                return err(400, "target_app_env_json must be a JSON object")
            if not isinstance(env_obj, dict):
                return err(400, "target_app_env_json must be a JSON object")
            normalized[k] = json.dumps({str(ek): str(ev) for ek, ev in env_obj.items()})
        elif k == "target_app_url":
            app_url = str(v or "").strip()
            if app_url:
                parsed = urlparse(app_url)
                if parsed.scheme not in {"http", "https"} or not parsed.netloc:
                    return err(400, "target_app_url must be an http:// or https:// URL")
            normalized[k] = app_url
        elif k in {
            "target_app_start_timeout_seconds",
            "target_app_stop_timeout_seconds",
            "target_app_rebuild_timeout_seconds",
            "target_app_status_timeout_seconds",
        }:
            try:
                n = int(v)
            except (TypeError, ValueError):
                return err(400, f"{k} must be an integer")
            if n < 1 or n > 3600:
                return err(400, f"{k} must be between 1 and 3600")
            normalized[k] = str(n)
        elif k == "target_app_auto_rebuild":
            choice = str(v or "").strip()
            allowed_modes = {"never", "on_worktree_merge", "hourly", "nightly"}
            if choice not in allowed_modes:
                return err(
                    400,
                    "target_app_auto_rebuild must be one of never, "
                    "on_worktree_merge, hourly, nightly",
                )
            normalized[k] = choice
        elif k == "target_app_tcp_check_port":
            port = str(v or "").strip()
            if port:
                try:
                    n = int(port)
                except ValueError:
                    return err(400, "target_app_tcp_check_port must be an integer")
                if n < 1 or n > 65535:
                    return err(400, "target_app_tcp_check_port must be between 1 and 65535")
                port = str(n)
            normalized[k] = port
        elif k == "backlog_promote_after_seconds":
            # -1 = never, 0 = instant, otherwise seconds. Restrict to the
            # canonical set shown in the UI so a stale client can't smuggle
            # in something weird.
            try:
                n = int(v)
            except (TypeError, ValueError):
                return err(400, "backlog_promote_after_seconds must be an integer")
            allowed_intervals = {-1, 0, 300, 1800, 3600, 10800, 21600, 86400}
            if n not in allowed_intervals:
                return err(400,
                    "backlog_promote_after_seconds must be one of "
                    "-1 (never), 0 (instant), 300, 1800, 3600, 10800, 21600, 86400")
            normalized[k] = str(n)
        elif k == "project_update_pulse_interval_seconds":
            try:
                n = int(v)
            except (TypeError, ValueError):
                return err(
                    400,
                    "project_update_pulse_interval_seconds must be an integer",
                )
            allowed_intervals = {-1, 30, 60, 300, 900, 1800, 3600}
            if n not in allowed_intervals:
                return err(
                    400,
                    "project_update_pulse_interval_seconds must be one of "
                    "-1 (never), 30, 60, 300, 900, 1800, 3600",
                )
            normalized[k] = str(n)
        elif k == "file_browser_ignore_patterns":
            raw = str(v or "")
            if "\0" in raw:
                return err(400, "file_browser_ignore_patterns contains an invalid character")
            patterns = [
                item.strip().replace("\\", "/").strip("/")
                for item in raw.split(",")
                if item.strip().strip("/")
            ]
            normalized[k] = ", ".join(patterns)
        elif k == "agent_limit_pause_seconds":
            try:
                n = int(v)
            except (TypeError, ValueError):
                return err(400, "agent_limit_pause_seconds must be an integer")
            allowed_intervals = {30, 60, 3600, 10800}
            if n not in allowed_intervals:
                return err(
                    400,
                    "agent_limit_pause_seconds must be one of 30, 60, 3600, 10800",
                )
            normalized[k] = str(n)
        else:
            normalized[k] = str(v)
    conn = _conn()
    try:
        for k, v in normalized.items():
            db.set_setting(conn, k, v)
        _cleanup_legacy_target_app_settings(conn)
        activity.append(
            conn, message=f"Settings updated: {', '.join(normalized.keys())}",
            severity="info", category="user", actor="refine",
        )
    finally:
        conn.close()
    if "paused" in normalized:
        stopped = normalized.get("paused") == "1"
        if stopped:
            _cancel_active_background_jobs()
        try:
            result = get_client().call(
                M_BACKGROUND_PROCESSES_SET,
                {"stopped": stopped, "settle_timeout_seconds": 8.0},
                timeout=30.0 if stopped else 10.0,
            )
        except BackendError as e:
            return _backend_err(e)
        if stopped and not result.get("ok", True):
            cleanup = result.get("cleanup") or {}
            return err(
                409,
                cleanup.get("message") or (
                    "background processes stopped but target worktree "
                    "cleanup did not complete"
                ),
            )
    if "quality_enabled" in normalized or "quality_timing" in normalized:
        try:
            get_client().call(M_ENFORCE_SCHEDULING, {}, timeout=10.0)
        except BackendError:
            pass
    if (
        normalized.get("target_app_auto_rebuild") == "on_worktree_merge"
        or "target_app_rebuild_command" in normalized
    ):
        try:
            get_client().call(M_TARGET_APP_REBUILD_PENDING, {}, timeout=10.0)
        except BackendError:
            pass
    return 200, {"ok": True}


def governance_get() -> tuple[int, dict]:
    conn = _conn()
    try:
        result = governance.load_settings(conn)
        result["configured"] = governance.is_configured(conn)
        return 200, result
    finally:
        conn.close()


def governance_save(body: dict) -> tuple[int, dict]:
    rules = body.get("rules")
    if rules is not None and not isinstance(rules, list):
        return err(400, "rules must be a list")
    conn = _conn()
    try:
        result = governance.save_settings(
            conn,
            product=body.get("product"),
            constitution=body.get("constitution"),
            rules=rules,
        )
        result["configured"] = governance.is_configured(conn)
        activity.append(
            conn,
            message="Governance settings updated",
            severity="info",
            category="governance",
            actor="refine",
        )
    finally:
        conn.close()
    try:
        get_client().call(M_GOVERNANCE_WAKE, {}, timeout=10.0)
    except BackendError:
        pass
    return 200, result


def quality_get() -> tuple[int, dict]:
    conn = _conn()
    try:
        result = quality.load_settings(conn)
        result["enabled"] = db.get_setting(conn, "quality_enabled", "0") or "0"
        result["timing"] = quality.timing(conn)
        result["regressions_enabled"] = (
            db.get_setting(conn, "quality_regressions_enabled", "0") or "0"
        )
        result["regressions"] = regressions.list_regressions()
        result["configured"] = quality.is_configured(conn)
        return 200, result
    finally:
        conn.close()


def quality_save(body: dict) -> tuple[int, dict]:
    if "timing" in body:
        raw_timing = str(body.get("timing") or "").strip()
        if raw_timing not in quality.QUALITY_TIMING_VALUES:
            return err(400, "timing must be one of pre_merge, post_rebuild")
    conn = _conn()
    enabled_changed = False
    timing_changed = False
    try:
        if "enabled" in body:
            enabled = (
                "1"
                if str(body.get("enabled")).strip().lower()
                in {"1", "true", "yes", "on"}
                else "0"
            )
            enabled_changed = enabled != (db.get_setting(conn, "quality_enabled", "0") or "0")
            db.set_setting(conn, "quality_enabled", enabled)
        if "timing" in body:
            timing_changed = (
                str(body.get("timing") or "").strip() != quality.timing(conn)
            )
        if "regressions_enabled" in body:
            regressions.set_enabled(conn, body.get("regressions_enabled"))
        result = quality.save_settings(
            conn,
            business_requirements=body.get("business_requirements"),
            instructions=body.get("instructions"),
            timing_value=body.get("timing"),
        )
        result["enabled"] = db.get_setting(conn, "quality_enabled", "0") or "0"
        result["timing"] = quality.timing(conn)
        result["regressions_enabled"] = (
            db.get_setting(conn, "quality_regressions_enabled", "0") or "0"
        )
        result["regressions"] = regressions.list_regressions()
        result["configured"] = quality.is_configured(conn)
        activity.append(
            conn,
            message="Quality settings updated",
            severity="info",
            category="quality",
            actor="refine",
        )
    finally:
        conn.close()
    if enabled_changed or timing_changed:
        try:
            get_client().call(M_ENFORCE_SCHEDULING, {}, timeout=10.0)
        except BackendError:
            pass
    return 200, result


def quality_regression_create(body: dict) -> tuple[int, dict]:
    title = str(body.get("title") or "").strip()
    prompt = str(body.get("prompt") or "").strip()
    description = str(body.get("description") or "").strip()
    if not title:
        title = prompt[:80].strip() or "Untitled regression"
    reg = regressions.create_regression(
        title=title,
        description=description,
        prompt=prompt,
    )
    conn = _conn()
    try:
        activity.append(
            conn,
            message=f"Regression created: {reg['title']}",
            severity="info",
            category="quality",
            actor="refine",
        )
    finally:
        conn.close()
    return 201, {"ok": True, "regression": reg}


def quality_regression_update(regression_id: str, body: dict) -> tuple[int, dict]:
    reg = regressions.update_regression(regression_id, body or {})
    if not reg:
        return err(404, "Regression not found")
    return 200, {"ok": True, "regression": reg}


def quality_regression_delete(regression_id: str) -> tuple[int, dict]:
    if not regressions.delete_regression(regression_id):
        return err(404, "Regression not found")
    return 200, {"ok": True}


def quality_regression_run(_body: dict | None = None) -> tuple[int, dict]:
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        result = get_client().call(
            M_REGRESSION_RUN,
            {"only_enabled": True},
            timeout=900.0,
        )
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def governance_generate_rules(body: dict) -> tuple[int, dict]:
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    product = str(body.get("product") or "").strip()
    constitution = str(body.get("constitution") or "").strip()
    if not product or not constitution:
        return err(400, "product and constitution are required")
    try:
        result = get_client().call(
            M_GOVERNANCE_GENERATE_RULES,
            {"product": product, "constitution": constitution},
            timeout=600.0,
        )
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def recheck_auth() -> tuple[int, dict]:
    try:
        result = get_client().call(M_PREFLIGHT, {}, timeout=30.0)
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def backend_diagnostics() -> tuple[int, dict]:
    backend = runtime.backend_info()
    try:
        result = get_client().call(M_DIAGNOSTICS, {}, timeout=5.0)
    except BackendError as e:
        return 200, {
            "reachable": False,
            "backend": backend,
            "error": {"message": e.message, "code": e.code},
        }
    result["reachable"] = True
    result["backend"] = backend
    return 200, result


# --- Activity / Dashboard -----------------------------------------------------

def list_activity(*, limit: int = 50, gap_id: str | None = None,
                  since_id: int | None = None,
                  severity: str | None = None,
                  category: str | None = None,
                  actor: str | None = None,
                  q: str | None = None,
                  offset: int = 0,
                  sort: str | None = None,
                  direction: str | None = None,
                  include_facets: bool = False) -> tuple[int, dict]:
    metric_start = perf_metrics.now()
    page_limit, page_offset = _page_bounds(limit, offset)
    conn = _conn()
    try:
        entries = activity.recent(
            conn, limit=page_limit + 1, offset=page_offset,
            gap_id=gap_id, since_id=since_id,
            severity=severity, category=category, actor=actor, q=q,
            sort=sort, direction=direction,
        )
        total = activity.count(
            conn, gap_id=gap_id, since_id=since_id,
            severity=severity, category=category, actor=actor, q=q,
        )
        has_more = len(entries) > page_limit
        body: dict = {
            "activity": entries[:page_limit],
            "page": {
                "limit": page_limit,
                "offset": page_offset,
                "has_more": has_more,
                "total": total,
            },
        }
        if include_facets:
            body["facets"] = {
                "categories": activity.distinct_categories(conn),
                "actors": activity.distinct_actors(conn),
                "severities": ["info", "warn", "error"],
            }
        perf_metrics.record(
            "api.list_activity",
            conn=conn,
            elapsed_ms=perf_metrics.elapsed_ms(metric_start),
            gap_id=gap_id,
            query_mode="filtered" if any([gap_id, since_id, severity, category, actor, q]) else "recent",
            rows_returned=len(body["activity"]),
            details={
                "limit": page_limit,
                "offset": page_offset,
                "since_id": since_id,
                "severity": severity or "",
                "category": category or "",
                "actor": actor or "",
                "q": bool(q),
                "sort": sort or "",
                "direction": direction or "",
            },
        )
    finally:
        conn.close()
    return 200, body


def record_ui_error(body: dict) -> tuple[int, dict]:
    message = str(body.get("message") or "UI error").strip()[:1000]
    details = body.get("details")
    detail_lines = []
    if details:
        detail_lines.append(str(details)[:4000])
    meta = {
        key: body.get(key)
        for key in ("route", "path", "status", "code", "source")
        if body.get(key) not in (None, "")
    }
    if meta:
        detail_lines.append(json.dumps(meta, sort_keys=True))
    try:
        conn = _conn(ensure_cache=False)
        try:
            activity.append(
                conn,
                message=message,
                severity="error",
                category="ui",
                actor="browser",
                details="\n\n".join(detail_lines) if detail_lines else None,
            )
        finally:
            conn.close()
    except Exception:
        return 200, {"ok": False}
    return 200, {"ok": True}


_LOG_RETENTION_OPTIONS = (0, 7, 30, 60, 90, 365)


def cleanup_logs(body: dict) -> tuple[int, dict]:
    """Delete activity entries older than `days` days.

    `days == 0` deletes the whole activity table (operator chose
    "don't keep any"). Anything else uses an ISO-timestamp cutoff
    computed against `now`. Returns the number of rows deleted.
    """
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    raw = body.get("days")
    try:
        days = int(raw)
    except (TypeError, ValueError):
        return err(400, "days must be an integer")
    if days not in _LOG_RETENTION_OPTIONS:
        return err(
            400,
            f"days must be one of {sorted(_LOG_RETENTION_OPTIONS)}",
        )
    conn = _conn()
    try:
        if days == 0:
            cur = conn.execute("DELETE FROM activity")
        else:
            cutoff = (
                datetime.now(timezone.utc) - timedelta(days=days)
            ).strftime("%Y-%m-%dT%H:%M:%SZ")
            cur = conn.execute(
                "DELETE FROM activity WHERE datetime < ?", (cutoff,),
            )
        deleted = cur.rowcount or 0
        conn.commit()
    finally:
        conn.close()
    return 200, {"deleted": deleted, "days_kept": days}


def dashboard_summary(*, instance: str | None = None) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    runner_snap = runtime.runner_status_snapshot()
    instance_scope = (instance or "current").strip() or "current"
    if instance_scope not in ("all", "current"):
        instance_scope = "current"
    active_instance_id = project_state.active_instance_id()
    instance_where = ""
    instance_args: list[Any] = []
    if instance_scope == "current":
        instance_where = "WHERE instance_id = ?"
        instance_args.append(active_instance_id)
    conn = _conn()
    try:
        counts = {}
        for row in conn.execute(
            "SELECT status, COUNT(*) AS n FROM gaps_index "
            f"{instance_where} GROUP BY status",
            instance_args,
        ):
            counts[row["status"]] = row["n"]
        pf = conn.execute(
            "SELECT ok, checked_at, message FROM preflight WHERE id = 1"
        ).fetchone()
        preflight = ({
            "ok": bool(pf["ok"]), "checked_at": pf["checked_at"],
            "message": pf["message"],
        } if pf else None)
        # latest activity (top of feed)
        feed = activity.recent(conn, limit=50)
        # Per-reporter stats: the runner mirrors the latest round's
        # reporter onto `gaps_index.reporter`, so the SQL aggregation
        # gives us exact counts without reading every gap.json.
        reporter_where = "WHERE reporter != ''"
        reporter_args = []
        if instance_where:
            reporter_where += " AND instance_id = ?"
            reporter_args.append(active_instance_id)
        stat_rows = conn.execute(
            "SELECT reporter, status, COUNT(*) AS n "
            "FROM gaps_index "
            f"{reporter_where} "
            "GROUP BY reporter, status",
            reporter_args,
        ).fetchall()
        known_reporters = (
            [r["name"] for r in reporters.list_all(conn)]
            if instance_scope == "all"
            else []
        )
        provider = (db.get_setting(conn, "agent_cli") or "claude").strip().lower()
        quality_timing = quality.timing(conn)
    finally:
        conn.close()
    reporter_stats = _compute_reporter_stats(stat_rows, known_reporters)
    runner_reachable = bool(runner_snap.get("runner_reachable"))
    return 200, {
        "counts": counts,
        "running": runner_snap.get("running") or [],
        "merger": runner_snap.get("merger"),
        "governance": runner_snap.get("governance"),
        "preflight": preflight,
        "activity": feed,
        "runner_reachable": runner_reachable,
        "reporter_stats": reporter_stats,
        "instance_scope": instance_scope,
        "instance_filter": "all" if instance_scope == "all" else "current",
        "quality_timing": quality_timing,
        "active_instance_id": active_instance_id,
        "active_instance_display_name": project_state.gap_instance_display(active_instance_id),
        "needs_attention": _compute_needs_attention(
            counts, preflight, runner_reachable, provider,
            instance_filter="all" if instance_scope == "all" else "current",
        ),
    }


def process_summary() -> tuple[int, dict]:
    """Return managed process state for System > Processes."""
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked

    runner_snap = runtime.runner_status_snapshot()
    backend = runner_snap.get("backend") or runtime.backend_info()
    runner_reachable = bool(runner_snap.get("runner_reachable"))
    runner_pid = runner_snap.get("pid")
    supervisor_pid = _int_or_none(os.environ.get("REFINE_SUPERVISOR_PID"))
    conn = _conn()
    try:
        settings = db.list_settings(conn)
        background_stopped = (db.get_setting(conn, "paused") or "0") == "1"
        agents_paused = (db.get_setting(conn, "agents_paused") or "0") == "1"
        agent_processes_paused = background_stopped or agents_paused
        target_app = _target_app_snapshot(conn)
    finally:
        conn.close()
    resource_caps = _process_resource_caps(settings)
    worker_caps = {
        "cpu_priority": resource_caps["worker_cpu_priority"],
        "max_memory": resource_caps["worker_max_memory"],
    }
    ui_caps = {
        "cpu_priority": resource_caps["ui_cpu_priority"],
        "max_memory": resource_caps["ui_max_memory"],
    }
    unmanaged_caps = {
        "cpu_priority": {"label": "unmanaged"},
        "max_memory": {"label": "unmanaged"},
    }
    no_caps = {
        "cpu_priority": {"label": "-"},
        "max_memory": {"label": "-"},
    }

    processes: list[dict[str, Any]] = []
    if backend.get("process_model") == "supervisor":
        processes.append({
            "id": "supervisor",
            "kind": "supervisor",
            "label": "Supervisor",
            "status": "running" if supervisor_pid else "unknown",
            "pid": supervisor_pid,
            "details": (
                "Supervises the UI and runner worker processes; shuts Refine "
                "down if either exits."
            ),
            "background_processes_stopped": background_stopped,
            "agents_paused": agents_paused,
            "actions": [
                "start_background_processes"
                if background_stopped
                else "stop_background_processes"
            ],
            **no_caps,
        })
    processes.extend([
        {
            "id": "ui",
            "kind": "ui",
            "label": "UI process",
            "status": "running",
            "pid": os.getpid(),
            "details": "Serves the web UI, API routes, and SSE updates.",
            "actions": [],
            **ui_caps,
        },
        {
            "id": "runner",
            "kind": "runner",
            "label": (
                "Runner worker"
                if backend.get("process_model") == "supervisor"
                else "In-process runner"
            ),
            "status": "running" if runner_reachable else "unreachable",
            "pid": runner_pid,
            "actions": [],
            **worker_caps,
        },
        {
            "id": "target-app",
            "kind": "target_app",
            "label": "Target application",
            "status": target_app.get("state") or "unknown",
            "pid": None,
            "actions": ["start", "rebuild", "stop", "check"],
            "target_app": target_app,
            **unmanaged_caps,
        },
    ])

    merger = runner_snap.get("merger") or None
    governance = runner_snap.get("governance") or None
    target_app_rebuild = runner_snap.get("target_app_rebuild") or None
    runner_work = _runner_work_summary(
        merger, governance, target_app_rebuild, paused=background_stopped,
    )

    for chat in runner_snap.get("chat") or []:
        session_id = str(chat.get("session_id") or "")
        processes.append({
            "id": f"chat:{session_id}",
            "kind": "chat",
            "label": "Chat",
            "status": chat.get("status") or "running",
            "session_id": session_id,
            "pid": chat.get("pid"),
            "provider": chat.get("provider"),
            "mode": chat.get("mode"),
            "gap_id": chat.get("gap_id"),
            "elapsed_seconds": chat.get("elapsed_seconds") or 0,
            "idle_seconds": chat.get("idle_seconds") or 0,
            "actions": ["stop"],
            **worker_caps,
        })

    for run in runner_snap.get("running") or []:
        gap_id = str(run.get("gap_id") or "")
        run_kind = str(run.get("kind") or "implementation")
        processes.append({
            "id": f"agent:{gap_id}",
            "kind": "agent",
            "label": "Quality agent" if run_kind == "quality" else "Agent",
            "status": "running",
            "gap_id": gap_id,
            "round_idx": run.get("round_idx"),
            "run_kind": run_kind,
            "pid": run.get("pid"),
            "elapsed_seconds": run.get("elapsed_seconds") or 0,
            "idle_seconds": run.get("idle_seconds") or 0,
            "actions": ["cancel"],
            **worker_caps,
        })

    return 200, {
        "paused": agent_processes_paused,
        "agents_paused": agents_paused,
        "agent_processes_paused": agent_processes_paused,
        "background_processes_stopped": background_stopped,
        "backend": backend,
        "runner_reachable": runner_reachable,
        "processes": processes,
        "running": runner_snap.get("running") or [],
        "chat": runner_snap.get("chat") or [],
        "runner_work": runner_work,
        "merger": merger,
        "governance": governance,
        "target_app_rebuild": target_app_rebuild,
        "target_app": target_app,
        "resource_caps": resource_caps,
    }


def set_background_processes(body: dict | None = None) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    body = body or {}
    conn = _conn()
    try:
        current_stopped = (db.get_setting(conn, "paused") or "0") == "1"
        stopped = (
            not current_stopped
            if "stopped" not in body
            else str(body.get("stopped")).strip().lower() in {"1", "true", "yes", "on"}
        )
        db.set_setting(conn, "paused", "1" if stopped else "0")
        project_state.set_setting("paused", "1" if stopped else "0")
        activity.append(
            conn,
            message=(
                "Background processes stopped"
                if stopped
                else "Background processes started"
            ),
            severity="warn" if stopped else "info",
            category="state",
            actor="refine",
        )
    finally:
        conn.close()

    cancelled_jobs = _cancel_active_background_jobs() if stopped else []
    try:
        runner_result = get_client().call(
            M_BACKGROUND_PROCESSES_SET,
            {"stopped": stopped},
            timeout=30.0 if stopped else 10.0,
        )
    except BackendError as e:
        return _backend_err(e)
    if stopped and not runner_result.get("ok", True):
        cleanup = runner_result.get("cleanup") or {}
        return err(
            409,
            cleanup.get("message") or (
                "background processes stopped but target worktree cleanup "
                "did not complete"
            ),
        )

    status, summary = process_summary()
    if status != 200:
        summary = {}
    return 200, {
        "stopped": stopped,
        "paused": stopped,
        "runner": runner_result,
        "cancelled_background_jobs": len(cancelled_jobs),
        "processes": summary.get("processes") or [],
        "runner_work": summary.get("runner_work") or [],
        "runner_reachable": summary.get("runner_reachable"),
    }


def set_agent_processes(body: dict | None = None) -> tuple[int, dict]:
    blocked = _schema_block_response()
    if blocked is not None:
        return blocked
    body = body or {}
    conn = _conn()
    try:
        current_paused = (db.get_setting(conn, "agents_paused") or "0") == "1"
        paused = (
            not current_paused
            if "paused" not in body
            else str(body.get("paused")).strip().lower() in {"1", "true", "yes", "on"}
        )
        db.set_setting(conn, "agents_paused", "1" if paused else "0")
        project_state.set_setting("agents_paused", "1" if paused else "0")
        activity.append(
            conn,
            message="Agents paused" if paused else "Agents unpaused",
            severity="warn" if paused else "info",
            category="state",
            actor="refine",
        )
    finally:
        conn.close()

    try:
        runner_result = get_client().call(
            M_ENFORCE_SCHEDULING,
            {"settle_timeout_seconds": 8.0},
            timeout=30.0 if paused else 10.0,
        )
    except BackendError as e:
        return _backend_err(e)
    if paused and not runner_result.get("ok", True):
        cleanup = runner_result.get("cleanup") or {}
        return err(
            409,
            cleanup.get("message") or (
                "agents paused but target worktree cleanup did not complete"
            ),
        )

    status, summary = process_summary()
    if status != 200:
        summary = {}
    return 200, {
        "paused": paused,
        "agents_paused": paused,
        "agent_processes_paused": summary.get("agent_processes_paused", paused),
        "background_processes_stopped": summary.get("background_processes_stopped"),
        "runner": runner_result,
        "processes": summary.get("processes") or [],
        "runner_work": summary.get("runner_work") or [],
        "runner_reachable": summary.get("runner_reachable"),
    }


def _int_or_none(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def _runner_work_summary(
    merger: dict | None,
    governance_state: dict | None,
    target_app_rebuild: dict | None,
    *,
    paused: bool = False,
) -> list[dict[str, Any]]:
    merger = merger or {}
    governance_state = governance_state or {}
    target_app_rebuild = target_app_rebuild or {}
    target_status = "idle"
    if target_app_rebuild.get("running"):
        target_status = "running"
    elif target_app_rebuild.get("queued"):
        target_status = "queued"
    rows = [
        {
            "id": "merger",
            "kind": "merger",
            "label": "Merger",
            "status": merger.get("state") or "unknown",
            "gap_id": merger.get("gap_id"),
            "elapsed_seconds": merger.get("elapsed_seconds") or 0,
            "queued": merger.get("queued") or 0,
            "last_outcome": merger.get("last_outcome") or "",
            "details": "Merges ready Gap work into the target branch.",
        },
        {
            "id": "governance",
            "kind": "governance",
            "label": "Governance",
            "status": governance_state.get("state") or "unknown",
            "gap_id": governance_state.get("gap_id"),
            "elapsed_seconds": governance_state.get("elapsed_seconds") or 0,
            "queued": governance_state.get("queued") or 0,
            "last_outcome": governance_state.get("last_outcome") or "",
            "details": (
                "Reviews Gaps against configured governance rules."
                if governance_state.get("configured")
                else "Idle until governance rules are configured."
            ),
        },
        {
            "id": "target-app-rebuilder",
            "kind": "target_app_rebuilder",
            "label": "Target-app rebuilder",
            "status": target_status if target_app_rebuild else "unknown",
            "queued": 1 if target_app_rebuild.get("queued") else 0,
            "details": (
                target_app_rebuild.get("last_reason")
                or "Rebuilds the target application after merged work."
            ),
        },
        _static_worker_row(
            "target-app-config-generator",
            "target_app_config_generator",
            "Target-app config generator",
            "Uses the AI provider to draft target-app commands from the codebase.",
        ),
        _background_worker_row(
            "sqlite-cache-rebuilder",
            "sqlite_cache_rebuild",
            "SQLite cache rebuilder",
            "Rebuilds index.sqlite from canonical .refine JSON.",
        ),
        _static_worker_row(
            "activity-log-cleanup",
            "activity_log_cleanup",
            "Activity log cleanup",
            "Deletes activity log entries older than the selected retention window.",
        ),
        _background_worker_row(
            "import-preparer",
            "import_prepare",
            "Import preparer",
            "Parses and deduplicates imported Gap drafts before review.",
        ),
        _background_worker_row(
            "import-persister",
            "import_persist",
            "Import persister",
            "Persists large imported Gap batches in the background.",
        ),
        _background_worker_row(
            "bulk-gap-updater",
            "bulk_update_gaps",
            "Bulk Gap updater",
            "Applies large bulk Gap updates in the background.",
        ),
        _background_worker_row(
            "bulk-gap-deleter",
            "bulk_delete_gaps",
            "Bulk Gap deleter",
            "Deletes large selected Gap batches in the background.",
        ),
    ]
    if paused:
        rows = [_paused_runner_worker(row) for row in rows]
    return rows


def _paused_runner_worker(row: dict[str, Any]) -> dict[str, Any]:
    paused = dict(row)
    paused["status"] = "paused"
    paused["queued"] = 0
    paused["paused"] = True
    return paused


def _static_worker_row(
    worker_id: str,
    kind: str,
    label: str,
    details: str,
) -> dict[str, Any]:
    return {
        "id": worker_id,
        "kind": kind,
        "label": label,
        "status": "idle",
        "gap_id": None,
        "elapsed_seconds": 0,
        "queued": 0,
        "details": details,
    }


def _background_worker_row(
    worker_id: str,
    job_kind: str,
    label: str,
    details: str,
) -> dict[str, Any]:
    job = _active_background_job(job_kind)
    if not job:
        return _static_worker_row(worker_id, job_kind, label, details)
    progress = job.get("progress") or {}
    message = progress.get("message") or job.get("label") or details
    return {
        "id": worker_id,
        "kind": job_kind,
        "label": label,
        "status": job.get("status") or "idle",
        "gap_id": None,
        "elapsed_seconds": _elapsed_since(job.get("started_at")),
        "queued": 1 if job.get("status") == "queued" else 0,
        "details": message,
        "job_id": job.get("id"),
        "progress": progress,
    }


def _active_background_job(kind: str) -> dict[str, Any] | None:
    try:
        conn = _conn()
        try:
            rows = conn.execute(
                "SELECT id FROM background_jobs "
                "WHERE kind = ? AND status IN ('queued', 'running') "
                "ORDER BY started_at DESC LIMIT 5",
                (kind,),
            ).fetchall()
        finally:
            conn.close()
    except Exception:
        return None
    for row in rows:
        snap = background_jobs.snapshot(str(row["id"]))
        if snap and snap.get("status") in {"queued", "running"}:
            return snap
    return None


def _elapsed_since(value: Any) -> int:
    text = str(value or "").strip()
    if not text:
        return 0
    try:
        started = datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        return 0
    now = datetime.now(started.tzinfo or timezone.utc)
    return max(0, int((now - started).total_seconds()))


def _process_resource_caps(settings: dict[str, str]) -> dict[str, Any]:
    resource_settings = runtime_resources.ResourceSettings.from_settings(settings)
    worker_memory_mb = runtime_resources.memory_limit_mb(resource_settings, "agent")
    ui_memory_mb = runtime_resources.memory_limit_mb(resource_settings, "ui")
    worker_cpu_weight = runtime_resources.cpu_weight(resource_settings, "agent")
    return {
        "worker_cpu_priority": {
            "label": _cpu_priority_label(
                resource_settings.worker_cpu_priority,
                worker_cpu_weight,
            ),
            "weight": worker_cpu_weight,
            "priority": resource_settings.worker_cpu_priority,
        },
        "ui_cpu_priority": {
            "label": "normal (weight 100)",
            "weight": 100,
            "priority": "normal",
        },
        "worker_max_memory": {
            "label": _memory_limit_label(worker_memory_mb),
            "mb": worker_memory_mb,
            "configured_mb": resource_settings.worker_memory_limit_mb,
        },
        "ui_max_memory": {
            "label": _memory_limit_label(ui_memory_mb),
            "mb": ui_memory_mb,
            "configured_mb": resource_settings.ui_memory_limit_mb,
        },
    }


def _memory_limit_label(memory_mb: int) -> str:
    return f"{memory_mb} MB" if memory_mb else "uncapped"


def _cpu_priority_label(priority: str, weight: int) -> str:
    label = priority.replace("_", " ")
    return f"{label} (weight {weight})"


_ACTIVE_STATUSES = ("todo", "in-progress", "qa", "ready-merge", "awaiting-rebuild", "review")


def _compute_reporter_stats(stat_rows, known_reporters: list[str]) -> list[dict]:
    """Build `reporter_stats` from the pre-aggregated (reporter, status,
    count) rows produced by the dashboard query. Seeds every known
    reporter (so inactive ones show as zeroes), then folds in any
    historical reporters that appear on Gaps but aren't in the table."""
    def _empty(name: str) -> dict:
        return {"reporter": name, "active": 0, "done": 0,
                "reported": 0, "completion_rate": 0.0}

    by_reporter: dict[str, dict] = {n: _empty(n) for n in known_reporters}
    for row in stat_rows:
        reporter = row["reporter"]
        bucket = by_reporter.setdefault(reporter, _empty(reporter))
        n = row["n"]
        bucket["reported"] += n
        status = row["status"]
        if status in _ACTIVE_STATUSES:
            bucket["active"] += n
        elif status == "done":
            bucket["done"] += n
    out = list(by_reporter.values())
    for b in out:
        b["completion_rate"] = (
            round(100.0 * b["done"] / b["reported"], 1) if b["reported"] else 0.0
        )
    out.sort(key=lambda b: (-b["done"], b["reporter"].lower()))
    return out


def _compute_needs_attention(counts: dict, preflight: dict | None,
                              runner_reachable: bool,
                              provider: str = "claude",
                              instance_filter: str = "current") -> list[dict]:
    items: list[dict] = []
    if not runner_reachable:
        items.append({
            "kind": "banner", "severity": "error",
            "message": "Backend runner unavailable",
        })
    if preflight and not preflight.get("ok"):
        login_hint = {
            "claude": "claude login",
            "codex": "codex login",
            "gemini": "gemini auth login",
            "copilot": "copilot login",
        }.get(provider, f"{provider} login")
        items.append({
            "kind": "banner", "severity": "error",
            "message": f"Refine cannot reach {provider} — run `{login_hint}` on the host",
        })
    if counts.get("failed", 0):
        items.append({
            "kind": "filter", "severity": "warn",
            "message": f"{counts['failed']} failed Gaps",
            "filter": {"status": "failed", "instance": instance_filter},
        })
    return items


# --- Import (LLM extraction) --------------------------------------------------

IMPORT_DEDUP_THRESHOLD = 0.62
_IMPORT_DEDUP_STOPWORDS = {
    "a", "an", "and", "are", "as", "be", "can", "for", "from", "in", "is",
    "it", "of", "on", "or", "the", "to", "user", "users", "when", "with",
}

def import_extract(body: dict) -> tuple[int, dict]:
    """LLM-driven extraction: hand the raw text to the host agent CLI
    via the runner and return the parsed `{name, actual, target}` drafts
    for the user to review before persisting. Times out generously since
    the model call can take 30–90s for longer pastes.
    """
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    raw = (body.get("text") or "").strip()
    if not raw:
        return err(400, "text is required")
    client = get_client()
    try:
        result = client.call(M_EXTRACT_GAPS, {"text": raw}, timeout=200.0)
    except BackendError as e:
        return _backend_err(e)
    return 200, {"drafts": result.get("drafts") or []}


def import_parse_csv(body: dict) -> tuple[int, dict]:
    raw = str(body.get("text") or "")
    if not raw.strip():
        return err(400, "CSV text is required")
    if body.get("background") is True:
        stopped = _background_processes_stopped_response()
        if stopped is not None:
            return stopped
        job_body = {"text": raw, "background": False, "dedup": bool(body.get("dedup"))}

        def run_job() -> dict[str, Any]:
            status, result = import_parse_csv(job_body)
            return {"http_status": status, **result}

        job = background_jobs.start(
            "import_prepare",
            "Prepare CSV import",
            run_job,
        )
        return 202, {"queued": True, "job": job}
    try:
        drafts = _import_parse_csv_drafts(raw)
    except ValueError as e:
        return err(400, str(e))
    if body.get("dedup"):
        matches = _import_dedup_matches(drafts)
        drafts = _annotate_import_duplicate_drafts(drafts, matches)
    return 200, {"drafts": drafts, "count": len(drafts)}


def _import_prepare_cancel_if_requested() -> None:
    if background_jobs.current_cancelled():
        raise background_jobs.CancellationRequested({"cancelled": True})


def _import_prepare_progress(completed: int, total: int, message: str) -> None:
    job_id = background_jobs.current_job_id()
    if not job_id:
        return
    background_jobs.progress(
        job_id,
        completed=completed,
        total=total,
        message=message,
    )


def _import_parse_csv_drafts(raw: str) -> list[dict[str, str]]:
    text = raw.lstrip("\ufeff")
    try:
        sample = text[:8192]
        dialect = csv.Sniffer().sniff(sample, delimiters=",\t;|")
        dialect.doublequote = True
    except csv.Error:
        dialect = csv.excel
    stream = io.StringIO(text, newline="")
    try:
        reader = csv.DictReader(stream, dialect=dialect, skipinitialspace=True)
    except csv.Error as e:
        raise ValueError(f"CSV could not be parsed: {e}") from e
    if not reader.fieldnames:
        raise ValueError("CSV header row is required")
    headers = {
        str(name or "").strip().lower(): name
        for name in reader.fieldnames
    }
    required = ("actual", "target", "reporter", "priority")
    missing = [field for field in required if field not in headers]
    if missing:
        noun = "field" if len(missing) == 1 else "fields"
        raise ValueError(f"CSV is missing required {noun}: {', '.join(missing)}")
    try:
        prepared_rows: list[tuple[int, dict[str, str]]] = []
        for row_number, row in enumerate(reader, start=2):
            values = {
                key: str(row.get(original) or "").strip()
                for key, original in headers.items()
            }
            if not any(values.values()):
                continue
            prepared_rows.append((row_number, values))
        total = len(prepared_rows)
        _import_prepare_progress(0, total, f"Parsing CSV 0 of {total} Gaps")
        drafts: list[dict[str, str]] = []
        for idx, (row_number, values) in enumerate(prepared_rows, start=1):
            _import_prepare_cancel_if_requested()
            actual = values.get("actual", "")
            target = values.get("target", "")
            reporter = values.get("reporter", "")
            priority = values.get("priority", "").lower()
            if (not actual and not target) or not reporter or not priority:
                raise ValueError(
                    f"CSV row {row_number} must include actual or target, plus reporter and priority"
                )
            if priority not in _VALID_PRIORITIES:
                raise ValueError(
                    f"CSV row {row_number} priority must be low, medium, or high"
                )
            if not _VALID_REPORTER.match(reporter):
                raise ValueError(f"CSV row {row_number} has an invalid reporter")
            drafts.append({
                "name": values.get("name", ""),
                "actual": actual,
                "target": target,
                "reporter": reporter,
                "priority": priority,
            })
            _import_prepare_progress(idx, total, f"Parsed {idx} of {total} Gaps")
    except csv.Error as e:
        raise ValueError(f"CSV could not be parsed: {e}") from e
    if not drafts:
        raise ValueError("CSV has no importable rows")
    return drafts


def import_dedup(body: dict) -> tuple[int, dict]:
    drafts = body.get("drafts") or []
    if not isinstance(drafts, list):
        return err(400, "drafts must be a list")
    matches = _import_dedup_matches(drafts)
    return 200, {
        "matches": matches,
        "threshold": IMPORT_DEDUP_THRESHOLD,
        "algorithm": (
            "deterministic normalized actual/target scoring: "
            "token/bigram cosine, character-trigram cosine, token Jaccard, "
            "and sequence ratio"
        ),
    }


def _import_dedup_matches(drafts: list[Any]) -> list[dict[str, Any]]:
    conn = _conn()
    try:
        candidates = _import_dedup_candidates(conn)
    finally:
        conn.close()
    matches: list[dict[str, Any]] = []
    total = len(drafts)
    _import_prepare_progress(0, total, f"Checking duplicates 0 of {total} Gaps")
    for idx, draft in enumerate(drafts, start=1):
        _import_prepare_cancel_if_requested()
        if not isinstance(draft, dict):
            _import_prepare_progress(idx, total, f"Checked duplicates for {idx} of {total} Gaps")
            continue
        actual = (draft.get("actual") or "").strip()
        target = (draft.get("target") or "").strip()
        if not actual and not target:
            _import_prepare_progress(idx, total, f"Checked duplicates for {idx} of {total} Gaps")
            continue
        best, best_score = _best_import_duplicate(actual, target, candidates)
        if best and best_score >= IMPORT_DEDUP_THRESHOLD:
            matches.append({
                "index": idx,
                "score": round(best_score, 3),
                "draft": {"actual": actual, "target": target},
                "match": best,
            })
        _import_prepare_progress(idx, total, f"Checked duplicates for {idx} of {total} Gaps")
    return matches


def _annotate_import_duplicate_drafts(
    drafts: list[dict[str, Any]],
    matches: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    by_index = {int(match["index"]) - 1: match for match in matches}
    out: list[dict[str, Any]] = []
    for idx, draft in enumerate(drafts):
        match = by_index.get(idx)
        if not match:
            out.append(draft)
            continue
        annotated = dict(draft)
        annotated["duplicate"] = match["match"]
        annotated["duplicateDecision"] = str(draft.get("duplicateDecision") or "")
        out.append(annotated)
    return out


def _duplicate_move_to_backlog_status(status: str | None) -> dict[str, Any]:
    current = str(status or "").strip() or "unknown"
    if current == "backlog":
        return {
            "can_move_to_backlog": False,
            "move_to_backlog_reason": "already_backlog",
        }
    if current in _DUPLICATE_BACKLOG_PROTECTED_STATUSES:
        return {
            "can_move_to_backlog": False,
            "move_to_backlog_reason": "protected_status",
        }
    return {
        "can_move_to_backlog": True,
        "move_to_backlog_reason": "",
    }


def _move_duplicate_original_to_backlog(gap_id: str) -> dict[str, Any]:
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT id, status, instance_id FROM gaps_index WHERE id = ?",
            (gap_id,),
        ).fetchone()
        if row is None:
            return {
                "moved": False,
                "reason": "missing",
                "gap_id": gap_id,
                "from": "unknown",
                "to": "backlog",
            }
        previous = str(row["status"] or "backlog")
        gate = _duplicate_move_to_backlog_status(previous)
        if not gate["can_move_to_backlog"]:
            return {
                "moved": False,
                "reason": gate["move_to_backlog_reason"],
                "gap_id": gap_id,
                "from": previous,
                "to": "backlog",
            }
        updated_at = now_iso()
        with db.transaction(conn):
            cur = conn.execute(
                "UPDATE gaps_index SET status = 'backlog', updated = ? "
                "WHERE id = ? AND status = ?",
                (updated_at, gap_id, previous),
            )
        if not cur.rowcount:
            reread = conn.execute(
                "SELECT status FROM gaps_index WHERE id = ?",
                (gap_id,),
            ).fetchone()
            current = str(reread["status"] if reread else "unknown")
            return {
                "moved": False,
                "reason": "status_changed",
                "gap_id": gap_id,
                "from": current,
                "to": "backlog",
            }
    finally:
        conn.close()
    try:
        gap = gap_writer.update_fields(gap_id, status="backlog")
        conn = _conn()
        try:
            with db.transaction(conn):
                search_index.upsert_gap(conn, gap)
        finally:
            conn.close()
        _append_gap_workflow_log(
            gap_id,
            f"Workflow status changed: {previous} → backlog; duplicate import recovery",
        )
    except Exception:
        pass
    return {
        "moved": True,
        "reason": "",
        "gap_id": gap_id,
        "from": previous,
        "to": "backlog",
    }


def _import_dedup_candidates(conn: sqlite3.Connection) -> list[dict[str, Any]]:
    rows = conn.execute(
        "SELECT id, name, status, priority, instance_id FROM gaps_index"
    ).fetchall()
    out: list[dict[str, Any]] = []
    for row in rows:
        gap = shared_gaps.read_gap_json(row["id"], include_logs=False) or {}
        rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
        if not rounds:
            continue
        latest = rounds[-1]
        actual = str(latest.get("actual") or "").strip()
        target = str(latest.get("target") or "").strip()
        if not actual and not target:
            continue
        move_gate = _duplicate_move_to_backlog_status(row["status"])
        out.append({
            "id": row["id"],
            "name": row["name"] or gap.get("name") or row["id"],
            "status": row["status"],
            "priority": row["priority"] or gap.get("priority") or "low",
            "instance_id": row["instance_id"] or project_state.DEFAULT_INSTANCE_ID,
            "instance_display_name": project_state.gap_instance_display(row["instance_id"]),
            "actual": actual,
            "target": target,
            **move_gate,
        })
    return out


def _find_import_duplicate(
    actual: str,
    target: str,
    candidates: list[dict[str, Any]] | None = None,
) -> dict[str, Any] | None:
    if candidates is None:
        conn = _conn()
        try:
            candidates = _import_dedup_candidates(conn)
        finally:
            conn.close()
    best, best_score = _best_import_duplicate(actual, target, candidates)
    if best and best_score >= IMPORT_DEDUP_THRESHOLD:
        return {"score": round(best_score, 3), "match": best}
    return None


def _best_import_duplicate(
    actual: str,
    target: str,
    candidates: list[dict[str, Any]],
) -> tuple[dict[str, Any] | None, float]:
    best: dict[str, Any] | None = None
    best_score = 0.0
    for candidate in candidates:
        score = _import_dedup_score(
            actual,
            target,
            candidate["actual"],
            candidate["target"],
        )
        if score > best_score:
            best_score = score
            best = candidate
    return best, best_score


def _import_dedup_score(
    draft_actual: str,
    draft_target: str,
    candidate_actual: str,
    candidate_target: str,
) -> float:
    draft = _import_dedup_normalize(f"{draft_actual}\n{draft_target}")
    candidate = _import_dedup_normalize(f"{candidate_actual}\n{candidate_target}")
    if not draft or not candidate:
        return 0.0
    if draft == candidate:
        return 1.0
    draft_numbers = set(re.findall(r"\d+", draft))
    candidate_numbers = set(re.findall(r"\d+", candidate))
    trigram = _import_trigram_cosine(draft, candidate)
    jaccard = _import_token_jaccard(draft, candidate)
    sequence = difflib.SequenceMatcher(None, draft, candidate).ratio()
    strict_score = (0.55 * trigram) + (0.30 * jaccard) + (0.15 * sequence)
    token_score = _import_token_cosine(draft, candidate)
    fuzzy_score = (0.45 * token_score) + (0.35 * sequence) + (0.20 * trigram)
    score = max(strict_score, fuzzy_score)
    if draft_numbers and candidate_numbers and draft_numbers != candidate_numbers:
        score = min(score, 0.5)
    return score


def _import_dedup_normalize(text: str) -> str:
    text = re.sub(r"[^a-z0-9]+", " ", text.lower())
    return re.sub(r"\s+", " ", text).strip()


def _import_token_jaccard(a: str, b: str) -> float:
    aa = set(a.split())
    bb = set(b.split())
    if not aa or not bb:
        return 0.0
    return len(aa & bb) / len(aa | bb)


def _import_token_cosine(a: str, b: str) -> float:
    ca = _import_token_counts(a)
    cb = _import_token_counts(b)
    if not ca or not cb:
        return 0.0
    dot = sum(ca[key] * cb.get(key, 0) for key in ca)
    mag_a = sum(v * v for v in ca.values()) ** 0.5
    mag_b = sum(v * v for v in cb.values()) ** 0.5
    if not mag_a or not mag_b:
        return 0.0
    return dot / (mag_a * mag_b)


def _import_token_counts(text: str) -> Counter[str]:
    tokens = [
        _import_stem_token(token)
        for token in text.split()
        if token not in _IMPORT_DEDUP_STOPWORDS
    ]
    counts: Counter[str] = Counter(tokens)
    counts.update(
        f"{left} {right}"
        for left, right in zip(tokens, tokens[1:])
    )
    return counts


def _import_stem_token(token: str) -> str:
    for suffix in ("ing", "ed", "es", "s"):
        if len(token) > len(suffix) + 3 and token.endswith(suffix):
            return token[:-len(suffix)]
    return token


def _import_trigram_cosine(a: str, b: str) -> float:
    ca = _import_char_ngrams(a)
    cb = _import_char_ngrams(b)
    if not ca or not cb:
        return 0.0
    dot = sum(ca[key] * cb.get(key, 0) for key in ca)
    mag_a = sum(v * v for v in ca.values()) ** 0.5
    mag_b = sum(v * v for v in cb.values()) ** 0.5
    if not mag_a or not mag_b:
        return 0.0
    return dot / (mag_a * mag_b)


def _import_char_ngrams(text: str, n: int = 3) -> Counter[str]:
    compact = f"  {text}  "
    if len(compact) <= n:
        return Counter([compact])
    return Counter(compact[i:i + n] for i in range(len(compact) - n + 1))


@_exclusive_mutation("Import Gaps")
def import_persist(body: dict) -> tuple[int, dict]:
    """Persist user-confirmed extracted Gaps."""
    drafts = body.get("drafts") or []
    if (
        isinstance(drafts, list)
        and (
            body.get("background") is True
            or (
                len(drafts) >= IMPORT_BACKGROUND_THRESHOLD
                and body.get("background") is not False
            )
        )
    ):
        stopped = _background_processes_stopped_response()
        if stopped is not None:
            return stopped
        job_body = json.loads(json.dumps({
            "reporter": (body.get("reporter") or "").strip(),
            "drafts": drafts,
            "background": False,
        }))

        def run_job() -> dict[str, Any]:
            status, result = _import_persist_sync(job_body)
            return {"http_status": status, **result}

        job = background_jobs.start(
            "import_persist",
            f"Import {len(drafts)} Gaps",
            run_job,
        )
        return 202, {"queued": True, "job": job, "drafts": len(drafts)}
    return _import_persist_sync(body)


def _cancel_import_if_requested(
    created: list[str],
    duplicate_moves: list[dict[str, Any]] | None = None,
    duplicate_updates: list[dict[str, Any]] | None = None,
) -> None:
    if not background_jobs.current_cancelled():
        return
    rolled_back = _rollback_import_created_gaps(created)
    restored = _rollback_import_duplicate_moves(duplicate_moves or [])
    restored_updates = _rollback_import_duplicate_updates(duplicate_updates or [])
    raise background_jobs.CancellationRequested({
        "cancelled": True,
        "created": created,
        "rolled_back": rolled_back,
        "restored_duplicates": restored,
        "restored_original_updates": restored_updates,
        "count": 0,
        "failures": [],
        "failed": 0,
    })


def _rollback_import_created_gaps(created: list[str]) -> int:
    if not created:
        return 0
    try:
        result = get_client().call(
            M_BULK_DELETE_GAPS,
            {"gap_ids": list(reversed(created))},
            timeout=120.0,
        )
        return int(result.get("deleted") or 0)
    except BackendError:
        rolled_back = 0
        for gap_id in reversed(created):
            try:
                result = get_client().call(M_DELETE_GAP, {"gap_id": gap_id}, timeout=30.0)
                if result.get("deleted"):
                    rolled_back += 1
            except BackendError:
                continue
        return rolled_back


def _rollback_import_duplicate_moves(moves: list[dict[str, Any]]) -> int:
    restored = 0
    for move in reversed(moves):
        gap_id = str(move.get("gap_id") or "")
        previous = str(move.get("from") or "")
        if not gap_id or not previous or previous == "backlog":
            continue
        updated_at = now_iso()
        try:
            conn = _conn()
            try:
                with db.transaction(conn):
                    cur = conn.execute(
                        "UPDATE gaps_index SET status = ?, updated = ? "
                        "WHERE id = ? AND status = 'backlog'",
                        (previous, updated_at, gap_id),
                    )
                if not cur.rowcount:
                    continue
            finally:
                conn.close()
            gap = gap_writer.update_fields(gap_id, status=previous)
            conn = _conn()
            try:
                with db.transaction(conn):
                    search_index.upsert_gap(conn, gap)
            finally:
                conn.close()
            _append_gap_workflow_log(
                gap_id,
                f"Workflow status changed: backlog → {previous}; import cancel rollback",
            )
            restored += 1
        except Exception:
            continue
    return restored


def _rollback_import_duplicate_updates(updates: list[dict[str, Any]]) -> int:
    restored = 0
    for update in reversed(updates):
        gap_id = str(update.get("gap_id") or "")
        before = update.get("before") if isinstance(update.get("before"), dict) else {}
        if not gap_id or not before:
            continue
        try:
            if any(field in before for field in ("actual", "target", "reporter")):
                gap_writer.edit_latest_round(
                    gap_id,
                    actual=before.get("actual") if "actual" in before else None,
                    target=before.get("target") if "target" in before else None,
                    reporter=before.get("reporter") if "reporter" in before else None,
                )
            if "priority" in before:
                _update_gap_priority_no_ownership(gap_id, str(before["priority"]))
            if "reporter" in before:
                _update_gap_reporter_index_no_ownership(gap_id, str(before["reporter"]))
            _upsert_gap_search_no_ownership(gap_id)
            _append_gap_workflow_log(
                gap_id,
                "Original Gap restored after cancelled import update",
            )
            restored += 1
        except Exception:
            continue
    return restored


def _duplicate_update_field(decision: str) -> str:
    if not decision.startswith(_DUPLICATE_UPDATE_PREFIX):
        return ""
    field = decision[len(_DUPLICATE_UPDATE_PREFIX):]
    return field if field in _DUPLICATE_UPDATE_FIELDS else ""


def _latest_round_snapshot(gap_id: str) -> dict[str, Any]:
    gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {}
    rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
    latest = rounds[-1] if rounds else {}
    return {
        "actual": str(latest.get("actual") or ""),
        "target": str(latest.get("target") or ""),
        "reporter": str(latest.get("reporter") or ""),
        "priority": str(gap.get("priority") or "low"),
    }


def _update_gap_priority_no_ownership(gap_id: str, priority: str) -> None:
    updated_at = now_iso()
    conn = _conn()
    try:
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET priority = ?, updated = ? WHERE id = ?",
                (priority, updated_at, gap_id),
            )
    finally:
        conn.close()
    gap_writer.update_fields(gap_id, priority=priority)


def _update_gap_reporter_index_no_ownership(gap_id: str, reporter: str) -> None:
    updated_at = now_iso()
    conn = _conn()
    try:
        with db.transaction(conn):
            conn.execute(
                "UPDATE gaps_index SET reporter = ?, updated = ? WHERE id = ?",
                (reporter, updated_at, gap_id),
            )
    finally:
        conn.close()


def _upsert_gap_search_no_ownership(gap_id: str) -> None:
    gap = shared_gaps.read_gap_json(gap_id, include_logs=False)
    if not gap:
        return
    conn = _conn()
    try:
        with db.transaction(conn):
            search_index.upsert_gap(conn, gap)
    finally:
        conn.close()


def _update_duplicate_original_from_draft(
    *,
    duplicate: dict[str, Any],
    draft: dict[str, str],
    field: str,
) -> dict[str, Any]:
    gap_id = str(duplicate.get("match", {}).get("id") or "")
    if not gap_id:
        raise ValueError("duplicate match is missing")
    before_all = _latest_round_snapshot(gap_id)
    before = {field: before_all[field]}
    if field in {"actual", "target", "reporter"}:
        value = str(draft.get(field) or "").strip()
        if field == "reporter" and (not value or not _VALID_REPORTER.match(value)):
            raise ValueError("invalid reporter name")
        gap_writer.edit_latest_round(
            gap_id,
            actual=value if field == "actual" else None,
            target=value if field == "target" else None,
            reporter=value if field == "reporter" else None,
        )
        if field == "reporter":
            _update_gap_reporter_index_no_ownership(gap_id, value)
    elif field == "priority":
        value = str(draft.get("priority") or "low").strip().lower()
        if value not in _VALID_PRIORITIES:
            raise ValueError("priority must be one of low/medium/high")
        _update_gap_priority_no_ownership(gap_id, value)
    else:
        raise ValueError("unsupported original update field")
    _upsert_gap_search_no_ownership(gap_id)
    _append_gap_workflow_log(
        gap_id,
        f"Original Gap {field} updated from duplicate import",
    )
    return {"gap_id": gap_id, "field": field, "before": before}


def _import_persist_progress(completed: int, total: int, message: str) -> None:
    job_id = background_jobs.current_job_id()
    if not job_id:
        return
    background_jobs.progress(
        job_id,
        completed=completed,
        total=total,
        message=message,
    )


def _import_persist_sync(body: dict) -> tuple[int, dict]:
    """Persist user-confirmed extracted Gaps synchronously."""
    reporter = (body.get("reporter") or "").strip()
    drafts = body.get("drafts") or []
    if not isinstance(drafts, list) or not drafts:
        return err(400, "drafts must be a non-empty list")
    conn = _conn()
    try:
        dedup_candidates = _import_dedup_candidates(conn)
    finally:
        conn.close()
    created: list[str] = []
    failures: list[dict[str, Any]] = []
    duplicate_actions = {
        "ignored": 0,
        "moved_to_backlog": 0,
        "move_noop": 0,
        "updated_original": 0,
        "updated_original_fields": {},
    }
    duplicate_moves: list[dict[str, Any]] = []
    duplicate_updates: list[dict[str, Any]] = []
    total = len(drafts)
    _import_persist_progress(0, total, f"Importing 0 of {total} Gaps")
    for idx, d in enumerate(drafts, start=1):
        _cancel_import_if_requested(created, duplicate_moves, duplicate_updates)
        _import_persist_progress(
            idx - 1,
            total,
            f"Importing Gap {idx} of {total}",
        )
        if not isinstance(d, dict):
            failures.append({
                "index": idx,
                "error": "draft must be an object",
                "draft": {},
            })
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        actual = (d.get("actual") or "").strip()
        target = (d.get("target") or "").strip()
        name = (d.get("name") or "").strip() or _autoname(actual, target)
        draft_reporter = (d.get("reporter") or reporter).strip()
        priority = (d.get("priority") or "low").strip().lower()
        if priority not in _VALID_PRIORITIES:
            failures.append({
                "index": idx,
                "error": "priority must be one of low/medium/high",
                "draft": {
                    "name": name,
                    "actual": actual,
                    "target": target,
                    "reporter": draft_reporter,
                    "priority": priority,
                },
            })
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        if not draft_reporter:
            failures.append({
                "index": idx,
                "error": "reporter is required",
                "draft": {
                    "name": name,
                    "actual": actual,
                    "target": target,
                    "reporter": "",
                    "priority": priority,
                },
            })
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        if not _VALID_REPORTER.match(draft_reporter):
            failures.append({
                "index": idx,
                "error": "invalid reporter name",
                "draft": {
                    "name": name,
                    "actual": actual,
                    "target": target,
                    "reporter": draft_reporter,
                    "priority": priority,
                },
            })
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        if not actual and not target:
            failures.append({
                "index": idx,
                "error": "actual or target must be non-empty",
                "draft": {
                    "name": name,
                    "actual": actual,
                    "target": target,
                    "reporter": draft_reporter,
                    "priority": priority,
                },
            })
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        duplicate_decision = str(d.get("duplicate_decision") or "").strip()
        if duplicate_decision == _DUPLICATE_DECISION_IGNORE:
            duplicate_actions["ignored"] += 1
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        duplicate = _find_import_duplicate(
            actual,
            target,
            candidates=dedup_candidates,
        )
        update_field = _duplicate_update_field(duplicate_decision)
        if update_field and not duplicate:
            failures.append({
                "index": idx,
                "error": "original Gap no longer matches this draft",
                "code": "duplicate_update_missing",
                "draft": {
                    "name": name,
                    "actual": actual,
                    "target": target,
                    "reporter": draft_reporter,
                    "priority": priority,
                },
            })
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        if duplicate and update_field:
            try:
                update = _update_duplicate_original_from_draft(
                    duplicate=duplicate,
                    draft={
                        "actual": actual,
                        "target": target,
                        "reporter": draft_reporter,
                        "priority": priority,
                    },
                    field=update_field,
                )
                duplicate_updates.append(update)
                duplicate_actions["updated_original"] += 1
                field_counts = duplicate_actions["updated_original_fields"]
                field_counts[update_field] = int(field_counts.get(update_field) or 0) + 1
            except Exception as e:
                failures.append({
                    "index": idx,
                    "error": str(e) or "Could not update original Gap",
                    "code": "duplicate_update_failed",
                    "duplicate": duplicate,
                    "draft": {
                        "name": name,
                        "actual": actual,
                        "target": target,
                        "reporter": draft_reporter,
                        "priority": priority,
                    },
                })
            _cancel_import_if_requested(created, duplicate_moves, duplicate_updates)
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        if duplicate and duplicate_decision == _DUPLICATE_DECISION_MOVE_ORIGINAL:
            move = _move_duplicate_original_to_backlog(duplicate["match"]["id"])
            if move.get("moved"):
                duplicate_actions["moved_to_backlog"] += 1
                duplicate_moves.append(move)
            else:
                duplicate_actions["move_noop"] += 1
            _cancel_import_if_requested(created, duplicate_moves, duplicate_updates)
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        if duplicate and duplicate_decision != _DUPLICATE_DECISION_IMPORT:
            failures.append({
                "index": idx,
                "error": "possible duplicate Gap found",
                "code": "duplicate_gap",
                "duplicate": duplicate,
                "draft": {
                    "name": name,
                    "actual": actual,
                    "target": target,
                    "reporter": draft_reporter,
                    "priority": priority,
                },
            })
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
            continue
        gap_id = new_ulid()
        try:
            get_client().call(M_CREATE_GAP, {
                "gap_id": gap_id, "name": name, "reporter": draft_reporter,
                "priority": priority, "actual": actual, "target": target,
            })
            created.append(gap_id)
            _cancel_import_if_requested(created, duplicate_moves, duplicate_updates)
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
        except BackendError as e:
            failures.append({
                "index": idx,
                "error": e.message,
                "code": e.code,
                "draft": {
                    "name": name,
                    "actual": actual,
                    "target": target,
                    "reporter": draft_reporter,
                    "priority": priority,
                },
            })
            _import_persist_progress(idx, total, f"Imported {idx} of {total} drafts")
    _cancel_import_if_requested(created, duplicate_moves, duplicate_updates)
    status = 201 if created and not failures else 200
    return status, {
        "created": created,
        "count": len(created),
        "failures": failures,
        "failed": len(failures),
        "duplicate_actions": duplicate_actions,
    }


# --- Chat ---------------------------------------------------------------------

def chat_start(body: dict) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_START, {
            "gap_id": body.get("gap_id"),
            "purpose": body.get("purpose"),
        })
    except BackendError as e:
        return _backend_err(e)
    return 201, result


def chat_input(sid: str, body: dict) -> tuple[int, dict]:
    text = body.get("text", "")
    try:
        result = get_client().call(M_CHAT_INPUT, {"session_id": sid, "text": text})
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def chat_read(sid: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_READ, {"session_id": sid})
    except BackendError as e:
        return _backend_err(e)
    return 200, result


def chat_stop(sid: str) -> tuple[int, dict]:
    try:
        result = get_client().call(M_CHAT_STOP, {"session_id": sid})
    except BackendError as e:
        return _backend_err(e)
    return 200, result


# --- Target application -------------------------------------------------------
#
# The operator writes plain-language start/stop prompts in Settings (or
# generates them via /api/target-app/generate-instructions). Clicking the
# nav toggle hits /start or /stop, which routes through the runner to a
# Standalone agent. State transitions are recorded in SQLite settings so
# every browser tab sees the same status.

_TARGET_APP_STATES = (
    "unknown", "starting", "rebuilding", "running", "degraded",
    "stopping", "stopped", "failed",
)


def target_app_status() -> tuple[int, dict]:
    """Return the current target-app state + last health-check snapshot."""
    conn = _conn()
    try:
        snap = _target_app_snapshot(conn)
    finally:
        conn.close()
    return 200, snap


def _target_app_snapshot(conn: sqlite3.Connection) -> dict:
    state = db.get_setting(conn, "target_app_state") or "unknown"
    _cleanup_legacy_target_app_settings(conn)
    settings = db.list_settings(conn)
    cfg = _target_app_config(settings)
    last_op = conn.execute(
        "SELECT id, kind, state, started_at, finished_at, exit_code, message "
        "FROM target_app_operations ORDER BY id DESC LIMIT 1"
    ).fetchone()
    legacy_start = (settings.get("target_app_start_instructions") or "").strip()
    legacy_stop = (settings.get("target_app_stop_instructions") or "").strip()
    return {
        "state": state if state in _TARGET_APP_STATES else "unknown",
        "health_url": cfg.get("http_check_url") or "",
        "app_url": settings.get("target_app_url") or "",
        "has_start_command": bool(cfg.get("start_command")),
        "has_stop_command": bool(cfg.get("stop_command")),
        "has_rebuild_command": bool(cfg.get("rebuild_command")),
        "has_status_checks": _has_status_checks(cfg),
        # Back-compat names for older JS during upgrades.
        "has_start_instructions": bool(cfg.get("start_command") or legacy_start),
        "has_stop_instructions": bool(cfg.get("stop_command") or legacy_stop),
        "last_check_at": settings.get("target_app_last_check_at") or "",
        "last_check_ok": (settings.get("target_app_last_check_ok") or "0") == "1",
        "last_check_message": settings.get("target_app_last_check_message") or "",
        "last_health_at": settings.get("target_app_last_check_at") or settings.get("target_app_last_health_at") or "",
        "last_health_ok": (settings.get("target_app_last_check_ok") or settings.get("target_app_last_health_ok") or "0") == "1",
        "last_health_message": settings.get("target_app_last_check_message") or settings.get("target_app_last_health_message") or "",
        "last_error": settings.get("target_app_last_error") or "",
        "last_operation_id": settings.get("target_app_last_operation_id") or "",
        "last_operation": dict(last_op) if last_op else None,
        "auto_rebuild": (
            settings.get("target_app_auto_rebuild") or "on_worktree_merge"
        ),
        "auto_rebuild_last_started_at": settings.get("target_app_auto_rebuild_last_started_at") or "",
        "auto_rebuild_last_finished_at": settings.get("target_app_auto_rebuild_last_finished_at") or "",
        "auto_rebuild_last_ok": (settings.get("target_app_auto_rebuild_last_ok") or "0") == "1",
        "auto_rebuild_last_message": settings.get("target_app_auto_rebuild_last_message") or "",
        "legacy_config_present": bool(legacy_start or legacy_stop or (settings.get("target_app_health_url") or "").strip()),
    }


def _cleanup_legacy_target_app_settings(conn: sqlite3.Connection) -> bool:
    settings = db.list_settings(conn)
    updates: dict[str, str] = {}
    legacy_health = (settings.get("target_app_health_url") or "").strip()
    if legacy_health:
        if not (settings.get("target_app_http_check_url") or "").strip():
            updates["target_app_http_check_url"] = legacy_health
        updates["target_app_health_url"] = ""
    if (
        (settings.get("target_app_start_instructions") or "").strip()
        and (settings.get("target_app_start_command") or "").strip()
    ):
        updates["target_app_start_instructions"] = ""
    if (
        (settings.get("target_app_stop_instructions") or "").strip()
        and (settings.get("target_app_stop_command") or "").strip()
    ):
        updates["target_app_stop_instructions"] = ""
    for key, value in updates.items():
        db.set_setting(conn, key, value)
    return bool(updates)


def _target_app_config(settings: dict[str, str]) -> dict[str, Any]:
    from refine_server import target_app as target_app_runtime
    return target_app_runtime.config_from_settings(settings)


def _has_status_checks(cfg: dict[str, Any]) -> bool:
    return any((
        (cfg.get("status_command") or "").strip(),
        (cfg.get("http_check_url") or "").strip(),
        (cfg.get("tcp_check_host") or "").strip() and (cfg.get("tcp_check_port") or "").strip(),
        (cfg.get("process_check_command") or "").strip(),
    ))


@_exclusive_mutation("Start target app")
def target_app_start(_body: dict | None = None) -> tuple[int, dict]:
    """Run the configured start command via the host runner."""
    return _target_app_run("start")


@_exclusive_mutation("Stop target app")
def target_app_stop(_body: dict | None = None) -> tuple[int, dict]:
    """Run the configured stop command via the host runner."""
    return _target_app_run("stop")


@_exclusive_mutation("Rebuild target app")
def target_app_rebuild(_body: dict | None = None) -> tuple[int, dict]:
    """Queue the standard stop/rebuild/start target-app rebuild sequence."""
    return target_app_rebuild_queue(_body)


def target_app_rebuild_queue(_body: dict | None = None) -> tuple[int, dict]:
    """Queue the persistent target-app rebuilder worker."""
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        result = get_client().call(M_TARGET_APP_REBUILD_QUEUE, {}, timeout=10.0)
    except BackendError as e:
        return _backend_err(e)
    return 202, result


def hard_reset_worktree(_body: dict | None = None) -> tuple[int, dict]:
    """Destructively reset the host target worktree through the runner."""
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    try:
        result = get_client().call(M_HARD_RESET_WORKTREE, {}, timeout=300.0)
    except BackendError as e:
        return _backend_err(e)
    return (200 if result.get("ok") else 409), result


def _target_app_run(kind: str) -> tuple[int, dict]:
    conn = _conn()
    try:
        settings = db.list_settings(conn)
        cfg = _target_app_config(settings)
        command = (cfg.get(f"{kind}_command") or "").strip()
        if not command:
            msg = f"No {kind} command configured; {kind} is a no-op."
            db.set_setting(conn, "target_app_last_error", "")
            promoted = _promote_rebuilt_gaps(conn) if kind == "rebuild" else 0
            activity.append(
                conn,
                message=f"target-app: {kind} skipped; no {kind} command configured",
                severity="info", category="target_app", actor="refine",
            )
            snap = _target_app_snapshot(conn)
            snap.update({
                "ok": True,
                "noop": True,
                "state": snap.get("state") or "unknown",
                "message": msg,
                "details": "",
                "promoted_gaps": promoted,
            })
            return 200, snap
        # Optimistic transition. The command run is synchronous but may be long;
        # SSE listeners see the in-flight state via /api/target-app/status.
        next_pending = {
            "start": "starting",
            "stop": "stopping",
            "rebuild": "rebuilding",
        }.get(kind, "unknown")
        db.set_setting(conn, "target_app_state", next_pending)
        db.set_setting(conn, "target_app_last_error", "")
        activity.append(
            conn,
            message=f"target-app: {kind} requested via UI",
            severity="info", category="target_app", actor="refine",
        )
    finally:
        conn.close()

    try:
        result = get_client().call(
            M_TARGET_APP_RUN,
            {"kind": kind, "config": cfg},
            timeout=900.0,
        )
    except BackendError as e:
        _target_app_record_failure(kind, e.message)
        return _backend_err(e)

    ok = bool(result.get("ok"))
    final_state = result.get("state") or ("running" if kind == "start" else "stopped")
    if final_state not in _TARGET_APP_STATES:
        final_state = "failed" if not ok else "unknown"
    err_msg = "" if ok else (result.get("message") or "target-app operation failed")
    conn = _conn()
    try:
        db.set_setting(conn, "target_app_state", final_state)
        db.set_setting(conn, "target_app_last_error", err_msg)
        op_id = _record_target_app_operation(conn, kind, result, final_state)
        db.set_setting(conn, "target_app_last_operation_id", str(op_id))
        if result.get("checks_configured"):
            _persist_check_settings(conn, result.get("checks") or [], result.get("message") or "")
        promoted = _promote_rebuilt_gaps(conn) if kind == "rebuild" and ok else 0
        snap = _target_app_snapshot(conn)
    finally:
        conn.close()

    status = 200 if ok else 502
    snap.update({
        "ok": ok,
        "state": final_state,
        "message": result.get("message") or "",
        "details": (
            result.get("stderr_tail")
            or result.get("stdout_tail")
            or json.dumps(result.get("checks") or [])
        )[:8000],
        "promoted_gaps": promoted,
    })
    return status, snap


def _record_target_app_operation(conn: sqlite3.Connection, kind: str,
                                 result: dict, state: str) -> int:
    cur = conn.execute(
        "INSERT INTO target_app_operations "
        "(kind, state, started_at, finished_at, command, cwd, exit_code, "
        "message, stdout_tail, stderr_tail, checks_json) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            kind, state,
            result.get("started_at") or now_iso(),
            result.get("finished_at") or now_iso(),
            result.get("command") or "",
            result.get("cwd") or "",
            result.get("exit_code"),
            result.get("message") or "",
            result.get("stdout_tail") or "",
            result.get("stderr_tail") or "",
            json.dumps(result.get("checks") or []),
        ),
    )
    return int(cur.lastrowid)


def _promote_rebuilt_gaps(conn: sqlite3.Connection) -> int:
    active_instance = project_state.active_instance_id()
    rows = conn.execute(
        "SELECT id FROM gaps_index WHERE status = 'awaiting-rebuild' "
        "AND instance_id = ? ORDER BY updated ASC",
        (active_instance,),
    ).fetchall()
    if not rows:
        return 0
    post_rebuild_quality = quality.enabled(conn) and quality.post_rebuild(conn)
    next_status = "qa" if post_rebuild_quality else "review"
    message = (
        "Target application rebuilt; Gap queued for QA"
        if post_rebuild_quality
        else "Target application rebuilt; Gap is ready for review"
    )
    with db.transaction(conn):
        conn.execute(
            "UPDATE gaps_index SET status = ?, updated = ? "
            "WHERE status = 'awaiting-rebuild' AND instance_id = ?",
            (next_status, now_iso(), active_instance),
        )
    for row in rows:
        gid = row["id"]
        try:
            gap_writer.update_fields(gid, status=next_status, branch_name=None)
            _append_gap_workflow_log(
                gid,
                message,
                actor="refine",
            )
        except Exception:
            pass
        activity.append(
            conn,
            message=message,
            severity="info", category="state", gap_id=gid, actor="refine",
        )
    return len(rows)


def _persist_check_settings(conn: sqlite3.Connection, checks: list[dict],
                            message: str, *, persist: bool = True) -> None:
    ok = bool(checks) and all(bool(c.get("ok")) for c in checks)
    checked_at = now_iso()
    db.set_setting(conn, "target_app_last_check_at", checked_at, persist=persist)
    db.set_setting(conn, "target_app_last_check_ok", "1" if ok else "0", persist=persist)
    db.set_setting(conn, "target_app_last_check_message", message or "", persist=persist)
    # Back-compat mirrors.
    db.set_setting(conn, "target_app_last_health_at", checked_at, persist=persist)
    db.set_setting(conn, "target_app_last_health_ok", "1" if ok else "0", persist=persist)
    db.set_setting(conn, "target_app_last_health_message", message or "", persist=persist)


def target_app_check(_body: dict | None = None) -> tuple[int, dict]:
    """Force an immediate deterministic status check."""
    quiet = bool((_body or {}).get("quiet"))
    conn = _conn()
    try:
        settings = db.list_settings(conn)
        cfg = _target_app_config(settings)
    finally:
        conn.close()
    try:
        result = get_client().call(
            M_TARGET_APP_RUN,
            {"kind": "status", "config": cfg, "quiet": quiet},
            timeout=60.0,
        )
    except BackendError as e:
        _target_app_record_failure("status", e.message)
        return _backend_err(e)
    if result.get("busy") and quiet:
        conn = _conn()
        try:
            return 200, _target_app_snapshot(conn)
        finally:
            conn.close()
    final_state = result.get("state") if result.get("state") in _TARGET_APP_STATES else "unknown"
    conn = _conn()
    try:
        persist_status = not quiet
        db.set_setting(conn, "target_app_state", final_state, persist=persist_status)
        db.set_setting(
            conn,
            "target_app_last_error",
            "" if result.get("ok") else (result.get("message") or "status check failed"),
            persist=persist_status,
        )
        if not quiet:
            op_id = _record_target_app_operation(conn, "status", result, final_state)
            db.set_setting(conn, "target_app_last_operation_id", str(op_id))
        _persist_check_settings(
            conn,
            result.get("checks") or [],
            result.get("message") or "",
            persist=persist_status,
        )
        snap = _target_app_snapshot(conn)
    finally:
        conn.close()
    snap.update({"ok": bool(result.get("ok")), "probe_message": result.get("message") or ""})
    return 200, snap


def target_app_health(_body: dict | None = None) -> tuple[int, dict]:
    """Back-compatible route name for a target-app status check."""
    return target_app_check(_body)


def _target_app_run_health_check() -> dict:
    """Back-compatible poller hook for deterministic target-app status."""
    status, snap = target_app_check({"quiet": True})
    return snap if status == 200 else {"state": "unknown", "last_check_ok": False}


def _target_app_record_failure(kind: str, message: str) -> None:
    conn = _conn()
    try:
        rollback = "stopped" if kind == "start" else (
            "running" if kind == "stop" else "unknown"
        )
        db.set_setting(conn, "target_app_state", rollback)
        db.set_setting(conn, "target_app_last_error", message)
        activity.append(
            conn,
            message=f"target-app: {kind} failed — {message}",
            severity="error", category="target_app", actor="refine",
        )
    finally:
        conn.close()


def target_app_generate(body: dict) -> tuple[int, dict]:
    """Use the agent to draft structured target-app config for this codebase."""
    stopped = _background_processes_stopped_response()
    if stopped is not None:
        return stopped
    kind = (body.get("kind") or "all").strip().lower()
    if kind not in ("all", "start", "stop", "rebuild", "status"):
        return err(400, "kind must be 'all', 'start', 'stop', 'rebuild', or 'status'")
    try:
        result = get_client().call(
            M_TARGET_APP_GENERATE, {"kind": kind}, timeout=600.0,
        )
    except BackendError as e:
        return _backend_err(e)
    if not result.get("ok"):
        return 502, {"error": {"message": result.get("message") or "generation failed"}}
    return 200, {
        "ok": True,
        "config": result.get("config") or {},
        "notes": (result.get("config") or {}).get("notes") or "",
        "raw": result.get("raw") or "",
    }


# --- helpers ------------------------------------------------------------------

def _backend_err(e: BackendError) -> tuple[int, dict]:
    if e.code == "backend_unavailable":
        code = 502
    elif e.code == "instance_ownership":
        code = 409
    elif e.code == "bad_request":
        code = 400
    else:
        code = 500
    return code, {"error": {"code": e.code, "message": e.message,
                            "details": e.details}}

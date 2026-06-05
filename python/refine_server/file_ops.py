"""Shared target-repo file browser operations."""
from __future__ import annotations

import base64
import fnmatch
import os
import re
import sqlite3
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from . import db


FILE_PREVIEW_MAX_BYTES = 1_000_000
FILE_TEXT_CHUNK_BYTES = 128_000
IMAGE_PREVIEW_MAX_BYTES = 5_000_000
FILES_TREE_MAX_DEPTH = 3
FILES_TREE_MAX_ENTRIES = 200
FILES_SEARCH_MAX_SCAN = 20_000
FILE_BROWSER_IGNORE_DEFAULT = "node_modules, .git, .refine, run"
FILE_BROWSER_ALWAYS_IGNORE = ["run"]
IMAGE_MIME_BY_EXT = {
    ".gif": "image/gif",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".webp": "image/webp",
}


def error(code: int, message: str, details: str | None = None) -> tuple[int, dict[str, Any]]:
    body: dict[str, Any] = {"error": {"message": message}}
    if details is not None:
        body["error"]["details"] = details
    return code, body


def file_browser_ignore_patterns(conn: sqlite3.Connection | None = None) -> list[str]:
    raw = db.DEFAULT_SETTINGS.get("file_browser_ignore_patterns", FILE_BROWSER_IGNORE_DEFAULT)
    if conn is not None:
        raw = db.get_setting(conn, "file_browser_ignore_patterns", raw) or raw
    else:
        try:
            local_conn = db.connect()
            try:
                raw = db.get_setting(local_conn, "file_browser_ignore_patterns", raw) or raw
            finally:
                local_conn.close()
        except Exception:
            pass
    patterns = [
        item.strip().replace("\\", "/").strip("/")
        for item in str(raw or "").split(",")
        if item.strip().strip("/")
    ]
    for item in FILE_BROWSER_ALWAYS_IGNORE:
        if item not in patterns:
            patterns.append(item)
    return patterns


def tree(
    root: Path,
    path: str | None = None,
    *,
    recursive: bool = False,
    max_depth: int = FILES_TREE_MAX_DEPTH,
    max_entries: int = FILES_TREE_MAX_ENTRIES,
    ignore_patterns: list[str] | None = None,
) -> tuple[int, dict[str, Any]]:
    resolved = _resolve_repo_path(root, path)
    if resolved[0] is None:
        return resolved[2]
    root, target, rel = resolved
    if not target.exists():
        return error(404, "path not found")
    if not target.is_dir():
        return error(400, "path is not a directory")
    max_depth = max(0, min(FILES_TREE_MAX_DEPTH, int(max_depth)))
    max_entries = max(1, min(FILES_TREE_MAX_ENTRIES, int(max_entries)))
    ignore_patterns = ignore_patterns or file_browser_ignore_patterns()
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
        return error(403, "directory cannot be read", str(e))
    body: dict[str, Any] = {
        "root": str(root),
        "path": rel,
        "entries": entries,
        "truncated": truncated,
        "max_depth": max_depth,
        "max_entries": max_entries,
    }
    if not recursive:
        return 200, body

    entries_by_path: dict[str, list[dict[str, Any]]] = {rel: entries}
    meta_by_path: dict[str, dict[str, Any]] = {
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
            child = dir_path / str(entry["name"])
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
                entries_by_path[str(entry["path"])] = []
                meta_by_path[str(entry["path"])] = {
                    "truncated": False,
                    "depth": depth + 1,
                    "error": "directory cannot be read",
                }
                continue
            entries_by_path[str(entry["path"])] = child_entries
            meta_by_path[str(entry["path"])] = {
                "truncated": child_truncated,
                "depth": depth + 1,
            }
            total += len(child_entries)
            if child_truncated or total >= max_entries:
                global_truncated = True
            walk(child, str(entry["path"]), depth + 1)

    walk(target, rel, 0)
    body.update({
        "entries_by_path": entries_by_path,
        "meta_by_path": meta_by_path,
        "total_entries": total,
        "truncated": global_truncated,
    })
    return 200, body


def search(
    root: Path,
    query: str | None = None,
    *,
    max_entries: int = FILES_TREE_MAX_ENTRIES,
    ignore_patterns: list[str] | None = None,
) -> tuple[int, dict[str, Any]]:
    root = root.resolve()
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
    ignore_patterns = ignore_patterns or file_browser_ignore_patterns()
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
        return error(403, "file search cannot be completed", str(e))
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


def read(
    root: Path,
    path: str | None = None,
    *,
    offset: int = 0,
    limit: int = FILE_TEXT_CHUNK_BYTES,
) -> tuple[int, dict[str, Any]]:
    resolved = _resolve_repo_path(root, path)
    if resolved[0] is None:
        return resolved[2]
    root, target, rel = resolved
    if not target.exists():
        return error(404, "path not found")
    if not target.is_file():
        return error(400, "path is not a file")
    try:
        stat = target.stat()
    except OSError as e:
        return error(403, "file cannot be read", str(e))
    offset = max(0, int(offset))
    limit = max(1, min(FILE_PREVIEW_MAX_BYTES, int(limit)))
    image_mime = IMAGE_MIME_BY_EXT.get(target.suffix.lower())
    base: dict[str, Any] = {
        "root": str(root),
        "path": rel,
        "name": target.name,
        "size": stat.st_size,
        "modified": datetime.fromtimestamp(stat.st_mtime, timezone.utc).isoformat(),
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
            return error(403, "file cannot be read", str(e))
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
        return error(403, "file cannot be read", str(e))
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


def _resolve_repo_path(
    root: Path,
    raw_path: str | None,
) -> tuple[Path, Path, str] | tuple[None, None, tuple[int, dict[str, Any]]]:
    root = root.resolve()
    raw = str(raw_path or "").replace("\\", "/").strip()
    while raw.startswith("/"):
        raw = raw[1:]
    parts = [part for part in raw.split("/") if part]
    if any(part == ".." for part in parts):
        return None, None, error(403, "path must stay inside the target repo")
    target = (root / "/".join(parts)).resolve()
    try:
        rel = target.relative_to(root)
    except ValueError:
        return None, None, error(403, "path must stay inside the target repo")
    return root, target, rel.as_posix() if rel.as_posix() != "." else ""


def _file_entry(root: Path, path: Path) -> dict[str, Any]:
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
        "modified": datetime.fromtimestamp(stat.st_mtime, timezone.utc).isoformat(),
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
) -> tuple[list[dict[str, Any]], bool]:
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
    control = sum(1 for ch in text if ord(ch) < 32 and ch not in "\t\n\f\r")
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
    control = sum(1 for byte in data if byte < 32 and byte not in (9, 10, 12, 13))
    return control / len(data) > 0.30

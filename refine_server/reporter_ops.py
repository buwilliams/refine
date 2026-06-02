"""Shared reporter operations for API and CLI."""
from __future__ import annotations

import re
import sqlite3
from collections.abc import Callable
from typing import Any

from . import reporters
from .backend_protocol import M_MERGE_REPORTER, M_RENAME_REPORTER


RunnerCall = Callable[[str, dict[str, Any], float], dict[str, Any]]
VALID_REPORTER = re.compile(r"^[\w .,@:+/-]{1,120}$")


def list_reporters(conn: sqlite3.Connection) -> dict[str, Any]:
    return {"reporters": reporters.list_all(conn)}


def create_reporter(conn: sqlite3.Connection, name: str) -> dict[str, Any]:
    name = (name or "").strip()
    if not name:
        raise ValueError("name is required")
    return {"reporter": reporters.add(conn, name)}


def delete_reporter(conn: sqlite3.Connection, rid: int) -> dict[str, Any]:
    reporters.remove(conn, rid)
    return {"ok": True}


def rename_reporter(runner_call: RunnerCall, rid: int, name: str) -> dict[str, Any]:
    name = (name or "").strip()
    if not name:
        raise ValueError("name is required")
    if not VALID_REPORTER.match(name):
        raise ValueError("invalid reporter name")
    result = runner_call(M_RENAME_REPORTER, {"rid": rid, "new_name": name}, 60.0)
    return {"ok": True, **result}


def merge_reporter(runner_call: RunnerCall, rid: int, target_rid: int) -> dict[str, Any]:
    if target_rid == rid:
        raise ValueError("cannot merge a reporter into itself")
    result = runner_call(
        M_MERGE_REPORTER,
        {"rid": rid, "target_rid": target_rid},
        60.0,
    )
    return {"ok": True, **result}

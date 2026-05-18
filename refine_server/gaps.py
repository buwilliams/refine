"""gap.json read/write.

The runner is the sole writer (atomic temp+rename); the webapp reads.
Convention is enforced by which package imports `write_gap_json` — the webapp
does not.
"""
from __future__ import annotations

import json
import os
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .paths import gap_dir, gap_json_path  # both consult the loaded Config


RULE_STATES = (
    "unclassified", "passed", "failed", "blocked", "needs_review",
    "needs_context", "exception_requested",
)
META_RULE_STATES = (
    "unclassified", "none", "candidate_rule", "rule_review_needed",
    "ambiguous_rule", "stale_rule", "conflicting_rules",
)
GOVERNANCE_BINARY_STATES = ("unclassified", "pass", "fail")


def now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def new_log_entry(
    message: str,
    *,
    severity: str = "info",
    category: str = "cli",
    details: str | None = None,
    actions: list[dict] | None = None,
    actor: str | None = None,
) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "datetime": now_iso(),
        "severity": severity,
        "category": category,
        "message": message,
    }
    if details is not None:
        entry["details"] = details
    if actions:
        entry["actions"] = actions
    if actor is not None:
        entry["actor"] = actor
    return entry


def empty_gap(gap_id: str, name: str) -> dict[str, Any]:
    now = now_iso()
    return {
        "id": gap_id,
        "name": name,
        "created": now,
        "updated": now,
        "notes": [],
        "rounds": [],
    }


def normalize_notes(value: Any) -> list[dict[str, Any]]:
    """Coerce a gap's `notes` field to the list form regardless of what's on
    disk. Old gap.json files store notes as a single string; we promote that
    to a single-entry list transparently so callers always see a list.
    """
    if value is None:
        return []
    if isinstance(value, list):
        return [n for n in value if isinstance(n, dict)]
    if isinstance(value, str):
        s = value.strip()
        if not s:
            return []
        return [{
            "id": "legacy",
            "author": "",
            "body": s,
            "created": "",
            "updated": "",
        }]
    return []


def new_round(reporter: str, actual: str, target: str) -> dict[str, Any]:
    now = now_iso()
    round_obj = {
        "reporter": reporter,
        "actual": actual,
        "target": target,
        "created": now,
        "updated": now,
        "logs": [],
    }
    reset_round_governance(round_obj)
    return round_obj


def default_round_governance() -> dict[str, Any]:
    return {
        "rule_state": "unclassified",
        "meta_rule_state": "unclassified",
        "product_state": "unclassified",
        "constitution_state": "unclassified",
        "governance_message": "",
        "governance_details": "",
        "governance_checked_at": "",
        "governance_rule_actions": [],
    }


def normalize_round_governance(round_obj: dict[str, Any]) -> dict[str, Any]:
    defaults = default_round_governance()
    for key, default in defaults.items():
        if key not in round_obj:
            round_obj[key] = default.copy() if isinstance(default, list) else default
    if round_obj.get("rule_state") not in RULE_STATES:
        round_obj["rule_state"] = "unclassified"
    if round_obj.get("meta_rule_state") not in META_RULE_STATES:
        round_obj["meta_rule_state"] = "unclassified"
    if round_obj.get("product_state") not in GOVERNANCE_BINARY_STATES:
        round_obj["product_state"] = "unclassified"
    if round_obj.get("constitution_state") not in GOVERNANCE_BINARY_STATES:
        round_obj["constitution_state"] = "unclassified"
    if not isinstance(round_obj.get("governance_rule_actions"), list):
        round_obj["governance_rule_actions"] = []
    for key in ("governance_message", "governance_details", "governance_checked_at"):
        if round_obj.get(key) is None:
            round_obj[key] = ""
    return round_obj


def reset_round_governance(round_obj: dict[str, Any]) -> dict[str, Any]:
    round_obj.update(default_round_governance())
    return round_obj


def read_gap_json(gap_id: str) -> dict[str, Any] | None:
    p = gap_json_path(gap_id)
    if not p.exists():
        return None
    with open(p, "rb") as f:
        gap = json.loads(f.read().decode("utf-8"))
    # Transparent legacy-shape migration: notes used to be a single string.
    gap["notes"] = normalize_notes(gap.get("notes"))
    for round_obj in gap.get("rounds") or []:
        if isinstance(round_obj, dict):
            normalize_round_governance(round_obj)
    return gap


def write_gap_json(gap: dict[str, Any]) -> None:
    """Atomic write: temp file in same directory + rename + fsync directory.

    RUNNER ONLY. Web HTTP handlers must route writes through the backend runner.
    """
    gid = gap["id"]
    d = gap_dir(gid)
    d.mkdir(parents=True, exist_ok=True)
    p = gap_json_path(gid)
    data = json.dumps(gap, ensure_ascii=False, indent=2).encode("utf-8")
    fd, tmp = tempfile.mkstemp(prefix=".gap.", suffix=".tmp", dir=str(d))
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, p)
    except Exception:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        raise
    # fsync the directory to make the rename durable
    try:
        dir_fd = os.open(str(d), os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    except OSError:
        pass  # not all filesystems support directory fsync

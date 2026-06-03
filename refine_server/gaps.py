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
QUALITY_STATES = ("unclassified", "passed", "failed")


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
    try:
        from . import project_state
        node_id = project_state.active_node_id()
    except Exception:
        node_id = "default"
    return {
        "id": gap_id,
        "name": name,
        "status": "backlog",
        "priority": "low",
        "branch_name": None,
        "feature_id": None,
        "feature_order": None,
        "node_id": node_id,
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
        "quality_state": "unclassified",
        "quality_message": "",
        "quality_details": "",
        "quality_checked_at": "",
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
    if round_obj.get("quality_state") not in QUALITY_STATES:
        round_obj["quality_state"] = "unclassified"
    for key in ("quality_message", "quality_details", "quality_checked_at"):
        if round_obj.get(key) is None:
            round_obj[key] = ""
    return round_obj


def reset_round_governance(round_obj: dict[str, Any]) -> dict[str, Any]:
    round_obj.update(default_round_governance())
    return round_obj


def read_gap_json(gap_id: str, *, include_logs: bool = True) -> dict[str, Any] | None:
    from . import perf_metrics

    start = perf_metrics.now()
    p = gap_json_path(gap_id)
    if not p.exists():
        perf_metrics.record(
            "gap_json_read",
            gap_id=gap_id,
            elapsed_ms=perf_metrics.elapsed_ms(start),
            success=False,
            details={"missing": True},
        )
        return None
    raw = b""
    try:
        with open(p, "rb") as f:
            raw = f.read()
        gap = json.loads(raw.decode("utf-8"))
        # Transparent legacy-shape migration: notes used to be a single string.
        gap["notes"] = normalize_notes(gap.get("notes"))
        gap.setdefault("status", "backlog")
        gap.setdefault("priority", "low")
        gap.setdefault("branch_name", None)
        gap.setdefault("feature_id", None)
        gap.setdefault("feature_order", None)
        if "node_id" not in gap and "instance_id" in gap:
            gap["node_id"] = gap.get("instance_id") or "default"
        gap.pop("instance_id", None)
        gap.setdefault("node_id", "default")
        for round_obj in gap.get("rounds") or []:
            if isinstance(round_obj, dict):
                normalize_round_governance(round_obj)
        if include_logs:
            from . import round_logs

            round_logs.hydrate_round_logs(gap)
        else:
            from . import round_logs

            round_logs.strip_embedded_logs(gap)
        perf_metrics.record(
            "gap_json_read",
            gap_id=gap_id,
            elapsed_ms=perf_metrics.elapsed_ms(start),
            bytes_in=len(raw),
            details=_gap_metric_details(gap),
        )
        return gap
    except Exception:
        perf_metrics.record(
            "gap_json_read",
            gap_id=gap_id,
            elapsed_ms=perf_metrics.elapsed_ms(start),
            success=False,
            bytes_in=len(raw) if raw else None,
        )
        raise


def write_gap_json(gap: dict[str, Any]) -> None:
    """Atomic write: temp file in same directory + rename + fsync directory.

    RUNNER ONLY. Web HTTP handlers must route writes through the backend runner.
    """
    from . import perf_metrics

    start = perf_metrics.now()
    fsync_ms = 0.0
    gid = gap["id"]
    d = gap_dir(gid)
    d.mkdir(parents=True, exist_ok=True)
    p = gap_json_path(gid)
    from . import round_logs

    externalized_logs = round_logs.externalize_embedded_logs(gap)
    data = json.dumps(gap, ensure_ascii=False, indent=2).encode("utf-8")
    fd, tmp = tempfile.mkstemp(prefix=".gap.", suffix=".tmp", dir=str(d))
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            fsync_start = perf_metrics.now()
            os.fsync(f.fileno())
            fsync_ms += perf_metrics.elapsed_ms(fsync_start)
        os.replace(tmp, p)
    except Exception:
        perf_metrics.record(
            "gap_json_write",
            gap_id=gid,
            elapsed_ms=perf_metrics.elapsed_ms(start),
            success=False,
            bytes_out=len(data),
            details={
                **_gap_metric_details(gap),
                "fsync_ms": round(fsync_ms, 2),
                "externalized_logs": externalized_logs,
            },
        )
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        raise
    # fsync the directory to make the rename durable
    try:
        dir_fd = os.open(str(d), os.O_RDONLY)
        try:
            fsync_start = perf_metrics.now()
            os.fsync(dir_fd)
            fsync_ms += perf_metrics.elapsed_ms(fsync_start)
        finally:
            os.close(dir_fd)
    except OSError:
        pass  # not all filesystems support directory fsync
    perf_metrics.record(
        "gap_json_write",
        gap_id=gid,
        elapsed_ms=perf_metrics.elapsed_ms(start),
        bytes_out=len(data),
        details={
            **_gap_metric_details(gap),
            "fsync_ms": round(fsync_ms, 2),
            "externalized_logs": externalized_logs,
        },
    )


def _gap_metric_details(gap: dict[str, Any]) -> dict[str, int]:
    rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
    log_count = sum(
        len(r.get("logs") or []) for r in rounds if isinstance(r.get("logs") or [], list)
    )
    return {"round_count": len(rounds), "log_count": log_count}

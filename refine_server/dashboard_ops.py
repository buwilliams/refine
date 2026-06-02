"""Shared dashboard summary operations."""
from __future__ import annotations

import sqlite3
from typing import Any

from . import activity, db, project_state, quality, reporters


ACTIVE_STATUSES = ("todo", "in-progress", "qa", "ready-merge", "awaiting-rebuild", "review")


def empty_dashboard(*, node: str | None = None) -> dict[str, Any]:
    node_scope = normalize_node_scope(node)
    return {
        "counts": {},
        "running": [],
        "merger": None,
        "governance": None,
        "preflight": None,
        "activity": [],
        "runner_reachable": False,
        "reporter_stats": [],
        "node_scope": node_scope,
        "node_filter": "all" if node_scope == "all" else "current",
        "quality_timing": "pre_merge",
        "active_node_id": "",
        "active_node_display_name": "",
        "needs_attention": [],
        "attached": False,
    }


def summary(
    conn: sqlite3.Connection,
    *,
    node: str | None = None,
    runner_snapshot: dict[str, Any],
) -> dict[str, Any]:
    node_scope = normalize_node_scope(node)
    active_node_id = project_state.active_node_id()
    node_where = ""
    node_args: list[Any] = []
    if node_scope == "current":
        node_where = "WHERE node_id = ?"
        node_args.append(active_node_id)

    counts = {}
    for row in conn.execute(
        "SELECT status, COUNT(*) AS n FROM gaps_index "
        f"{node_where} GROUP BY status",
        node_args,
    ):
        counts[row["status"]] = row["n"]
    preflight = _preflight(conn)
    feed = activity.recent(conn, limit=50)
    reporter_where = "WHERE reporter != ''"
    reporter_args = []
    if node_where:
        reporter_where += " AND node_id = ?"
        reporter_args.append(active_node_id)
    stat_rows = conn.execute(
        "SELECT reporter, status, COUNT(*) AS n "
        "FROM gaps_index "
        f"{reporter_where} "
        "GROUP BY reporter, status",
        reporter_args,
    ).fetchall()
    known_reporters = (
        [r["name"] for r in reporters.list_all(conn)]
        if node_scope == "all"
        else []
    )
    provider = (db.get_setting(conn, "agent_cli") or "claude").strip().lower()
    quality_timing = quality.timing(conn)

    reporter_stats = compute_reporter_stats(stat_rows, known_reporters)
    runner_reachable = bool(runner_snapshot.get("runner_reachable"))
    node_filter = "all" if node_scope == "all" else "current"
    return {
        "counts": counts,
        "running": runner_snapshot.get("running") or [],
        "merger": runner_snapshot.get("merger"),
        "governance": runner_snapshot.get("governance"),
        "preflight": preflight,
        "activity": feed,
        "runner_reachable": runner_reachable,
        "reporter_stats": reporter_stats,
        "node_scope": node_scope,
        "node_filter": node_filter,
        "quality_timing": quality_timing,
        "active_node_id": active_node_id,
        "active_node_display_name": project_state.gap_node_display(active_node_id),
        "needs_attention": compute_needs_attention(
            counts,
            preflight,
            runner_reachable,
            provider,
            node_filter=node_filter,
        ),
    }


def normalize_node_scope(node: str | None) -> str:
    node_scope = (node or "current").strip() or "current"
    return node_scope if node_scope in {"all", "current"} else "current"


def compute_reporter_stats(stat_rows, known_reporters: list[str]) -> list[dict]:  # noqa: ANN001
    def empty(name: str) -> dict:
        return {
            "reporter": name,
            "active": 0,
            "done": 0,
            "reported": 0,
            "completion_rate": 0.0,
        }

    by_reporter: dict[str, dict] = {name: empty(name) for name in known_reporters}
    for row in stat_rows:
        reporter = row["reporter"]
        bucket = by_reporter.setdefault(reporter, empty(reporter))
        count = row["n"]
        bucket["reported"] += count
        status = row["status"]
        if status in ACTIVE_STATUSES:
            bucket["active"] += count
        elif status == "done":
            bucket["done"] += count
    out = list(by_reporter.values())
    for bucket in out:
        bucket["completion_rate"] = (
            round(100.0 * bucket["done"] / bucket["reported"], 1)
            if bucket["reported"]
            else 0.0
        )
    out.sort(key=lambda bucket: (-bucket["done"], bucket["reporter"].lower()))
    return out


def compute_needs_attention(
    counts: dict,
    preflight: dict | None,
    runner_reachable: bool,
    provider: str = "claude",
    node_filter: str = "current",
) -> list[dict]:
    items: list[dict] = []
    if not runner_reachable:
        items.append({
            "kind": "banner",
            "severity": "error",
            "message": "Backend runner unavailable",
        })
    if preflight and not preflight.get("ok"):
        login_hint = {
            "claude": "claude login",
            "codex": "codex login",
            "gemini": "gemini auth login",
            "copilot": "copilot login",
            "smoke-ai": "REFINE_SMOKE_AI_PATH",
        }.get(provider, f"{provider} login")
        action = (
            f"set `{login_hint}` on the host"
            if provider == "smoke-ai"
            else f"run `{login_hint}` on the host"
        )
        items.append({
            "kind": "banner",
            "severity": "error",
            "message": f"Refine cannot reach {provider} -- {action}",
        })
    if counts.get("failed", 0):
        items.append({
            "kind": "filter",
            "severity": "warn",
            "message": f"{counts['failed']} failed Gaps",
            "filter": {"status": "failed", "node": node_filter},
        })
    return items


def _preflight(conn: sqlite3.Connection) -> dict[str, Any] | None:
    row = conn.execute(
        "SELECT ok, checked_at, message FROM preflight WHERE id = 1"
    ).fetchone()
    if row is None:
        return None
    return {
        "ok": bool(row["ok"]),
        "checked_at": row["checked_at"],
        "message": row["message"],
    }

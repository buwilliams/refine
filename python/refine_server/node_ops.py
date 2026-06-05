"""Shared node operations used by the API and CLI."""
from __future__ import annotations

import sqlite3
from typing import Any

from . import activity, db, project_state


def summary() -> dict[str, Any]:
    nodes = project_state.list_nodes()
    active = project_state.active_node_id()
    return {
        "nodes": nodes,
        "active_node_id": active,
        "active_node": next(
            (node for node in nodes if node.get("id") == active),
            None,
        ),
    }


def list_with_counts(conn: sqlite3.Connection) -> dict[str, Any]:
    counts: dict[str, dict[str, int]] = {}
    for row in conn.execute(
        "SELECT node_id, status, COUNT(*) AS n "
        "FROM gaps_index GROUP BY node_id, status"
    ):
        counts.setdefault(row["node_id"] or "", {})[row["status"]] = row["n"]
    nodes = project_state.list_nodes()
    known = {node.get("id") for node in nodes}
    unknown_ids = [node_id for node_id in counts if node_id and node_id not in known]
    return {
        "nodes": nodes,
        "active_node_id": project_state.active_node_id(),
        "counts": counts,
        "unknown_node_ids": unknown_ids,
    }


def create(display_name: str) -> dict[str, Any]:
    name = (display_name or "").strip()
    if not name:
        raise ValueError("display_name is required")
    return project_state.create_node(name)


def update(
    node_id: str,
    *,
    display_name: str | None = None,
    archived: bool | None = None,
) -> dict[str, Any]:
    return project_state.update_node(
        node_id,
        display_name=display_name,
        archived=archived,
    )


def activate(
    node_id: str,
    *,
    conn: sqlite3.Connection | None = None,
    rebuild_cache: bool = True,
) -> dict[str, Any]:
    node_id = (node_id or "").strip()
    if not node_id:
        raise ValueError("node_id is required")
    project_state.set_active_node(node_id)
    if rebuild_cache:
        close_conn = conn is None
        conn = conn or db.connect()
        try:
            project_state.rebuild_sqlite_cache(conn)
        finally:
            if close_conn:
                conn.close()
    return {"ok": True, **summary()}


def copy_settings(source_node_id: str, section: str) -> dict[str, Any]:
    source = (source_node_id or "").strip()
    if not source:
        raise ValueError("source_node_id is required")
    section = (section or "").strip()
    return project_state.copy_node_settings(source, section)


def record_settings_copy(
    conn: sqlite3.Connection,
    *,
    source_node_id: str,
    section: str,
) -> dict[str, str]:
    settings = db.list_settings(conn)
    activity.append(
        conn,
        message=(
            f"Copied {section} settings from "
            f"{project_state.gap_node_display(source_node_id)}"
        ),
        severity="info",
        category="settings",
        actor="refine",
    )
    return settings


def transfer_gaps(
    source_node_id: str | None,
    target_node_id: str,
    *,
    statuses: set[str] | None = None,
    gap_ids: set[str] | None = None,
) -> dict[str, Any]:
    return project_state.transfer_gaps(
        source_node_id,
        target_node_id,
        statuses=statuses,
        gap_ids=gap_ids,
    )

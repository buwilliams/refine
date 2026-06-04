"""Shared Feature operations for API, CLI, and scheduler paths."""
from __future__ import annotations

import json
import os
import sqlite3
import tempfile
from pathlib import Path
from typing import Any

from refine_server import db, gap_ops, gap_writer, gaps as shared_gaps, project_state
from refine_server.backend_protocol import M_FEATURE_WORKFLOW_MOVE
from refine_server.gap_ops import RunnerCall, page_bounds
from refine_server.paths import feature_dir, feature_json_path, relative_feature_path
from refine_server.ulid import new_ulid

TERMINAL_STATUSES = {"done", "cancelled"}
FEATURE_WORKFLOW_TARGETS = {"backlog", "todo"}
FEATURE_WORKFLOW_PROTECTED_STATUSES = {
    "review",
    "done",
    "ready-merge",
    "awaiting-rebuild",
}
FEATURE_CANCEL_STATUSES = {
    "backlog",
    "todo",
    "in-progress",
    "qa",
    "ready-merge",
    "awaiting-rebuild",
    "review",
    "failed",
}
FEATURE_SORT_EXPRESSIONS = {
    "name": "name",
    "reporter": "reporter",
    "node": "node_id",
    "updated": "updated",
    "created": "created",
    "id": "id",
}


def _conn() -> sqlite3.Connection:
    conn = db.connect()
    project_state.ensure_sqlite_cache_current(conn)
    return conn


def create_feature(body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    name = str(body.get("name") or "").strip()
    if not name:
        return _err(400, "name is required")
    node_id = str(body.get("node_id") or project_state.active_node_id()).strip()
    if project_state.node_by_id(node_id) is None:
        return _err(400, f"unknown node: {node_id}")
    active = project_state.active_node_id()
    if node_id != active:
        return _ownership_error(node_id, "Feature")
    feature = empty_feature(
        feature_id=str(body.get("id") or new_ulid()).upper(),
        name=name,
        description=str(body.get("description") or "").strip(),
        reporter=str(body.get("reporter") or "").strip(),
        node_id=node_id,
    )
    write_feature_json(feature)
    conn = _conn()
    try:
        with db.transaction(conn):
            upsert_feature_index(conn, feature)
    finally:
        conn.close()
    return 201, {"feature": feature}


def list_features(
    *,
    status: str | None = None,
    q: str | None = None,
    reporter: str | None = None,
    node: str | None = None,
    limit: int = 50,
    offset: int = 0,
    sort: str | None = None,
    direction: str | None = None,
) -> tuple[int, dict[str, Any]]:
    page_limit, page_offset = page_bounds(limit, offset)
    conn = _conn()
    try:
        storage_sort = None if (sort or "").lower() == "status" else sort
        rows = _feature_rows(conn, q=q, reporter=reporter, node=node,
                             sort=storage_sort, direction=direction)
        features = [enrich_feature_row(conn, row, include_gaps=False) for row in rows]
    finally:
        conn.close()
    if status:
        features = [f for f in features if f.get("status") == status]
    if (sort or "").lower() == "status":
        reverse = (direction or "ASC").upper() == "DESC"
        features.sort(key=lambda f: (str(f.get("status") or ""), str(f.get("id") or "")), reverse=reverse)
    total = len(features)
    page = features[page_offset:page_offset + page_limit]
    return 200, {
        "features": page,
        "page": {
            "limit": page_limit,
            "offset": page_offset,
            "has_more": page_offset + len(page) < total,
            "total": total,
        },
    }


def get_feature(feature_id: str) -> tuple[int, dict[str, Any]]:
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT id, name, description, reporter, node_id, created, updated, json_path "
            "FROM features_index WHERE id = ?",
            (feature_id.upper(),),
        ).fetchone()
        if row is None:
            return _err(404, "Feature not found")
        feature = enrich_feature_row(conn, dict(row), include_gaps=True)
    finally:
        conn.close()
    durable = read_feature_json(feature_id) or {}
    feature.update({
        "description": str(durable.get("description") or feature.get("description") or ""),
    })
    return 200, {"feature": feature}


def update_feature(feature_id: str, body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    feature_id = feature_id.upper()
    conn = _conn()
    try:
        row = _require_feature(conn, feature_id)
        if isinstance(row, tuple):
            return row
        owner_err = _require_active_node(row["node_id"], "Feature")
        if owner_err is not None:
            return owner_err
        feature = read_feature_json(feature_id) or dict(row)
        if "name" in body:
            name = str(body.get("name") or "").strip()
            if not name:
                return _err(400, "name is required")
            feature["name"] = name
        if "description" in body:
            feature["description"] = str(body.get("description") or "").strip()
        if "reporter" in body:
            feature["reporter"] = str(body.get("reporter") or "").strip()
        feature["updated"] = shared_gaps.now_iso()
        write_feature_json(feature)
        with db.transaction(conn):
            upsert_feature_index(conn, feature)
        return 200, {"feature": enrich_feature_row(conn, feature, include_gaps=True)}
    finally:
        conn.close()


def delete_feature(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    feature_id: str,
) -> tuple[int, dict[str, Any]]:
    """Cascade delete a Feature and its associated Gaps."""
    feature_id = feature_id.upper()
    feature = _require_feature(conn, feature_id)
    if isinstance(feature, tuple):
        return feature
    owner_err = _require_active_node(feature["node_id"], "Feature")
    if owner_err is not None:
        return owner_err
    gaps = _ordered_gap_rows(conn, feature_id)
    for gap in gaps:
        if str(gap.get("node_id") or "") != str(feature["node_id"]):
            return _err(409, "Feature contains a Gap owned by another node")
    gap_ids = [str(gap["id"]) for gap in gaps]
    result = {
        "deleted": 0,
        "ids": [],
        "failures": [],
        "failed": 0,
        "progress": {"completed": 0, "total": len(gap_ids)},
    }
    if gap_ids:
        status, result = gap_ops.bulk_delete_gaps(
            conn,
            runner_call,
            {"selected_ids": gap_ids},
        )
        if status >= 400 or int(result.get("failed") or 0):
            return status, {"feature_id": feature_id, "delete": result}
    remove_feature_record(conn, feature_id)
    return 200, {"deleted": True, "feature_id": feature_id, "gaps": result}


def cancel_feature(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    feature_id: str,
) -> tuple[int, dict[str, Any]]:
    """Cascade cancel non-terminal Gaps in a Feature."""
    feature_id = feature_id.upper()
    feature = _require_feature(conn, feature_id)
    if isinstance(feature, tuple):
        return feature
    owner_err = _require_active_node(feature["node_id"], "Feature")
    if owner_err is not None:
        return owner_err
    gaps = _ordered_gap_rows(conn, feature_id)
    cancelled_ids: list[str] = []
    skipped_ids: list[str] = []
    failures: list[dict[str, Any]] = []
    for gap in gaps:
        gap_id = str(gap["id"])
        if str(gap.get("node_id") or "") != str(feature["node_id"]):
            failures.append({"id": gap_id, "error": "Gap is owned by another node"})
            continue
        status = str(gap.get("status") or "")
        if status == "done":
            skipped_ids.append(gap_id)
            continue
        if status not in FEATURE_CANCEL_STATUSES:
            skipped_ids.append(gap_id)
            continue
        code, payload = gap_ops.cancel(conn, runner_call, gap_id)
        if code >= 400:
            failures.append({
                "id": gap_id,
                "status": code,
                "error": payload.get("error", {}).get("message", "cancel failed"),
            })
        else:
            cancelled_ids.append(gap_id)
    if failures:
        return 409, {
            "feature_id": feature_id,
            "cancelled": len(cancelled_ids),
            "cancelled_ids": cancelled_ids,
            "skipped_ids": skipped_ids,
            "failures": failures,
        }
    status, body = get_feature(feature_id)
    feature_body = body.get("feature") if status < 400 else None
    return 200, {
        "feature_id": feature_id,
        "cancelled": len(cancelled_ids),
        "cancelled_ids": cancelled_ids,
        "skipped_ids": skipped_ids,
        "feature": feature_body,
    }


def move_feature_workflow(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    feature_id: str,
    target_status: str,
) -> tuple[int, dict[str, Any]]:
    """Move eligible Gaps in a Feature to backlog or todo."""
    feature_id = feature_id.upper()
    target = str(target_status or "").strip().lower()
    if target not in FEATURE_WORKFLOW_TARGETS:
        return _err(400, "status must be one of backlog or todo")
    feature = _require_feature(conn, feature_id)
    if isinstance(feature, tuple):
        return feature
    owner_err = _require_active_node(feature["node_id"], "Feature")
    if owner_err is not None:
        return owner_err

    selected_ids: list[str] = []
    skipped: list[dict[str, str]] = []
    failures: list[dict[str, str]] = []
    for gap in _ordered_gap_rows(conn, feature_id):
        gap_id = str(gap["id"]).upper()
        if str(gap.get("node_id") or "") != str(feature["node_id"]):
            failures.append({"id": gap_id, "error": "Gap is owned by another node"})
            continue
        status = str(gap.get("status") or "").lower()
        if status in FEATURE_WORKFLOW_PROTECTED_STATUSES:
            skipped.append({"id": gap_id, "reason": f"status:{status}"})
            continue
        selected_ids.append(gap_id)

    if failures:
        return 409, {
            "feature_id": feature_id,
            "status": target,
            "updated": 0,
            "ids": [],
            "skipped": len(skipped),
            "skipped_details": skipped,
            "failed": len(failures),
            "failures": failures,
        }
    if not selected_ids:
        status, body = get_feature(feature_id)
        feature_body = body.get("feature") if status < 400 else None
        return 200, {
            "feature_id": feature_id,
            "status": target,
            "updated": 0,
            "ids": [],
            "skipped": len(skipped),
            "skipped_details": skipped,
            "failed": 0,
            "failures": [],
            "feature": feature_body,
        }

    result = runner_call(
        M_FEATURE_WORKFLOW_MOVE,
        {
            "feature_id": feature_id,
            "status": target,
            "gap_ids": selected_ids,
        },
        max(30.0, min(300.0, len(selected_ids) / 10)),
    )
    result["feature_id"] = feature_id
    result["status"] = target
    result["skipped"] = int(result.get("skipped") or 0) + len(skipped)
    result["skipped_details"] = [
        *skipped,
        *list(result.get("skipped_details") or []),
    ]
    status, body = get_feature(feature_id)
    if status < 400:
        result["feature"] = body.get("feature")
    return (409 if int(result.get("failed") or 0) else 200), result


def assign_gap(feature_id: str, gap_id: str) -> tuple[int, dict[str, Any]]:
    feature_id = feature_id.upper()
    gap_id = gap_id.upper()
    conn = _conn()
    try:
        feature = _require_feature(conn, feature_id)
        if isinstance(feature, tuple):
            return feature
        gap = _require_gap(conn, gap_id)
        if isinstance(gap, tuple):
            return gap
        owner_err = _require_active_node(feature["node_id"], "Feature")
        if owner_err is not None:
            return owner_err
        if str(gap["node_id"]) != str(feature["node_id"]):
            return _err(409, "Gap and Feature must be owned by the same node")
        old_feature = str(gap["feature_id"] or "")
        if old_feature == feature_id:
            return get_feature(feature_id)
        next_order = _next_feature_order(conn, feature_id)
        _set_gap_membership(conn, gap_id, feature_id, next_order)
        if old_feature and old_feature != feature_id:
            _compact_feature_orders(conn, old_feature)
        _compact_feature_orders(conn, feature_id)
        return get_feature(feature_id)
    finally:
        conn.close()


def bulk_assign_gaps(feature_id: str, body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
    feature_id = feature_id.upper()
    conn = _conn()
    try:
        feature = _require_feature(conn, feature_id)
        if isinstance(feature, tuple):
            return feature
        owner_err = _require_active_node(feature["node_id"], "Feature")
        if owner_err is not None:
            return owner_err
        code, selected = gap_ops.select_bulk_update_candidates(
            conn,
            body.get("filter") or {},
            set(body.get("exclude_ids") or []),
            skip_automated=False,
            selected_ids=gap_ops.selected_gap_ids(body),
        )
        if code != 200:
            return code, selected
        rows = selected.get("gaps") or []
        skipped_details = list(selected.get("skipped_details") or [])
        if not rows:
            return 200, {
                "feature_id": feature_id,
                "updated": 0,
                "ids": [],
                "skipped": len(skipped_details),
                "skipped_details": skipped_details,
            }
        next_order = _next_feature_order(conn, feature_id)
        moved_ids: list[str] = []
        old_features: set[str] = set()
        with db.transaction(conn):
            for row in rows:
                gap_id = str(row["id"]).upper()
                if str(row.get("node_id") or "") != str(feature["node_id"]):
                    skipped_details.append({
                        "id": gap_id,
                        "reason": "different-node",
                    })
                    continue
                old_feature = str(row.get("feature_id") or "").upper()
                if old_feature == feature_id:
                    skipped_details.append({
                        "id": gap_id,
                        "reason": "already-assigned",
                    })
                    continue
                _set_gap_membership(conn, gap_id, feature_id, next_order)
                moved_ids.append(gap_id)
                next_order += 1
                if old_feature:
                    old_features.add(old_feature)
            for old_feature in sorted(old_features):
                _compact_feature_orders(conn, old_feature)
        return 200, {
            "feature_id": feature_id,
            "updated": len(moved_ids),
            "ids": moved_ids,
            "skipped": len(skipped_details),
            "skipped_details": skipped_details,
        }
    finally:
        conn.close()


def remove_gap(feature_id: str, gap_id: str) -> tuple[int, dict[str, Any]]:
    feature_id = feature_id.upper()
    gap_id = gap_id.upper()
    conn = _conn()
    try:
        feature = _require_feature(conn, feature_id)
        if isinstance(feature, tuple):
            return feature
        owner_err = _require_active_node(feature["node_id"], "Feature")
        if owner_err is not None:
            return owner_err
        gap = _require_gap(conn, gap_id)
        if isinstance(gap, tuple):
            return gap
        if str(gap["feature_id"] or "") != feature_id:
            return _err(409, "Gap is not assigned to this Feature")
        _set_gap_membership(conn, gap_id, None, None)
        _compact_feature_orders(conn, feature_id)
        return get_feature(feature_id)
    finally:
        conn.close()


def reorder_gap(
    feature_id: str,
    gap_id: str,
    *,
    before: str | None = None,
    after: str | None = None,
) -> tuple[int, dict[str, Any]]:
    feature_id = feature_id.upper()
    gap_id = gap_id.upper()
    before = before.upper() if before else None
    after = after.upper() if after else None
    if before and after:
        return _err(400, "Specify only one of before or after")
    conn = _conn()
    try:
        feature = _require_feature(conn, feature_id)
        if isinstance(feature, tuple):
            return feature
        owner_err = _require_active_node(feature["node_id"], "Feature")
        if owner_err is not None:
            return owner_err
        rows = _ordered_gap_rows(conn, feature_id)
        ids = [str(row["id"]) for row in rows]
        if gap_id not in ids:
            return _err(404, "Gap is not assigned to this Feature")
        if before and before not in ids:
            return _err(404, "before Gap is not assigned to this Feature")
        if after and after not in ids:
            return _err(404, "after Gap is not assigned to this Feature")
        ids.remove(gap_id)
        if before:
            ids.insert(ids.index(before), gap_id)
        elif after:
            ids.insert(ids.index(after) + 1, gap_id)
        else:
            ids.append(gap_id)
        for order, ordered_gap_id in enumerate(ids, start=1):
            _set_gap_membership(conn, ordered_gap_id, feature_id, order)
        return get_feature(feature_id)
    finally:
        conn.close()


def candidate_gaps(feature_id: str, *, limit: int = 50, offset: int = 0) -> tuple[int, dict[str, Any]]:
    feature_id = feature_id.upper()
    page_limit, page_offset = page_bounds(limit, offset)
    conn = _conn()
    try:
        feature = _require_feature(conn, feature_id)
        if isinstance(feature, tuple):
            return feature
        rows = [
            dict(row) for row in conn.execute(
                "SELECT id, name, status, priority, reporter, round_count, created, updated, "
                "branch_name, node_id, feature_id, feature_order "
                "FROM gaps_index WHERE node_id = ? AND feature_id IS NULL "
                "ORDER BY updated DESC LIMIT ? OFFSET ?",
                (feature["node_id"], page_limit + 1, page_offset),
            )
        ]
    finally:
        conn.close()
    has_more = len(rows) > page_limit
    return 200, {
        "gaps": rows[:page_limit],
        "page": {"limit": page_limit, "offset": page_offset, "has_more": has_more},
    }


def empty_feature(
    *,
    feature_id: str,
    name: str,
    description: str = "",
    reporter: str = "",
    node_id: str | None = None,
) -> dict[str, Any]:
    now = shared_gaps.now_iso()
    return {
        "id": feature_id,
        "name": name,
        "description": description,
        "reporter": reporter,
        "node_id": node_id or project_state.active_node_id(),
        "created": now,
        "updated": now,
        "json_path": relative_feature_path(feature_id),
    }


def read_feature_json(feature_id: str) -> dict[str, Any] | None:
    path = feature_json_path(feature_id)
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(data, dict):
        return None
    data.setdefault("description", "")
    data.setdefault("reporter", "")
    data.setdefault("node_id", project_state.DEFAULT_NODE_ID)
    data.setdefault("json_path", relative_feature_path(str(data.get("id") or feature_id)))
    return data


def write_feature_json(feature: dict[str, Any]) -> None:
    feature_id = str(feature["id"]).upper()
    feature["id"] = feature_id
    feature["json_path"] = relative_feature_path(feature_id)
    directory = feature_dir(feature_id)
    directory.mkdir(parents=True, exist_ok=True)
    path = feature_json_path(feature_id)
    data = json.dumps(feature, ensure_ascii=False, indent=2).encode("utf-8")
    fd, tmp = tempfile.mkstemp(prefix=".feature.", suffix=".tmp", dir=str(directory))
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp, path)
    except Exception:
        try:
            os.unlink(tmp)
        except FileNotFoundError:
            pass
        raise


def upsert_feature_index(conn: sqlite3.Connection, feature: dict[str, Any]) -> None:
    conn.execute(
        "INSERT OR REPLACE INTO features_index "
        "(id, name, description, reporter, node_id, created, updated, json_path) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (
            str(feature.get("id") or ""),
            str(feature.get("name") or "Untitled Feature"),
            str(feature.get("description") or ""),
            str(feature.get("reporter") or ""),
            str(feature.get("node_id") or project_state.DEFAULT_NODE_ID),
            str(feature.get("created") or shared_gaps.now_iso()),
            str(feature.get("updated") or feature.get("created") or shared_gaps.now_iso()),
            str(feature.get("json_path") or relative_feature_path(str(feature.get("id") or ""))),
        ),
    )


def remove_feature_record(conn: sqlite3.Connection, feature_id: str) -> None:
    feature_id = feature_id.upper()
    try:
        feature_json_path(feature_id).unlink()
    except FileNotFoundError:
        pass
    except OSError:
        pass
    conn.execute("DELETE FROM features_index WHERE id = ?", (feature_id,))


def enrich_feature_row(
    conn: sqlite3.Connection,
    row: dict[str, Any],
    *,
    include_gaps: bool,
) -> dict[str, Any]:
    feature = dict(row)
    gaps = _ordered_gap_rows(conn, str(row["id"]))
    rollup = derive_feature_rollup(gaps)
    feature.update(rollup)
    feature["node_display_name"] = project_state.gap_node_display(feature.get("node_id"))
    if include_gaps:
        feature["gaps"] = gaps
    return feature


def derive_feature_rollup(gaps: list[dict[str, Any]]) -> dict[str, Any]:
    gap_count = len(gaps)
    done_count = sum(1 for gap in gaps if gap.get("status") == "done")
    cancelled_count = sum(1 for gap in gaps if gap.get("status") == "cancelled")
    failed_count = sum(1 for gap in gaps if gap.get("status") == "failed")
    active_count = sum(1 for gap in gaps if gap.get("status") in {"in-progress", "qa", "ready-merge", "awaiting-rebuild", "review"})
    next_gap = next((gap for gap in gaps if gap.get("status") not in TERMINAL_STATUSES), None)
    if gap_count == 0:
        status = "backlog"
    elif done_count == gap_count:
        status = "done"
    elif done_count + cancelled_count == gap_count and cancelled_count:
        status = "cancelled"
    elif next_gap:
        status = str(next_gap.get("status") or "backlog")
    else:
        status = "backlog"
    blocked_count = 0
    if next_gap and next_gap.get("status") not in TERMINAL_STATUSES:
        seen_next = False
        for gap in gaps:
            if gap["id"] == next_gap["id"]:
                seen_next = True
                continue
            if seen_next and gap.get("status") not in TERMINAL_STATUSES:
                blocked_count += 1
    return {
        "status": status,
        "gap_count": gap_count,
        "done_count": done_count,
        "active_count": active_count,
        "failed_count": failed_count,
        "cancelled_count": cancelled_count,
        "blocked_count": blocked_count,
        "next_gap": next_gap,
    }


def _feature_rows(
    conn: sqlite3.Connection,
    *,
    q: str | None,
    reporter: str | None,
    node: str | None,
    sort: str | None,
    direction: str | None,
) -> list[dict[str, Any]]:
    sql = [
        "SELECT id, name, description, reporter, node_id, created, updated, json_path "
        "FROM features_index"
    ]
    where: list[str] = []
    args: list[Any] = []
    if q:
        where.append("(name LIKE ? OR description LIKE ?)")
        like = f"%{q}%"
        args.extend([like, like])
    if reporter:
        where.append("reporter = ?")
        args.append(reporter)
    if node:
        if node == "current":
            where.append("node_id = ?")
            args.append(project_state.active_node_id())
        elif node != "all":
            where.append("node_id = ?")
            args.append(node)
    if where:
        sql.append("WHERE " + " AND ".join(where))
    key = (sort or "updated").lower()
    expr = FEATURE_SORT_EXPRESSIONS.get(key, "updated")
    order_dir = (direction or "DESC").upper()
    if order_dir not in {"ASC", "DESC"}:
        order_dir = "DESC"
    sql.append(f"ORDER BY {expr} {order_dir}, id DESC")
    return [dict(row) for row in conn.execute(" ".join(sql), args)]


def _ordered_gap_rows(conn: sqlite3.Connection, feature_id: str) -> list[dict[str, Any]]:
    return [
        dict(row) for row in conn.execute(
            "SELECT id, name, status, priority, reporter, round_count, created, updated, "
            "branch_name, node_id, feature_id, feature_order "
            "FROM gaps_index WHERE feature_id = ? "
            "ORDER BY feature_order ASC, updated ASC, id ASC",
            (feature_id,),
        )
    ]


def _require_feature(conn: sqlite3.Connection, feature_id: str) -> dict[str, Any] | tuple[int, dict[str, Any]]:
    row = conn.execute(
        "SELECT id, name, description, reporter, node_id, created, updated, json_path "
        "FROM features_index WHERE id = ?",
        (feature_id,),
    ).fetchone()
    if row is None:
        return _err(404, "Feature not found")
    return dict(row)


def _require_gap(conn: sqlite3.Connection, gap_id: str) -> dict[str, Any] | tuple[int, dict[str, Any]]:
    row = conn.execute(
        "SELECT id, name, status, priority, reporter, round_count, created, updated, "
        "branch_name, node_id, feature_id, feature_order "
        "FROM gaps_index WHERE id = ?",
        (gap_id,),
    ).fetchone()
    if row is None:
        return _err(404, "Gap not found")
    return dict(row)


def _next_feature_order(conn: sqlite3.Connection, feature_id: str) -> int:
    row = conn.execute(
        "SELECT COALESCE(MAX(feature_order), 0) AS max_order "
        "FROM gaps_index WHERE feature_id = ?",
        (feature_id,),
    ).fetchone()
    return int(row["max_order"] or 0) + 1


def _compact_feature_orders(conn: sqlite3.Connection, feature_id: str) -> None:
    for order, row in enumerate(_ordered_gap_rows(conn, feature_id), start=1):
        _set_gap_membership(conn, str(row["id"]), feature_id, order)


def _set_gap_membership(
    conn: sqlite3.Connection,
    gap_id: str,
    feature_id: str | None,
    feature_order: int | None,
) -> None:
    gap_writer.update_fields(
        gap_id,
        feature_id=feature_id,
        feature_order=feature_order,
    )
    conn.execute(
        "UPDATE gaps_index SET feature_id = ?, feature_order = ?, updated = ? WHERE id = ?",
        (feature_id, feature_order, shared_gaps.now_iso(), gap_id),
    )


def _require_active_node(owner_id: str | None, subject: str) -> tuple[int, dict[str, Any]] | None:
    owner = str(owner_id or project_state.DEFAULT_NODE_ID)
    if owner == project_state.active_node_id():
        return None
    return _ownership_error(owner, subject)


def _ownership_error(owner_id: str, subject: str) -> tuple[int, dict[str, Any]]:
    owner_name = project_state.gap_node_display(owner_id)
    active = project_state.active_node_id()
    active_name = project_state.gap_node_display(active)
    return _err(
        409,
        f"Action not allowed: {subject} is owned by another node ({owner_name}).",
        {
            "owner_node_id": owner_id,
            "owner_node_display_name": owner_name,
            "active_node_id": active,
            "active_node_display_name": active_name,
        },
    )


def _err(status: int, message: str, extra: dict[str, Any] | None = None) -> tuple[int, dict[str, Any]]:
    body: dict[str, Any] = {"error": {"message": message}}
    if extra:
        body["error"].update(extra)
    return status, body

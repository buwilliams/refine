"""Shared Gap and Changes operations for API and CLI."""
from __future__ import annotations

from dataclasses import dataclass
import re
import sqlite3
from typing import Any, Callable

from refine_server import activity, db, gaps as shared_gaps, project_state
from refine_server import gap_writer, round_logs, search_index
from refine_server import perf_metrics
from refine_server.backend_protocol import (
    M_APPEND_ROUND,
    M_BULK_DELETE_GAPS,
    M_BULK_UPDATE_GAPS,
    M_CANCEL,
    M_DELETE_GAP,
    M_EDIT_ROUND,
    M_ENFORCE_SCHEDULING,
    M_LIST_CHANGES,
    M_PREFLIGHT,
    M_RETRY_MERGE,
    M_RETRY_QA,
    M_SET_NOTES,
    M_UNDO_GAP,
    M_VERIFY,
)
from refine_server.gaps import now_iso


RunnerCall = Callable[[str, dict[str, Any], float], dict[str, Any]]

VALID_STATUSES = (
    "backlog", "todo", "in-progress", "qa", "ready-merge", "awaiting-rebuild",
    "review", "done", "failed", "cancelled",
)
VALID_PRIORITIES = ("low", "medium", "high")
USER_STATUS_TRANSITIONS = {
    "backlog": {"todo"},
    "todo": {"backlog"},
    "review": {"todo"},
    "done": {"review"},
    "failed": {"todo"},
    "cancelled": {"todo"},
}
BULK_STATUS_AUTOMATED_VALUES = {"in-progress", "qa", "ready-merge"}
BULK_STATUS_VALUES = set(VALID_STATUSES) - BULK_STATUS_AUTOMATED_VALUES
BULK_STATUS_SOURCE_VALUES = BULK_STATUS_VALUES
BULK_LAST_WORKFLOW_STATUS = "__last_workflow_state"
VALID_REPORTER = re.compile(r"^[^\x00-\x1f]{1,80}$")

GAPS_SORT_EXPRESSIONS: dict[str, str] = {
    "name": "name COLLATE NOCASE",
    "status": "status",
    "priority": "CASE priority WHEN 'high' THEN 0 WHEN 'medium' THEN 1 ELSE 2 END",
    "reporter": "reporter COLLATE NOCASE",
    "rounds": "round_count",
    "node": "node_id COLLATE NOCASE",
    "updated": "updated",
    "created": "created",
    "id": "id",
}

GAPS_DEFAULT_DIR: dict[str, str] = {
    "name": "ASC",
    "status": "ASC",
    "priority": "ASC",
    "reporter": "ASC",
    "rounds": "ASC",
    "node": "ASC",
    "updated": "DESC",
    "created": "DESC",
    "id": "DESC",
}


@dataclass(frozen=True)
class BulkUpdatePlan:
    field: str
    value: str
    selected_gaps: list[dict[str, Any]]
    skipped_details: list[dict[str, str]]

    @property
    def gap_ids(self) -> list[str]:
        return [str(g["id"]) for g in self.selected_gaps]


def _conn() -> sqlite3.Connection:
    conn = db.connect()
    project_state.ensure_sqlite_cache_current(conn)
    return conn


def page_bounds(limit: int, offset: int = 0) -> tuple[int, int]:
    return max(1, int(limit)), max(0, int(offset))


def empty_page(limit: int, offset: int) -> dict[str, Any]:
    page_limit, page_offset = page_bounds(limit, offset)
    return {
        "limit": page_limit,
        "offset": page_offset,
        "has_more": False,
        "total": 0,
    }


def gaps_order_clause(sort: str | None, direction: str | None) -> str:
    key = (sort or "updated").lower()
    if key not in GAPS_SORT_EXPRESSIONS:
        key = "updated"
    expr = GAPS_SORT_EXPRESSIONS[key]
    order_dir = (direction or "").upper()
    if order_dir not in ("ASC", "DESC"):
        order_dir = GAPS_DEFAULT_DIR[key]
    tiebreaker = "" if key == "updated" else ", updated DESC"
    return f"{expr} {order_dir}{tiebreaker}"


def round_count_bounds(
    rounds_gte: Any | None,
    rounds_lte: Any | None,
) -> tuple[int | None, int | None, tuple[int, dict] | None]:
    lower, lower_err = _parse_round_bound(rounds_gte, "rounds_gte")
    if lower_err is not None:
        return None, None, lower_err
    upper, upper_err = _parse_round_bound(rounds_lte, "rounds_lte")
    if upper_err is not None:
        return None, None, upper_err
    return lower, upper, None


def _parse_round_bound(value: Any | None, name: str) -> tuple[int | None, tuple[int, dict] | None]:
    if value is None:
        return None, None
    text = str(value).strip()
    if not text:
        return None, None
    if not re.fullmatch(r"\d+", text):
        return None, _err(400, f"{name} must be a non-negative integer")
    return int(text), None


def list_gaps(
    *,
    attached: bool = True,
    status: str | None = None,
    q: str | None = None,
    severity: str | None = None,
    category: str | None = None,
    actor: str | None = None,
    reporter: str | None = None,
    feature: str | None = None,
    rounds_gte: Any | None = None,
    rounds_lte: Any | None = None,
    node: str | None = None,
    limit: int = 50,
    offset: int = 0,
    sort: str | None = None,
    direction: str | None = None,
    include_facets: bool = False,
) -> tuple[int, dict]:
    if not attached:
        body: dict[str, Any] = {
            "gaps": [],
            "page": empty_page(limit, offset),
            "attached": False,
        }
        if include_facets:
            body["facets"] = {"categories": [], "actors": []}
        return 200, body
    metric_start = perf_metrics.now()
    page_limit, page_offset = page_bounds(limit, offset)
    min_rounds, max_rounds, bounds_err = round_count_bounds(rounds_gte, rounds_lte)
    if bounds_err is not None:
        return bounds_err
    fts_match = search_index.fts_query(q)
    sql = [
        "SELECT id, name, status, priority, reporter, "
        "round_count, created, updated, branch_name, node_id, feature_id, feature_order "
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
    if feature:
        feature_value = str(feature).strip()
        if feature_value == "standalone":
            where.append("feature_id IS NULL")
        elif feature_value != "all":
            where.append("feature_id = ?")
            args.append(feature_value.upper())
    if min_rounds is not None:
        where.append("round_count >= ?")
        args.append(min_rounds)
    if max_rounds is not None:
        where.append("round_count <= ?")
        args.append(max_rounds)
    if node:
        if node == "current":
            where.append("node_id = ?")
            args.append(project_state.active_node_id())
        elif node == "unknown":
            known = [i.get("id") for i in project_state.list_nodes()]
            if known:
                where.append(
                    "(node_id = '' OR node_id NOT IN ("
                    + ",".join("?" * len(known)) + "))"
                )
                args.extend(known)
            else:
                where.append("1 = 1")
        elif node != "all":
            where.append("node_id = ?")
            args.append(node)
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
    sql.append("ORDER BY " + gaps_order_clause(sort, direction))
    sql.append("LIMIT ? OFFSET ?")
    args.extend([page_limit + 1, page_offset])
    conn = _conn()
    try:
        rows = [enrich_gap_row(dict(r)) for r in conn.execute(" ".join(sql), args)]
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
    body: dict[str, Any] = {
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
            "feature": feature or "",
            "rounds_gte": "" if min_rounds is None else min_rounds,
            "rounds_lte": "" if max_rounds is None else max_rounds,
            "node": node or "",
            "limit": page_limit,
            "offset": page_offset,
            "sort": sort or "",
            "direction": direction or "",
        },
    )
    return 200, body


def get_gap(gap_id: str) -> tuple[int, dict]:
    metric_start = perf_metrics.now()
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT id, name, status, priority, created, updated, branch_name, node_id, "
            "feature_id, feature_order "
            "FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if not row:
            return _err(404, "Gap not found")
    finally:
        conn.close()
    gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {
        "id": gap_id,
        "name": row["name"],
        "rounds": [],
        "created": row["created"],
        "updated": row["updated"],
    }
    gap.pop("_refine_embedded_round_logs", None)
    gap = dict(gap)
    gap["status"] = row["status"]
    gap["priority"] = row["priority"] or "low"
    gap["branch_name"] = row["branch_name"]
    gap["node_id"] = row["node_id"]
    gap["feature_id"] = row["feature_id"]
    gap["feature_order"] = row["feature_order"]
    gap["node_display_name"] = project_state.gap_node_display(row["node_id"])
    rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
    log_counts = round_logs.count_by_round(gap_id, len(rounds))
    for idx, round_obj in enumerate(rounds):
        round_obj["log_count"] = log_counts.get(idx, 0)
        latest_log, latest_error_log = round_logs.latest_for_round(gap_id, idx)
        latest_state_log = round_logs.latest_state_for_round(gap_id, idx)
        latest_workflow_log = round_logs.latest_workflow_for_round(gap_id, idx)
        if latest_log:
            round_obj["latest_log"] = compact_log(latest_log)
        if latest_error_log:
            round_obj["latest_error_log"] = compact_log(latest_error_log)
        if latest_state_log:
            round_obj["latest_state_log"] = compact_log(latest_state_log)
        if latest_workflow_log:
            round_obj["latest_workflow_log"] = compact_log(latest_workflow_log)
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


def get_gap_logs(
    gap_id: str,
    *,
    round_idx: int,
    limit: int = 50,
    offset: int = 0,
) -> tuple[int, dict]:
    limit = max(1, min(int(limit), 200))
    offset = max(0, int(offset))
    metric_start = perf_metrics.now()
    page_limit, page_offset = page_bounds(limit, offset)
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT id FROM gaps_index WHERE id = ?", (gap_id,),
        ).fetchone()
        if not row:
            return _err(404, "Gap not found")
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            return _err(404, "Gap not found")
        rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
        if round_idx < 0 or round_idx >= len(rounds):
            return _err(404, "Round not found")
        round_log_count = round_logs.count_by_round(gap_id, len(rounds)).get(round_idx, 0)
        entries, has_more = round_logs.page_round_logs(
            gap_id,
            round_idx,
            limit=page_offset + page_limit,
            offset=0,
        )
        activity_logs = activity_for_round(conn, gap_id, rounds, round_idx)
    finally:
        conn.close()

    merged = [
        *mark_log_source(entries, "round"),
        *mark_log_source(activity_logs, "activity"),
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


def list_changes(
    runner_call: RunnerCall,
    *,
    attached: bool = True,
    limit: int = 50,
    offset: int = 0,
    q: str | None = None,
    status: str | None = None,
    priority: str | None = None,
) -> tuple[int, dict]:
    page_limit, page_offset = page_bounds(limit, offset)
    if not attached:
        return 200, {
            "changes": [],
            "branch": "",
            "page": {
                "limit": page_limit,
                "offset": page_offset,
                "has_more": False,
                "total": 0,
            },
            "attached": False,
        }
    result = runner_call(
        M_LIST_CHANGES,
        {
            "limit": page_limit,
            "offset": page_offset,
            "q": q or "",
            "status": status or "",
            "priority": priority or "",
        },
        15.0,
    )
    return 200, result


def selected_gap_ids(body: dict[str, Any]) -> list[str] | None:
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


def id_chunks(values: list[str], size: int = 500) -> list[list[str]]:
    return [values[idx:idx + size] for idx in range(0, len(values), size)]


def require_active_gap_ids(
    conn: sqlite3.Connection,
    gap_ids: list[str],
) -> tuple[bool, tuple[int, dict] | None]:
    if not gap_ids:
        return True, None
    active = project_state.active_node_id()
    rows = []
    for chunk in id_chunks(gap_ids):
        placeholders = ",".join("?" * len(chunk))
        rows.extend(conn.execute(
            f"SELECT id, node_id FROM gaps_index WHERE id IN ({placeholders})",
            chunk,
        ).fetchall())
    violations = [
        node_owner(row["node_id"])
        for row in rows
        if node_owner(row["node_id"]) != active
    ]
    if violations:
        return False, ownership_error(
            sorted(set(violations))[0],
            active_id=active,
            count=len(violations),
        )
    return True, None


def select_bulk_update_candidates(
    conn: sqlite3.Connection,
    filt: dict[str, Any],
    excluded: set[str],
    *,
    skip_automated: bool,
    selected_ids: list[str] | None = None,
) -> tuple[int, dict]:
    if selected_ids is not None:
        if not selected_ids:
            return 200, {"gaps": [], "skipped_details": []}
        found: dict[str, dict[str, Any]] = {}
        for chunk in id_chunks(selected_ids):
            placeholders = ",".join("?" * len(chunk))
            rows = conn.execute(
                "SELECT id, name, status, priority, reporter, "
                "round_count, created, updated, branch_name, node_id, "
                "feature_id, feature_order "
                f"FROM gaps_index WHERE id IN ({placeholders})",
                chunk,
            ).fetchall()
            for row in rows:
                found[row["id"]] = dict(row)
        rows = [found[gap_id] for gap_id in selected_ids if gap_id in found]
        return filter_bulk_candidate_rows(rows, skip_automated=skip_automated)

    q = str(filt.get("q") or "").strip()
    fts_match = search_index.fts_query(q)
    severity = filt.get("severity") or None
    category = filt.get("category") or None
    actor = filt.get("actor") or None
    reporter = filt.get("reporter") or None
    feature = str(filt.get("feature") or "").strip()
    min_rounds, max_rounds, bounds_err = round_count_bounds(
        filt.get("rounds_gte"),
        filt.get("rounds_lte"),
    )
    if bounds_err is not None:
        return bounds_err
    sql = [
        "SELECT id, name, status, priority, reporter, "
        "round_count, created, updated, branch_name, node_id, feature_id, feature_order "
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
    if feature:
        if feature == "standalone":
            where.append("feature_id IS NULL")
        elif feature != "all":
            where.append("feature_id = ?")
            args.append(feature.upper())
    if min_rounds is not None:
        where.append("round_count >= ?")
        args.append(min_rounds)
    if max_rounds is not None:
        where.append("round_count <= ?")
        args.append(max_rounds)
    node = filt.get("node") or None
    if node:
        if node == "current":
            where.append("node_id = ?")
            args.append(project_state.active_node_id())
        elif node == "unknown":
            known = [i.get("id") for i in project_state.list_nodes()]
            if known:
                where.append(
                    "(node_id = '' OR node_id NOT IN ("
                    + ",".join("?" * len(known)) + "))"
                )
                args.extend(known)
            else:
                where.append("1 = 1")
        elif node != "all":
            where.append("node_id = ?")
            args.append(node)
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
    sql.append("ORDER BY " + gaps_order_clause(None, None))
    rows = [dict(r) for r in conn.execute(" ".join(sql), args)]
    rows = [r for r in rows if r["id"] not in excluded]
    return filter_bulk_candidate_rows(rows, skip_automated=skip_automated)


def filter_bulk_candidate_rows(
    rows: list[dict[str, Any]],
    *,
    skip_automated: bool,
) -> tuple[int, dict]:
    skipped: list[dict[str, str]] = []
    if skip_automated:
        status_order = {status: idx for idx, status in enumerate(VALID_STATUSES)}
        skipped = [
            {"id": r["id"], "reason": f"status:{r.get('status')}"}
            for r in rows
            if str(r.get("status") or "") in BULK_STATUS_AUTOMATED_VALUES
        ]
        skipped.sort(key=lambda item: (
            status_order.get(item["reason"].split(":", 1)[1], 999),
            item["id"],
        ))
        rows = [
            r for r in rows
            if str(r.get("status") or "") in BULK_STATUS_SOURCE_VALUES
        ]
    return 200, {"gaps": rows, "skipped_details": skipped}


def is_last_workflow_bulk_update(body: dict[str, Any]) -> bool:
    update = body.get("update") or {}
    raw = update.get("status")
    return (
        isinstance(raw, str)
        and raw.strip().lower() == BULK_LAST_WORKFLOW_STATUS
    )


def prepare_bulk_update(
    conn: sqlite3.Connection,
    body: dict[str, Any],
) -> tuple[int, BulkUpdatePlan | dict]:
    update = body.get("update") or {}
    update = {k: v for k, v in update.items()
              if k in ("priority", "status", "reporter")}
    if len(update) != 1:
        return _err(
            400,
            "update must contain exactly one of `priority`, `status`, or `reporter`",
        )
    field, raw = next(iter(update.items()))
    value = str(raw or "").strip()
    if field == "priority":
        value = value.lower()
        if value not in VALID_PRIORITIES:
            return _err(400, "priority must be one of low/medium/high")
    elif field == "status":
        value = value.lower()
        if value == BULK_LAST_WORKFLOW_STATUS:
            pass
        elif value not in VALID_STATUSES:
            return _err(400, "invalid status")
        elif value not in BULK_STATUS_VALUES:
            return _err(
                409,
                (
                    "Bulk status updates cannot set in-progress, qa, or ready-merge. "
                    "Use per-Gap workflow actions for automated states."
                ),
            )
    else:
        if not value or not VALID_REPORTER.match(value):
            return _err(400, "invalid reporter name")

    filt = body.get("filter") or {}
    excluded = set(body.get("exclude_ids") or [])
    code, selected = select_bulk_update_candidates(
        conn,
        filt,
        excluded,
        skip_automated=(
            field == "status" and value != BULK_LAST_WORKFLOW_STATUS
        ),
        selected_ids=selected_gap_ids(body),
    )
    if code != 200:
        return code, selected
    selected_gaps = selected["gaps"]
    skipped_details = selected["skipped_details"]
    gap_ids = [g["id"] for g in selected_gaps]
    if not gap_ids:
        return 200, {
            "updated": 0,
            "ids": [],
            "skipped": len(skipped_details),
            "skipped_details": skipped_details,
        }
    ok, ownership_err = require_active_gap_ids(conn, gap_ids)
    if not ok and ownership_err is not None:
        return ownership_err
    return 200, BulkUpdatePlan(
        field=field,
        value=value,
        selected_gaps=selected_gaps,
        skipped_details=skipped_details,
    )


def bulk_update_gaps(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    body: dict[str, Any],
) -> tuple[int, dict]:
    code, plan_or_payload = prepare_bulk_update(conn, body)
    if code != 200 or not isinstance(plan_or_payload, BulkUpdatePlan):
        return code, plan_or_payload
    result = run_bulk_update(runner_call, plan_or_payload)
    if int(result.get("http_status") or 200) >= 400:
        return int(result["http_status"]), result
    return 200, result


def run_bulk_update(
    runner_call: RunnerCall,
    plan: BulkUpdatePlan,
) -> dict[str, Any]:
    gap_ids = plan.gap_ids
    try:
        result = runner_call(
            M_BULK_UPDATE_GAPS,
            {"field": plan.field, "value": plan.value, "gap_ids": gap_ids},
            max(30.0, min(300.0, len(gap_ids) / 10)),
        )
    except Exception as e:
        code, body = backend_exception_error(e)
        return {
            "updated": 0,
            "ids": [],
            "field": plan.field,
            "value": plan.value,
            "skipped": len(plan.skipped_details),
            "skipped_details": plan.skipped_details,
            "failed": len(gap_ids),
            "failures": [{"id": gid, "error": body["error"]["message"]} for gid in gap_ids],
            "error": body["error"],
            "http_status": code,
        }
    runner_skipped_details = result.get("skipped_details") or []
    if not isinstance(runner_skipped_details, list):
        runner_skipped_details = []
    all_skipped_details = [*plan.skipped_details, *runner_skipped_details]
    return {
        "updated": int(result.get("updated") or 0),
        "ids": result.get("ids") or [],
        "field": plan.field,
        "value": plan.value,
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


def bulk_delete_gaps(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    body: dict[str, Any],
) -> tuple[int, dict]:
    filt = body.get("filter") or {}
    excluded = set(body.get("exclude_ids") or [])
    code, selected = select_bulk_update_candidates(
        conn,
        filt,
        excluded,
        skip_automated=False,
        selected_ids=selected_gap_ids(body),
    )
    if code != 200:
        return code, selected
    gap_ids = [g["id"] for g in selected["gaps"]]
    if not gap_ids:
        return 200, {"deleted": 0, "ids": [], "failures": []}
    ok, ownership_err = require_active_gap_ids(conn, gap_ids)
    if not ok and ownership_err is not None:
        return ownership_err
    try:
        result = runner_call(
            M_BULK_DELETE_GAPS,
            {"gap_ids": gap_ids},
            max(60.0, min(300.0, len(gap_ids) / 5)),
        )
    except Exception as e:
        code, body = backend_exception_error(e)
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


def update_gap(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
    body: dict[str, Any],
    *,
    background_processes_stopped: bool = False,
) -> tuple[int, dict]:
    row, ownership_err = require_active_gap(conn, gap_id)
    if ownership_err is not None:
        return ownership_err
    updates: dict[str, Any] = {}
    sql_fields: dict[str, Any] = {}
    if "name" in body:
        name = str(body.get("name") or "").strip()
        if not name:
            return _err(400, "name is required")
        updates["name"] = name
        sql_fields["name"] = name
    if "priority" in body:
        priority = str(body.get("priority") or "").strip().lower()
        if priority not in ("low", "medium", "high"):
            return _err(400, "priority must be one of low/medium/high")
        updates["priority"] = priority
        sql_fields["priority"] = priority
    if "status" in body:
        next_status = str(body.get("status") or "").strip().lower()
        if next_status not in VALID_STATUSES:
            return _err(400, "invalid status")
        transition_err = validate_user_status_transition(row["status"], next_status)
        if transition_err is not None:
            return transition_err
        updates["status"] = next_status
        sql_fields["status"] = next_status
    notes_change = "notes" in body
    if not updates and not notes_change:
        return _err(400, "expected `name`, `priority`, `status`, and/or `notes`")
    updated_at = now_iso()
    previous_status = row["status"]
    if sql_fields:
        set_parts = [f"{field} = ?" for field in sql_fields]
        args = list(sql_fields.values())
        set_parts.append("updated = ?")
        args.append(updated_at)
        args.append(gap_id)
        args.append(project_state.active_node_id())
        with db.transaction(conn):
            cur = conn.execute(
                f"UPDATE gaps_index SET {', '.join(set_parts)} "
                "WHERE id = ? AND node_id = ?",
                args,
            )
        if not cur.rowcount:
            return ownership_error(None)
        try:
            gap = gap_writer.update_fields(gap_id, **updates)
            with db.transaction(conn):
                search_index.upsert_gap(conn, gap)
                if "name" in sql_fields:
                    conn.execute(
                        "DELETE FROM guidance_decisions WHERE gap_id = ?",
                        (gap_id,),
                    )
            if "status" in sql_fields:
                append_gap_workflow_log(
                    gap_id,
                    f"Workflow status changed: {previous_status} → {sql_fields['status']}",
                )
        except Exception:
            pass
    if notes_change:
        notes = body.get("notes")
        if not isinstance(notes, list):
            return _err(400, "notes must be a list of {id, author, body, ...} objects")
        runner_call(M_SET_NOTES, {"gap_id": gap_id, "notes": notes}, 30.0)
    elif sql_fields:
        try:
            runner_call(
                M_EDIT_ROUND,
                {"gap_id": gap_id, "actual": None, "target": None, "reporter": None},
                30.0,
            )
        except Exception:
            pass
    if ("priority" in sql_fields or "status" in sql_fields) and not background_processes_stopped:
        try:
            runner_call(M_ENFORCE_SCHEDULING, {}, 10.0)
        except Exception:
            pass
    return 200, {"ok": True}


def delete_gap(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
) -> tuple[int, dict]:
    _, ownership_err = require_active_gap(conn, gap_id)
    if ownership_err is not None:
        return ownership_err
    result = runner_call(M_DELETE_GAP, {"gap_id": gap_id}, 30.0)
    return 200, result


def append_round(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
    body: dict[str, Any],
) -> tuple[int, dict]:
    reporter = (body.get("reporter") or "").strip()
    actual = (body.get("actual") or "").strip()
    target = (body.get("target") or "").strip()
    if not reporter:
        return _err(400, "reporter is required")
    if not actual and not target:
        return _err(400, "actual or target must be non-empty")
    row, ownership_err = require_active_gap(conn, gap_id, columns="status, node_id")
    if ownership_err is not None:
        return ownership_err
    if row["status"] != "review":
        return _err(
            409,
            "New rounds may only be appended from `review` "
            f"(status={row['status']}). From `todo` or `failed`, edit the "
            "latest round instead.",
        )
    result = runner_call(M_APPEND_ROUND, {
        "gap_id": gap_id,
        "reporter": reporter,
        "actual": actual,
        "target": target,
    }, 30.0)
    return 201, result


def edit_latest_round(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
    body: dict[str, Any],
) -> tuple[int, dict]:
    row, ownership_err = require_active_gap(conn, gap_id, columns="status, node_id")
    if ownership_err is not None:
        return ownership_err
    if row["status"] not in ("backlog", "todo", "failed"):
        return _err(
            409,
            "Only the latest unaddressed round can be edited "
            f"(status={row['status']})",
        )
    result = runner_call(M_EDIT_ROUND, {
        "gap_id": gap_id,
        "actual": body.get("actual"),
        "target": body.get("target"),
        "reporter": body.get("reporter"),
    }, 30.0)
    return 200, result


def verify(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
) -> tuple[int, dict]:
    _, ownership_err = require_active_gap(conn, gap_id)
    if ownership_err is not None:
        return ownership_err
    result = runner_call(M_VERIFY, {"gap_id": gap_id}, 120.0)
    return 200, result


def undo_change(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    body: dict[str, Any],
) -> tuple[int, dict]:
    commit = (body.get("commit") or "").strip()
    if not commit:
        return _err(400, "commit is required")
    from refine_server import git_ops

    gap_id = git_ops.gap_id_from_commit(commit)
    if gap_id:
        _, ownership_err = require_active_gap(conn, gap_id)
        if ownership_err is not None:
            return ownership_err
    result = runner_call(M_UNDO_GAP, {"commit": commit}, 120.0)
    return (200 if result.get("ok") else 409), result


def retry(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
) -> tuple[int, dict]:
    row, ownership_err = require_active_gap(conn, gap_id, columns="status, node_id")
    if ownership_err is not None:
        return ownership_err
    prev_status = row["status"]
    if prev_status not in ("failed", "done", "cancelled"):
        return _err(
            409,
            f"Reopen only valid from failed/done/cancelled (status={prev_status})",
        )
    last = conn.execute(
        "SELECT failure_category FROM runs WHERE gap_id = ? "
        "ORDER BY id DESC LIMIT 1",
        (gap_id,),
    ).fetchone()
    if last and last["failure_category"] == "auth":
        pf = runner_call(M_PREFLIGHT, {}, 30.0)
        if not pf.get("ok"):
            return _err(409, "Auth pre-flight still failing - Reopen blocked")
    with db.transaction(conn):
        cur = conn.execute(
            "UPDATE gaps_index SET status = 'todo', updated = ? "
            "WHERE id = ? AND node_id = ?",
            (now_iso(), gap_id, project_state.active_node_id()),
        )
    if not cur.rowcount:
        return ownership_error(None)
    try:
        gap_writer.update_fields(gap_id, status="todo")
        append_gap_workflow_log(
            gap_id,
            f"Workflow status changed: {prev_status} → todo; reopened",
        )
    except Exception:
        pass
    activity.append(
        conn,
        message=f"Reopened from {prev_status} → todo",
        severity="info",
        category="state",
        gap_id=gap_id,
        actor="refine",
    )
    try:
        runner_call(M_ENFORCE_SCHEDULING, {}, 10.0)
    except Exception:
        pass
    return 200, {"ok": True}


def retry_merge(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
) -> tuple[int, dict]:
    _, ownership_err = require_active_gap(conn, gap_id)
    if ownership_err is not None:
        return ownership_err
    result = runner_call(M_RETRY_MERGE, {"gap_id": gap_id}, 10.0)
    return (200 if result.get("ok") else 409), result


def retry_qa(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
) -> tuple[int, dict]:
    _, ownership_err = require_active_gap(conn, gap_id)
    if ownership_err is not None:
        return ownership_err
    result = runner_call(M_RETRY_QA, {"gap_id": gap_id}, 10.0)
    return (200 if result.get("ok") else 409), result


def cancel(
    conn: sqlite3.Connection,
    runner_call: RunnerCall,
    gap_id: str,
) -> tuple[int, dict]:
    row, ownership_err = require_active_gap(conn, gap_id, columns="status, node_id")
    if ownership_err is not None:
        return ownership_err
    if row["status"] in ("done", "cancelled"):
        return _err(409, f"Already terminal (status={row['status']})")
    result = runner_call(M_CANCEL, {"gap_id": gap_id}, 30.0)
    return 200, result


def compact_log(log: dict[str, Any] | None) -> dict[str, Any] | None:
    if not isinstance(log, dict):
        return None
    out: dict[str, Any] = {}
    for key in ("id", "datetime", "severity", "category", "message", "actor", "gap_id"):
        if key in log and log[key] is not None:
            out[key] = log[key]
    return out


def round_metadata(round_obj: dict[str, Any]) -> dict[str, Any]:
    meta = dict(round_obj)
    logs = meta.pop("logs", [])
    if not isinstance(logs, list):
        logs = []
    meta["log_count"] = len(logs)
    if logs:
        meta["latest_log"] = compact_log(logs[-1])
        for log in reversed(logs):
            if isinstance(log, dict) and log.get("category") == "state":
                meta["latest_state_log"] = compact_log(log)
                break
        for log in reversed(logs):
            if isinstance(log, dict) and log.get("severity") == "error":
                meta["latest_error_log"] = compact_log(log)
                break
        for log in reversed(logs):
            if (
                isinstance(log, dict)
                and log.get("category") == "state"
                and str(log.get("message") or "").startswith(
                    "Workflow status changed:",
                )
            ):
                meta["latest_workflow_log"] = compact_log(log)
                break
    return meta


def mark_log_source(logs: list[dict[str, Any]], source: str) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for log in logs:
        item = dict(log)
        item.setdefault("source", source)
        out.append(item)
    return out


def activity_for_round(
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


def enrich_gap_row(row: dict[str, Any]) -> dict[str, Any]:
    row["node_display_name"] = project_state.gap_node_display(row.get("node_id"))
    return row


def node_owner(node_id: str | None) -> str:
    return str(node_id or project_state.DEFAULT_NODE_ID)


def ownership_error(
    owner_id: str | None,
    *,
    active_id: str | None = None,
    count: int = 1,
) -> tuple[int, dict]:
    owner = node_owner(owner_id)
    active = active_id or project_state.active_node_id()
    owner_name = project_state.gap_node_display(owner)
    active_name = project_state.gap_node_display(active)
    subject = "Gap is" if count == 1 else f"{count} Gaps are"
    return _err(
        409,
        (
            f"Action not allowed: {subject} owned by another node "
            f"({owner_name}). Transfer to {active_name} before making changes."
        ),
        error_code="node_ownership",
    )


def require_active_gap(
    conn: sqlite3.Connection,
    gap_id: str,
    *,
    columns: str = "status, branch_name, node_id",
) -> tuple[sqlite3.Row | None, tuple[int, dict] | None]:
    row = conn.execute(
        f"SELECT {columns} FROM gaps_index WHERE id = ?",
        (gap_id,),
    ).fetchone()
    if not row:
        return None, _err(404, "Gap not found")
    active = project_state.active_node_id()
    if node_owner(row["node_id"]) != active:
        return None, ownership_error(row["node_id"], active_id=active)
    return row, None


def validate_user_status_transition(
    previous: str | None,
    next_status: str,
) -> tuple[int, dict] | None:
    if previous == next_status:
        return None
    allowed = USER_STATUS_TRANSITIONS.get(previous or "", set())
    if next_status in allowed:
        return None
    return _err(
        409,
        (
            f"Invalid workflow transition: {previous or 'unknown'} → {next_status}. "
            "Use the dedicated workflow action for system-owned states."
        ),
    )


def append_gap_workflow_log(
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


def backend_exception_error(e: Exception) -> tuple[int, dict[str, Any]]:
    code_name = str(getattr(e, "code", "") or "")
    if code_name == "backend_unavailable":
        code = 502
    elif code_name == "node_ownership":
        code = 409
    elif code_name == "bad_request":
        code = 400
    else:
        code = 500
    message = str(getattr(e, "message", "") or str(e) or "Backend error")
    details = getattr(e, "details", None)
    error: dict[str, Any] = {"code": code_name or "backend_error", "message": message}
    if details is not None:
        error["details"] = details
    return code, {"error": error}


def _err(code: int, message: str, *, error_code: str | None = None) -> tuple[int, dict]:
    body: dict[str, Any] = {"error": {"message": message}}
    if error_code is not None:
        body["error"]["code"] = error_code
    return code, body

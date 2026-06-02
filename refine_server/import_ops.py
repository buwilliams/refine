"""Shared Gap import operations for the HTTP API and CLI."""
from __future__ import annotations

import csv
import difflib
import io
import re
import sqlite3
from collections import Counter
from typing import Any, Callable

from refine_server import cluster, db, gap_writer, gaps as shared_gaps
from refine_server import project_state, search_index
from refine_server.backend_protocol import (
    M_BULK_DELETE_GAPS,
    M_CREATE_GAP,
    M_DELETE_GAP,
    M_EXTRACT_GAPS,
)
from refine_server.gaps import now_iso
from refine_server.ulid import new_ulid


RunnerCall = Callable[[str, dict[str, Any], float], dict[str, Any]]
ProgressCallback = Callable[[int, int, str], None]
PrepareCancelCallback = Callable[[], None]
PersistCancelCallback = Callable[
    [list[str], list[dict[str, Any]], list[dict[str, Any]]],
    None,
]

VALID_PRIORITIES = ("low", "medium", "high")
VALID_REPORTER = re.compile(r"^[^\x00-\x1f]{1,80}$")
IMPORT_DEDUP_THRESHOLD = 0.62
DUPLICATE_DECISION_IGNORE = "duplicate"
DUPLICATE_DECISION_IMPORT = "original"
DUPLICATE_DECISION_MOVE_ORIGINAL = "move_original_to_backlog"
DUPLICATE_UPDATE_PREFIX = "update_original_"
DUPLICATE_UPDATE_FIELDS = {"actual", "target", "reporter", "priority"}
DUPLICATE_BACKLOG_PROTECTED_STATUSES = {
    "todo",
    "in-progress",
    "qa",
    "ready-merge",
    "awaiting-rebuild",
    "awaiting-review",
}
DEDUP_ALGORITHM_DESCRIPTION = (
    "deterministic normalized actual/target scoring: "
    "token/bigram cosine, character-trigram cosine, token Jaccard, "
    "and sequence ratio"
)

_IMPORT_DEDUP_STOPWORDS = {
    "a", "an", "and", "are", "as", "be", "can", "for", "from", "in", "is",
    "it", "of", "on", "or", "the", "to", "user", "users", "when", "with",
}


def _err(code: int, message: str, *, error_code: str | None = None) -> tuple[int, dict]:
    body: dict[str, Any] = {"error": {"message": message}}
    if error_code is not None:
        body["error"]["code"] = error_code
    return code, body


def _conn() -> sqlite3.Connection:
    conn = db.connect()
    project_state.ensure_sqlite_cache_current(conn)
    return conn


def extract(runner_call: RunnerCall, body: dict[str, Any]) -> tuple[int, dict]:
    raw = (body.get("text") or "").strip()
    if not raw:
        return _err(400, "text is required")
    result = runner_call(M_EXTRACT_GAPS, {"text": raw}, 200.0)
    return 200, {"drafts": result.get("drafts") or []}


def parse_csv(
    body: dict[str, Any],
    *,
    progress: ProgressCallback | None = None,
    cancel_requested: PrepareCancelCallback | None = None,
) -> tuple[int, dict]:
    raw = str(body.get("text") or "")
    if not raw.strip():
        return _err(400, "CSV text is required")
    try:
        drafts = import_parse_csv_drafts(
            raw,
            progress=progress,
            cancel_requested=cancel_requested,
        )
    except ValueError as e:
        return _err(400, str(e))
    if body.get("distribute"):
        try:
            drafts = assign_import_nodes(drafts)
        except ValueError as e:
            return _err(400, str(e))
    if body.get("dedup"):
        matches = import_dedup_matches(
            drafts,
            progress=progress,
            cancel_requested=cancel_requested,
        )
        drafts = annotate_import_duplicate_drafts(drafts, matches)
    return 200, {"drafts": drafts, "count": len(drafts)}


def import_parse_csv_drafts(
    raw: str,
    *,
    progress: ProgressCallback | None = None,
    cancel_requested: PrepareCancelCallback | None = None,
) -> list[dict[str, str]]:
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
        _progress(progress, 0, total, f"Parsing CSV 0 of {total} Gaps")
        drafts: list[dict[str, str]] = []
        for idx, (row_number, values) in enumerate(prepared_rows, start=1):
            if cancel_requested is not None:
                cancel_requested()
            actual = values.get("actual", "")
            target = values.get("target", "")
            reporter = values.get("reporter", "")
            priority = values.get("priority", "").lower()
            if (not actual and not target) or not reporter or not priority:
                raise ValueError(
                    f"CSV row {row_number} must include actual or target, plus reporter and priority"
                )
            if priority not in VALID_PRIORITIES:
                raise ValueError(
                    f"CSV row {row_number} priority must be low, medium, or high"
                )
            if not VALID_REPORTER.match(reporter):
                raise ValueError(f"CSV row {row_number} has an invalid reporter")
            drafts.append({
                "name": values.get("name", ""),
                "actual": actual,
                "target": target,
                "reporter": reporter,
                "priority": priority,
            })
            _progress(progress, idx, total, f"Parsed {idx} of {total} Gaps")
    except csv.Error as e:
        raise ValueError(f"CSV could not be parsed: {e}") from e
    if not drafts:
        raise ValueError("CSV has no importable rows")
    return drafts


def assign_import_nodes(drafts: list[dict[str, Any]]) -> list[dict[str, Any]]:
    candidates = [
        str(n.get("id") or "")
        for n in cluster.list_nodes()
        if n.get("enabled", True) and str(n.get("id") or "")
    ]
    if not candidates:
        candidates = [
            str(n.get("id") or "")
            for n in project_state.list_nodes()
            if not n.get("archived") and str(n.get("id") or "")
        ]
    if not candidates:
        raise ValueError("no enabled nodes are available for distribution")
    out: list[dict[str, Any]] = []
    for idx, draft in enumerate(drafts):
        assigned = dict(draft)
        assigned["node_id"] = candidates[idx % len(candidates)]
        out.append(assigned)
    return out


def dedup(
    body: dict[str, Any],
    *,
    progress: ProgressCallback | None = None,
    cancel_requested: PrepareCancelCallback | None = None,
) -> tuple[int, dict]:
    drafts = body.get("drafts") or []
    if not isinstance(drafts, list):
        return _err(400, "drafts must be a list")
    matches = import_dedup_matches(
        drafts,
        progress=progress,
        cancel_requested=cancel_requested,
    )
    return 200, {
        "matches": matches,
        "threshold": IMPORT_DEDUP_THRESHOLD,
        "algorithm": DEDUP_ALGORITHM_DESCRIPTION,
    }


def import_dedup_matches(
    drafts: list[Any],
    *,
    progress: ProgressCallback | None = None,
    cancel_requested: PrepareCancelCallback | None = None,
) -> list[dict[str, Any]]:
    conn = _conn()
    try:
        candidates = import_dedup_candidates(conn)
    finally:
        conn.close()
    matches: list[dict[str, Any]] = []
    total = len(drafts)
    _progress(progress, 0, total, f"Checking duplicates 0 of {total} Gaps")
    for idx, draft in enumerate(drafts, start=1):
        if cancel_requested is not None:
            cancel_requested()
        if not isinstance(draft, dict):
            _progress(progress, idx, total, f"Checked duplicates for {idx} of {total} Gaps")
            continue
        actual = (draft.get("actual") or "").strip()
        target = (draft.get("target") or "").strip()
        if not actual and not target:
            _progress(progress, idx, total, f"Checked duplicates for {idx} of {total} Gaps")
            continue
        best, best_score = best_import_duplicate(actual, target, candidates)
        if best and best_score >= IMPORT_DEDUP_THRESHOLD:
            matches.append({
                "index": idx,
                "score": round(best_score, 3),
                "draft": {"actual": actual, "target": target},
                "match": best,
            })
        _progress(progress, idx, total, f"Checked duplicates for {idx} of {total} Gaps")
    return matches


def annotate_import_duplicate_drafts(
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


def duplicate_move_to_backlog_status(status: str | None) -> dict[str, Any]:
    current = str(status or "").strip() or "unknown"
    if current == "backlog":
        return {
            "can_move_to_backlog": False,
            "move_to_backlog_reason": "already_backlog",
        }
    if current in DUPLICATE_BACKLOG_PROTECTED_STATUSES:
        return {
            "can_move_to_backlog": False,
            "move_to_backlog_reason": "protected_status",
        }
    return {
        "can_move_to_backlog": True,
        "move_to_backlog_reason": "",
    }


def move_duplicate_original_to_backlog(gap_id: str) -> dict[str, Any]:
    conn = _conn()
    try:
        row = conn.execute(
            "SELECT id, status, node_id FROM gaps_index WHERE id = ?",
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
        gate = duplicate_move_to_backlog_status(previous)
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
        append_gap_workflow_log(
            gap_id,
            f"Workflow status changed: {previous} -> backlog; duplicate import recovery",
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


def import_dedup_candidates(conn: sqlite3.Connection) -> list[dict[str, Any]]:
    rows = conn.execute(
        "SELECT id, name, status, priority, node_id FROM gaps_index"
    ).fetchall()
    node_names = {
        str(node.get("id") or ""): str(node.get("display_name") or node.get("id") or "Unknown")
        for node in project_state.list_nodes()
    }
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
        move_gate = duplicate_move_to_backlog_status(row["status"])
        out.append({
            "id": row["id"],
            "name": row["name"] or gap.get("name") or row["id"],
            "status": row["status"],
            "priority": row["priority"] or gap.get("priority") or "low",
            "node_id": row["node_id"] or project_state.DEFAULT_NODE_ID,
            "node_display_name": node_names.get(row["node_id"], "Unknown"),
            "actual": actual,
            "target": target,
            **move_gate,
        })
    return out


def find_import_duplicate(
    actual: str,
    target: str,
    candidates: list[dict[str, Any]] | None = None,
) -> dict[str, Any] | None:
    if candidates is None:
        conn = _conn()
        try:
            candidates = import_dedup_candidates(conn)
        finally:
            conn.close()
    best, best_score = best_import_duplicate(actual, target, candidates)
    if best and best_score >= IMPORT_DEDUP_THRESHOLD:
        return {"score": round(best_score, 3), "match": best}
    return None


def best_import_duplicate(
    actual: str,
    target: str,
    candidates: list[dict[str, Any]],
) -> tuple[dict[str, Any] | None, float]:
    best: dict[str, Any] | None = None
    best_score = 0.0
    for candidate in candidates:
        score = import_dedup_score(
            actual,
            target,
            candidate["actual"],
            candidate["target"],
        )
        if score > best_score:
            best_score = score
            best = candidate
    return best, best_score


def import_dedup_score(
    draft_actual: str,
    draft_target: str,
    candidate_actual: str,
    candidate_target: str,
) -> float:
    draft = import_dedup_normalize(f"{draft_actual}\n{draft_target}")
    candidate = import_dedup_normalize(f"{candidate_actual}\n{candidate_target}")
    if not draft or not candidate:
        return 0.0
    if draft == candidate:
        return 1.0
    draft_numbers = set(re.findall(r"\d+", draft))
    candidate_numbers = set(re.findall(r"\d+", candidate))
    trigram = import_trigram_cosine(draft, candidate)
    jaccard = import_token_jaccard(draft, candidate)
    sequence = difflib.SequenceMatcher(None, draft, candidate).ratio()
    strict_score = (0.55 * trigram) + (0.30 * jaccard) + (0.15 * sequence)
    token_score = import_token_cosine(draft, candidate)
    fuzzy_score = (0.45 * token_score) + (0.35 * sequence) + (0.20 * trigram)
    score = max(strict_score, fuzzy_score)
    if draft_numbers and candidate_numbers and draft_numbers != candidate_numbers:
        score = min(score, 0.5)
    return score


def import_dedup_normalize(text: str) -> str:
    text = re.sub(r"[^a-z0-9]+", " ", text.lower())
    return re.sub(r"\s+", " ", text).strip()


def import_token_jaccard(a: str, b: str) -> float:
    aa = set(a.split())
    bb = set(b.split())
    if not aa or not bb:
        return 0.0
    return len(aa & bb) / len(aa | bb)


def import_token_cosine(a: str, b: str) -> float:
    ca = import_token_counts(a)
    cb = import_token_counts(b)
    if not ca or not cb:
        return 0.0
    dot = sum(ca[key] * cb.get(key, 0) for key in ca)
    mag_a = sum(v * v for v in ca.values()) ** 0.5
    mag_b = sum(v * v for v in cb.values()) ** 0.5
    if not mag_a or not mag_b:
        return 0.0
    return dot / (mag_a * mag_b)


def import_token_counts(text: str) -> Counter[str]:
    tokens = [
        import_stem_token(token)
        for token in text.split()
        if token not in _IMPORT_DEDUP_STOPWORDS
    ]
    counts: Counter[str] = Counter(tokens)
    counts.update(
        f"{left} {right}"
        for left, right in zip(tokens, tokens[1:])
    )
    return counts


def import_stem_token(token: str) -> str:
    for suffix in ("ing", "ed", "es", "s"):
        if len(token) > len(suffix) + 3 and token.endswith(suffix):
            return token[:-len(suffix)]
    return token


def import_trigram_cosine(a: str, b: str) -> float:
    ca = import_char_ngrams(a)
    cb = import_char_ngrams(b)
    if not ca or not cb:
        return 0.0
    dot = sum(ca[key] * cb.get(key, 0) for key in ca)
    mag_a = sum(v * v for v in ca.values()) ** 0.5
    mag_b = sum(v * v for v in cb.values()) ** 0.5
    if not mag_a or not mag_b:
        return 0.0
    return dot / (mag_a * mag_b)


def import_char_ngrams(text: str, n: int = 3) -> Counter[str]:
    compact = f"  {text}  "
    if len(compact) <= n:
        return Counter([compact])
    return Counter(compact[i:i + n] for i in range(len(compact) - n + 1))


def rollback_import_created_gaps(
    runner_call: RunnerCall,
    created: list[str],
) -> int:
    if not created:
        return 0
    remaining = list(reversed(created))
    rolled_back = 0
    try:
        result = runner_call(
            M_BULK_DELETE_GAPS,
            {"gap_ids": remaining},
            120.0,
        )
        rolled_back = int(result.get("deleted") or 0)
        deleted_ids = {
            str(gap_id)
            for gap_id in (result.get("ids") or [])
        }
        if rolled_back >= len(created):
            return rolled_back
        if deleted_ids:
            remaining = [gap_id for gap_id in remaining if gap_id not in deleted_ids]
    except Exception:
        pass
    for gap_id in remaining:
        try:
            result = runner_call(M_DELETE_GAP, {"gap_id": gap_id}, 30.0)
            if result.get("deleted"):
                rolled_back += 1
        except Exception:
            continue
    try:
        conn = _conn()
        try:
            existing = {
                str(row["id"])
                for row in conn.execute(
                    "SELECT id FROM gaps_index WHERE id IN ("
                    + ",".join("?" * len(created))
                    + ")",
                    created,
                )
            }
        finally:
            conn.close()
        confirmed_deleted = sum(1 for gap_id in created if gap_id not in existing)
        rolled_back = max(rolled_back, confirmed_deleted)
    except Exception:
        pass
    return rolled_back


def rollback_import_duplicate_moves(moves: list[dict[str, Any]]) -> int:
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
            append_gap_workflow_log(
                gap_id,
                f"Workflow status changed: backlog -> {previous}; import cancel rollback",
            )
            restored += 1
        except Exception:
            continue
    return restored


def rollback_import_duplicate_updates(updates: list[dict[str, Any]]) -> int:
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
                update_gap_priority_no_ownership(gap_id, str(before["priority"]))
            if "reporter" in before:
                update_gap_reporter_index_no_ownership(gap_id, str(before["reporter"]))
            upsert_gap_search_no_ownership(gap_id)
            append_gap_workflow_log(
                gap_id,
                "Original Gap restored after cancelled import update",
            )
            restored += 1
        except Exception:
            continue
    return restored


def duplicate_update_field(decision: str) -> str:
    if not decision.startswith(DUPLICATE_UPDATE_PREFIX):
        return ""
    field = decision[len(DUPLICATE_UPDATE_PREFIX):]
    return field if field in DUPLICATE_UPDATE_FIELDS else ""


def latest_round_snapshot(gap_id: str) -> dict[str, Any]:
    gap = shared_gaps.read_gap_json(gap_id, include_logs=False) or {}
    rounds = [r for r in (gap.get("rounds") or []) if isinstance(r, dict)]
    latest = rounds[-1] if rounds else {}
    return {
        "actual": str(latest.get("actual") or ""),
        "target": str(latest.get("target") or ""),
        "reporter": str(latest.get("reporter") or ""),
        "priority": str(gap.get("priority") or "low"),
    }


def update_gap_priority_no_ownership(gap_id: str, priority: str) -> None:
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


def update_gap_reporter_index_no_ownership(gap_id: str, reporter: str) -> None:
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


def upsert_gap_search_no_ownership(gap_id: str) -> None:
    gap = shared_gaps.read_gap_json(gap_id, include_logs=False)
    if not gap:
        return
    conn = _conn()
    try:
        with db.transaction(conn):
            search_index.upsert_gap(conn, gap)
    finally:
        conn.close()


def update_duplicate_original_from_draft(
    *,
    duplicate: dict[str, Any],
    draft: dict[str, str],
    field: str,
) -> dict[str, Any]:
    gap_id = str(duplicate.get("match", {}).get("id") or "")
    if not gap_id:
        raise ValueError("duplicate match is missing")
    before_all = latest_round_snapshot(gap_id)
    before = {field: before_all[field]}
    if field in {"actual", "target", "reporter"}:
        value = str(draft.get(field) or "").strip()
        if field == "reporter" and (not value or not VALID_REPORTER.match(value)):
            raise ValueError("invalid reporter name")
        gap_writer.edit_latest_round(
            gap_id,
            actual=value if field == "actual" else None,
            target=value if field == "target" else None,
            reporter=value if field == "reporter" else None,
        )
        if field == "reporter":
            update_gap_reporter_index_no_ownership(gap_id, value)
    elif field == "priority":
        value = str(draft.get("priority") or "low").strip().lower()
        if value not in VALID_PRIORITIES:
            raise ValueError("priority must be one of low/medium/high")
        update_gap_priority_no_ownership(gap_id, value)
    else:
        raise ValueError("unsupported original update field")
    upsert_gap_search_no_ownership(gap_id)
    append_gap_workflow_log(
        gap_id,
        f"Original Gap {field} updated from duplicate import",
    )
    return {"gap_id": gap_id, "field": field, "before": before}


def persist(
    runner_call: RunnerCall,
    body: dict[str, Any],
    *,
    progress: ProgressCallback | None = None,
    cancel: PersistCancelCallback | None = None,
) -> tuple[int, dict]:
    reporter = (body.get("reporter") or "").strip()
    drafts = body.get("drafts") or []
    if not isinstance(drafts, list) or not drafts:
        return _err(400, "drafts must be a non-empty list")
    dedup_candidates: list[dict[str, Any]] | None = None
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
    _progress(progress, 0, total, f"Importing 0 of {total} Gaps")
    for idx, d in enumerate(drafts, start=1):
        _cancel(cancel, created, duplicate_moves, duplicate_updates)
        _progress(progress, idx - 1, total, f"Importing Gap {idx} of {total}")
        if not isinstance(d, dict):
            failures.append({
                "index": idx,
                "error": "draft must be an object",
                "draft": {},
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        actual = (d.get("actual") or "").strip()
        target = (d.get("target") or "").strip()
        name = (d.get("name") or "").strip() or autoname(actual, target)
        draft_reporter = (d.get("reporter") or reporter).strip()
        priority = (d.get("priority") or "low").strip().lower()
        draft_node_id = str(d.get("node_id") or "").strip()
        failure_draft = {
            "name": name,
            "actual": actual,
            "target": target,
            "reporter": draft_reporter,
            "priority": priority,
        }
        if draft_node_id and project_state.node_by_id(draft_node_id) is None:
            failures.append({
                "index": idx,
                "error": f"unknown node: {draft_node_id}",
                "draft": {**failure_draft, "node_id": draft_node_id},
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        if priority not in VALID_PRIORITIES:
            failures.append({
                "index": idx,
                "error": "priority must be one of low/medium/high",
                "draft": failure_draft,
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        if not draft_reporter:
            failures.append({
                "index": idx,
                "error": "reporter is required",
                "draft": {**failure_draft, "reporter": ""},
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        if not VALID_REPORTER.match(draft_reporter):
            failures.append({
                "index": idx,
                "error": "invalid reporter name",
                "draft": failure_draft,
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        if not actual and not target:
            failures.append({
                "index": idx,
                "error": "actual or target must be non-empty",
                "draft": failure_draft,
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        duplicate_decision = str(d.get("duplicate_decision") or "").strip()
        if duplicate_decision == DUPLICATE_DECISION_IGNORE:
            duplicate_actions["ignored"] += 1
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        update_field = duplicate_update_field(duplicate_decision)
        duplicate = None
        if duplicate_decision != DUPLICATE_DECISION_IMPORT or update_field:
            if dedup_candidates is None:
                conn = _conn()
                try:
                    dedup_candidates = import_dedup_candidates(conn)
                finally:
                    conn.close()
            duplicate = find_import_duplicate(
                actual,
                target,
                candidates=dedup_candidates,
            )
        if update_field and not duplicate:
            failures.append({
                "index": idx,
                "error": "original Gap no longer matches this draft",
                "code": "duplicate_update_missing",
                "draft": failure_draft,
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        if duplicate and update_field:
            try:
                update = update_duplicate_original_from_draft(
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
                    "draft": failure_draft,
                })
            _cancel(cancel, created, duplicate_moves, duplicate_updates)
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        if duplicate and duplicate_decision == DUPLICATE_DECISION_MOVE_ORIGINAL:
            move = move_duplicate_original_to_backlog(duplicate["match"]["id"])
            if move.get("moved"):
                duplicate_actions["moved_to_backlog"] += 1
                duplicate_moves.append(move)
            else:
                duplicate_actions["move_noop"] += 1
            _cancel(cancel, created, duplicate_moves, duplicate_updates)
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        if duplicate and duplicate_decision != DUPLICATE_DECISION_IMPORT:
            failures.append({
                "index": idx,
                "error": "possible duplicate Gap found",
                "code": "duplicate_gap",
                "duplicate": duplicate,
                "draft": failure_draft,
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
            continue
        gap_id = new_ulid()
        try:
            runner_call(M_CREATE_GAP, {
                "gap_id": gap_id,
                "name": name,
                "reporter": draft_reporter,
                "priority": priority,
                "actual": actual,
                "target": target,
                "node_id": draft_node_id or project_state.active_node_id(),
            }, 30.0)
            created.append(gap_id)
            _cancel(cancel, created, duplicate_moves, duplicate_updates)
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
        except Exception as e:
            failures.append({
                "index": idx,
                "error": getattr(e, "message", None) or str(e),
                "code": getattr(e, "code", None),
                "draft": failure_draft,
            })
            _progress(progress, idx, total, f"Imported {idx} of {total} drafts")
    _cancel(cancel, created, duplicate_moves, duplicate_updates)
    status = 201 if created and not failures else 200
    return status, {
        "created": created,
        "count": len(created),
        "failures": failures,
        "failed": len(failures),
        "duplicate_actions": duplicate_actions,
    }


def create_gap(
    runner_call: RunnerCall,
    body: dict[str, Any],
) -> tuple[int, dict]:
    reporter = (body.get("reporter") or "").strip()
    actual = (body.get("actual") or "").strip()
    target = (body.get("target") or "").strip()
    name = (body.get("name") or "").strip() or autoname(actual, target)
    priority = (body.get("priority") or "low").strip().lower()
    duplicate_decision = str(body.get("duplicate_decision") or "").strip()
    if priority not in VALID_PRIORITIES:
        return _err(400, "priority must be one of low/medium/high")
    if not reporter:
        return _err(400, "reporter is required")
    if not actual and not target:
        return _err(400, "actual or target must be non-empty")
    if not VALID_REPORTER.match(reporter):
        return _err(400, "invalid reporter name")
    duplicate = find_import_duplicate(actual, target)
    if duplicate and duplicate_decision == DUPLICATE_DECISION_IGNORE:
        return 200, {
            "ok": True,
            "created": False,
            "duplicate_action": "ignored",
            "duplicate": duplicate,
        }
    if duplicate and duplicate_decision == DUPLICATE_DECISION_MOVE_ORIGINAL:
        move = move_duplicate_original_to_backlog(duplicate["match"]["id"])
        return 200, {
            "ok": True,
            "created": False,
            "duplicate_action": "move_original_to_backlog",
            "duplicate": duplicate,
            "move": move,
        }
    if duplicate and duplicate_decision != DUPLICATE_DECISION_IMPORT:
        return 409, {
            "error": {
                "message": "Possible duplicate Gap found",
                "code": "duplicate_gap",
                "duplicate": duplicate,
            }
        }
    gap_id = new_ulid()
    result = runner_call(M_CREATE_GAP, {
        "gap_id": gap_id,
        "name": name,
        "priority": priority,
        "reporter": reporter,
        "actual": actual,
        "target": target,
    }, 30.0)
    return 201, result


def autoname(actual: str, target: str) -> str:
    text = (target or actual or "Untitled Gap").strip()
    text = text.split("\n", 1)[0]
    match = re.split(r"[.!?]", text, maxsplit=1)
    short = (match[0] if match else text).strip()
    if len(short) > 80:
        short = short[:77].rstrip() + "..."
    return short or "Untitled Gap"


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


def _progress(
    progress: ProgressCallback | None,
    completed: int,
    total: int,
    message: str,
) -> None:
    if progress is not None:
        progress(completed, total, message)


def _cancel(
    cancel: PersistCancelCallback | None,
    created: list[str],
    duplicate_moves: list[dict[str, Any]],
    duplicate_updates: list[dict[str, Any]],
) -> None:
    if cancel is not None:
        cancel(created, duplicate_moves, duplicate_updates)

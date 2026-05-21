"""Sole owner of gap.json writes. Serializes per-Gap with a lock map.

Backend handlers, subprocess supervisor, and dispatcher call into this module
to mutate gap.json. Reads happen elsewhere via refine_server.gaps.
"""
from __future__ import annotations

import threading
from collections import defaultdict
from pathlib import Path
from typing import Any

from refine_server import gaps as shared_gaps
from refine_server.gaps import now_iso

_locks: dict[str, threading.Lock] = defaultdict(threading.Lock)
_locks_master = threading.Lock()


def _lock_for(gap_id: str) -> threading.Lock:
    with _locks_master:
        return _locks[gap_id]


def create_gap(*, gap_id: str, name: str, initial_round: dict[str, Any],
               status: str = "backlog", priority: str = "low",
               instance_id: str | None = None) -> dict[str, Any]:
    """Initialize gap.json with one round. Returns the new Gap record."""
    with _lock_for(gap_id):
        gap = shared_gaps.empty_gap(gap_id, name)
        gap["status"] = status
        gap["priority"] = priority
        if instance_id:
            gap["instance_id"] = instance_id
        gap["rounds"].append(initial_round)
        gap["updated"] = now_iso()
        shared_gaps.write_gap_json(gap)
        return gap


def update_fields(gap_id: str, **fields: Any) -> dict[str, Any]:
    """Update canonical top-level gap fields and touch updated."""
    allowed = {"name", "status", "priority", "branch_name", "instance_id"}
    unknown = set(fields) - allowed
    if unknown:
        raise ValueError(f"unknown gap fields: {', '.join(sorted(unknown))}")
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            raise FileNotFoundError(f"gap.json missing for {gap_id}")
        for key, value in fields.items():
            gap[key] = value
        gap["updated"] = now_iso()
        shared_gaps.write_gap_json(gap)
        return gap


def set_notes(gap_id: str, notes: list[dict[str, Any]]) -> dict[str, Any]:
    """Replace the Gap's notes list in gap.json. Each note must be a dict
    with at least an `id` and `body`; missing timestamps / author are
    filled in here so the on-disk format stays consistent.
    """
    if not isinstance(notes, list):
        raise ValueError("notes must be a list")
    now = now_iso()
    cleaned: list[dict[str, Any]] = []
    for n in notes:
        if not isinstance(n, dict):
            raise ValueError("each note must be an object")
        body = (n.get("body") or "").strip()
        if not body:
            # Drop empty notes silently — keeps the API forgiving.
            continue
        nid = n.get("id") or _new_note_id()
        cleaned.append({
            "id": str(nid),
            "author": str(n.get("author") or "").strip(),
            "body": body,
            "created": str(n.get("created") or now),
            "updated": str(n.get("updated") or now),
        })
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            raise FileNotFoundError(f"gap.json missing for {gap_id}")
        gap["notes"] = cleaned
        gap["updated"] = now
        shared_gaps.write_gap_json(gap)
        return gap


def _new_note_id() -> str:
    import secrets
    return "note_" + secrets.token_hex(6)


def append_round(gap_id: str, round_obj: dict[str, Any]) -> dict[str, Any]:
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            raise FileNotFoundError(f"gap.json missing for {gap_id}")
        gap["rounds"].append(round_obj)
        gap["updated"] = now_iso()
        shared_gaps.write_gap_json(gap)
        return gap


def edit_latest_round(gap_id: str, *, actual: str | None = None,
                      target: str | None = None, reporter: str | None = None) -> dict[str, Any]:
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            raise FileNotFoundError(f"gap.json missing for {gap_id}")
        if not gap["rounds"]:
            raise ValueError("Gap has no rounds")
        r = gap["rounds"][-1]
        if actual is not None:
            r["actual"] = actual
        if target is not None:
            r["target"] = target
        if reporter is not None:
            r["reporter"] = reporter
        if actual is not None or target is not None or reporter is not None:
            shared_gaps.reset_round_governance(r)
        r["updated"] = now_iso()
        gap["updated"] = r["updated"]
        shared_gaps.write_gap_json(gap)
        return gap


def set_latest_round_governance(gap_id: str, fields: dict[str, Any]) -> dict[str, Any]:
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            raise FileNotFoundError(f"gap.json missing for {gap_id}")
        if not gap["rounds"]:
            raise ValueError("Gap has no rounds")
        r = gap["rounds"][-1]
        shared_gaps.normalize_round_governance(r)
        allowed = set(shared_gaps.default_round_governance())
        for key, value in fields.items():
            if key in allowed:
                r[key] = value
        shared_gaps.normalize_round_governance(r)
        r["updated"] = now_iso()
        gap["updated"] = r["updated"]
        shared_gaps.write_gap_json(gap)
        return gap


def rename_reporter_in_rounds(
    conn,
    old_name: str,
    new_name: str,
    *,
    instance_id: str | None = None,
) -> int:
    """Rewrite every round whose `reporter == old_name` to `new_name`.

    Walks all Gaps in `gaps_index`, takes the same per-Gap write lock the
    other writers use, and updates each gap.json atomically. Returns the
    number of Gaps actually touched (not the number of rounds). No-op if
    the names match or either side is empty.

    This is the data-side cascade used by the runner's rename-reporter
    handler so historical rounds line up with the renamed dropdown entry.
    Deletes do *not* call this — by design, removing a reporter from the
    table preserves the original reporter string on historical rounds so
    audit history stays intact.
    """
    if not old_name or not new_name or old_name == new_name:
        return 0
    if instance_id is None:
        rows = conn.execute("SELECT id FROM gaps_index").fetchall()
    else:
        rows = conn.execute(
            "SELECT id FROM gaps_index WHERE instance_id = ?",
            (instance_id,),
        ).fetchall()
    touched = 0
    for row in rows:
        gap_id = row["id"]
        with _lock_for(gap_id):
            gap = shared_gaps.read_gap_json(gap_id)
            if gap is None:
                continue
            changed = False
            latest_changed = False
            now = now_iso()
            rounds = gap.get("rounds") or []
            for idx, r in enumerate(rounds):
                if r.get("reporter") == old_name:
                    r["reporter"] = new_name
                    r["updated"] = now
                    changed = True
                    if idx == len(rounds) - 1:
                        latest_changed = True
            if changed:
                gap["updated"] = now
                shared_gaps.write_gap_json(gap)
                touched += 1
                # Mirror the rename onto the index `reporter` column when
                # it's the latest round that changed — the column tracks
                # the latest-round attribution. Older-round renames don't
                # affect the current attribution.
                if latest_changed:
                    conn.execute(
                        "UPDATE gaps_index SET reporter = ? WHERE id = ?",
                        (new_name, gap_id),
                    )
                try:
                    from refine_server import search_index

                    search_index.upsert_gap(conn, gap)
                except Exception:
                    pass
    return touched


def append_round_log(*, gap_id: str, round_idx: int, message: str,
                     severity: str = "info", category: str = "cli",
                     details: str | None = None,
                     actor: str | None = None,
                     actions: list[dict] | None = None) -> None:
    with _lock_for(gap_id):
        gap = shared_gaps.read_gap_json(gap_id)
        if gap is None:
            return
        rounds = gap.get("rounds", [])
        if round_idx < 0 or round_idx >= len(rounds):
            return
        entry = shared_gaps.new_log_entry(
            message,
            severity=severity, category=category,
            details=details, actions=actions, actor=actor,
        )
        rounds[round_idx].setdefault("logs", []).append(entry)
        rounds[round_idx]["updated"] = entry["datetime"]
        gap["updated"] = entry["datetime"]
        shared_gaps.write_gap_json(gap)


def append_latest_round_log(*, gap_id: str, message: str,
                            severity: str = "info", category: str = "cli",
                            details: str | None = None,
                            actor: str | None = None,
                            actions: list[dict] | None = None) -> None:
    gap = shared_gaps.read_gap_json(gap_id)
    if gap is None:
        return
    rounds = gap.get("rounds") or []
    if not rounds:
        return
    append_round_log(
        gap_id=gap_id,
        round_idx=len(rounds) - 1,
        message=message,
        severity=severity,
        category=category,
        details=details,
        actor=actor,
        actions=actions,
    )


def update_name(gap_id: str, name: str) -> None:
    try:
        update_fields(gap_id, name=name)
    except FileNotFoundError:
        return


def delete_gap_file(gap_id: str) -> None:
    """Remove gap.json and the containing dir. (SQLite cleanup is separate.)"""
    from refine_server.paths import gap_dir, gap_json_path
    with _lock_for(gap_id):
        p = gap_json_path(gap_id)
        if p.exists():
            p.unlink()
        d = gap_dir(gap_id)
        try:
            d.rmdir()
            d.parent.rmdir()  # shard dir, if empty
        except OSError:
            pass

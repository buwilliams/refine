"""Background job registry for long UI actions.

Job execution remains in-process, but status snapshots are written through
SQLite so polling survives handler reloads and concurrent jobs are serialized
per kind.
"""
from __future__ import annotations

import inspect
import json
import threading
import traceback
import uuid
from collections.abc import Callable
from datetime import datetime, timezone
from typing import Any

from refine_server import db


_JOBS: dict[str, dict[str, Any]] = {}
_LOCK = threading.Lock()
_KIND_LOCKS: dict[str, threading.Lock] = {}
_MAX_JOBS = 100


def start(kind: str, label: str, fn: Callable[..., dict[str, Any]]) -> dict[str, Any]:
    job_id = uuid.uuid4().hex
    job = {
        "id": job_id,
        "kind": kind,
        "label": label,
        "status": "queued",
        "started_at": _now(),
        "finished_at": "",
        "result": None,
        "error": None,
        "progress": {"completed": 0, "total": 0, "message": "Queued"},
    }
    with _LOCK:
        _JOBS[job_id] = job
        _trim_locked()
    _persist(job)
    thread = threading.Thread(
        target=_run,
        args=(job_id, fn),
        name=f"refine-job-{kind}",
        daemon=False,
    )
    thread.start()
    return snapshot(job_id) or job


def snapshot(job_id: str) -> dict[str, Any] | None:
    stored = _load(job_id)
    if stored is not None:
        with _LOCK:
            _JOBS[job_id] = stored
        return stored
    with _LOCK:
        job = _JOBS.get(job_id)
        return dict(job) if job else None


def progress(
    job_id: str,
    *,
    completed: int | None = None,
    total: int | None = None,
    message: str | None = None,
) -> None:
    with _LOCK:
        job = _JOBS.get(job_id)
        if not job:
            return
        current = dict(job.get("progress") or {})
        if completed is not None:
            current["completed"] = max(0, int(completed))
        if total is not None:
            current["total"] = max(0, int(total))
        if message is not None:
            current["message"] = str(message)
        job["progress"] = current
        snap = dict(job)
    _persist(snap)


def _run(job_id: str, fn: Callable[..., dict[str, Any]]) -> None:
    job = snapshot(job_id)
    kind = str((job or {}).get("kind") or "")
    kind_lock = _lock_for_kind(kind)
    with kind_lock:
        _mark_running(job_id)
        callback = lambda **kwargs: progress(job_id, **kwargs)
        try:
            if _accepts_progress(fn):
                result = fn(callback)
            else:
                result = fn()
        except Exception as e:
            with _LOCK:
                job = _JOBS[job_id]
                job["status"] = "failed"
                job["finished_at"] = _now()
                job["error"] = {
                    "message": str(e) or repr(e),
                    "details": traceback.format_exc(limit=20),
                }
                snap = dict(job)
            _persist(snap)
            return
        with _LOCK:
            job = _JOBS[job_id]
            job["status"] = "complete"
            job["finished_at"] = _now()
            job["result"] = result
            result_dict = result if isinstance(result, dict) else {}
            prog = dict(job.get("progress") or {})
            total = int(prog.get("total") or result_dict.get("updated") or 0)
            completed = int(prog.get("completed") or total)
            job["progress"] = {
                "completed": completed,
                "total": total,
                "message": "Complete",
            }
            snap = dict(job)
        _persist(snap)


def _mark_running(job_id: str) -> None:
    with _LOCK:
        job = _JOBS[job_id]
        job["status"] = "running"
        current = dict(job.get("progress") or {})
        current.setdefault("completed", 0)
        current.setdefault("total", 0)
        current["message"] = "Running"
        job["progress"] = current
        snap = dict(job)
    _persist(snap)


def _accepts_progress(fn: Callable[..., dict[str, Any]]) -> bool:
    try:
        sig = inspect.signature(fn)
    except (TypeError, ValueError):
        return False
    return bool(sig.parameters)


def _lock_for_kind(kind: str) -> threading.Lock:
    with _LOCK:
        lock = _KIND_LOCKS.get(kind)
        if lock is None:
            lock = threading.Lock()
            _KIND_LOCKS[kind] = lock
        return lock


def _trim_locked() -> None:
    if len(_JOBS) <= _MAX_JOBS:
        return
    done = [
        job for job in _JOBS.values()
        if job.get("status") in {"complete", "failed"}
    ]
    done.sort(key=lambda job: str(job.get("finished_at") or job.get("started_at") or ""))
    for job in done[:max(0, len(_JOBS) - _MAX_JOBS)]:
        _JOBS.pop(str(job["id"]), None)


def _persist(job: dict[str, Any]) -> None:
    try:
        conn = db.connect()
        try:
            with db.transaction(conn):
                conn.execute(
                    "INSERT INTO background_jobs "
                    "(id, kind, label, status, started_at, finished_at, "
                    "result_json, error_json, progress_json) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) "
                    "ON CONFLICT(id) DO UPDATE SET "
                    "kind = excluded.kind, "
                    "label = excluded.label, "
                    "status = excluded.status, "
                    "started_at = excluded.started_at, "
                    "finished_at = excluded.finished_at, "
                    "result_json = excluded.result_json, "
                    "error_json = excluded.error_json, "
                    "progress_json = excluded.progress_json",
                    (
                        job["id"],
                        job["kind"],
                        job["label"],
                        job["status"],
                        job["started_at"],
                        job.get("finished_at") or "",
                        json.dumps(job.get("result")),
                        json.dumps(job.get("error")),
                        json.dumps(job.get("progress") or {}),
                    ),
                )
        finally:
            conn.close()
    except Exception:
        pass


def _load(job_id: str) -> dict[str, Any] | None:
    try:
        conn = db.connect()
        try:
            row = conn.execute(
                "SELECT id, kind, label, status, started_at, finished_at, "
                "result_json, error_json, progress_json "
                "FROM background_jobs WHERE id = ?",
                (job_id,),
            ).fetchone()
        finally:
            conn.close()
    except Exception:
        return None
    if not row:
        return None
    with _LOCK:
        local_exists = job_id in _JOBS
    status = row["status"]
    error = _loads(row["error_json"])
    finished_at = row["finished_at"] or ""
    stale = status in {"queued", "running"} and not local_exists
    if stale:
        status = "failed"
        finished_at = _now()
        error = {
            "message": "Background job interrupted by process restart.",
            "details": "",
        }
    snap = {
        "id": row["id"],
        "kind": row["kind"],
        "label": row["label"],
        "status": status,
        "started_at": row["started_at"],
        "finished_at": finished_at,
        "result": _loads(row["result_json"]),
        "error": error,
        "progress": _loads(row["progress_json"]) or {},
    }
    if stale:
        _persist(snap)
    return snap


def _loads(raw: str | None) -> Any:
    if not raw:
        return None
    try:
        return json.loads(raw)
    except Exception:
        return None


def _now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

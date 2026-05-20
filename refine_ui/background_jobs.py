"""Small in-process background job registry for long UI actions."""
from __future__ import annotations

import threading
import traceback
import uuid
from collections.abc import Callable
from datetime import datetime, timezone
from typing import Any


_JOBS: dict[str, dict[str, Any]] = {}
_LOCK = threading.Lock()
_MAX_JOBS = 100


def start(kind: str, label: str, fn: Callable[[], dict[str, Any]]) -> dict[str, Any]:
    job_id = uuid.uuid4().hex
    job = {
        "id": job_id,
        "kind": kind,
        "label": label,
        "status": "running",
        "started_at": _now(),
        "finished_at": "",
        "result": None,
        "error": None,
    }
    with _LOCK:
        _JOBS[job_id] = job
        _trim_locked()
    thread = threading.Thread(
        target=_run,
        args=(job_id, fn),
        name=f"refine-job-{kind}",
        daemon=True,
    )
    thread.start()
    return snapshot(job_id) or job


def snapshot(job_id: str) -> dict[str, Any] | None:
    with _LOCK:
        job = _JOBS.get(job_id)
        return dict(job) if job else None


def _run(job_id: str, fn: Callable[[], dict[str, Any]]) -> None:
    try:
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
        return
    with _LOCK:
        job = _JOBS[job_id]
        job["status"] = "complete"
        job["finished_at"] = _now()
        job["result"] = result


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


def _now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

"""Background job persistence and per-kind serialization tests."""
from __future__ import annotations

import threading
import time

from tests.helpers import cleanup_tmp, init_refine, make_client_repo


def main() -> int:
    tmp, client = make_client_repo("refine-background-jobs-")
    conn = init_refine(client)
    try:
        from refine_ui import background_jobs

        def quick(progress):  # noqa: ANN001, ANN202
            progress(completed=1, total=1, message="step")
            return {"ok": True, "updated": 1}

        job = background_jobs.start("persist_test", "Persist test", quick)
        done = wait_job(job["id"])
        assert done["status"] == "complete", done
        assert done["progress"] == {
            "completed": 1,
            "total": 1,
            "message": "Complete",
        }, done

        with background_jobs._LOCK:  # noqa: SLF001
            background_jobs._JOBS.pop(job["id"], None)  # noqa: SLF001
        persisted = background_jobs.snapshot(job["id"])
        assert persisted and persisted["status"] == "complete", persisted
        assert persisted["result"] == {"ok": True, "updated": 1}, persisted

        events: list[str] = []
        first_started = threading.Event()

        def first(progress):  # noqa: ANN001, ANN202
            progress(completed=0, total=1, message="first")
            events.append("first-start")
            first_started.set()
            time.sleep(0.2)
            events.append("first-end")
            return {"updated": 1}

        def second(progress):  # noqa: ANN001, ANN202
            progress(completed=0, total=1, message="second")
            events.append("second-start")
            return {"updated": 1}

        first_job = background_jobs.start("serialized_kind", "First", first)
        assert first_started.wait(timeout=2), events
        second_job = background_jobs.start("serialized_kind", "Second", second)
        time.sleep(0.05)
        assert events == ["first-start"], events
        wait_job(first_job["id"])
        wait_job(second_job["id"])
        assert events == ["first-start", "first-end", "second-start"], events
    finally:
        try:
            conn.close()
        except Exception:
            pass
        cleanup_tmp(tmp)

    print("background jobs tests OK")
    return 0


def wait_job(job_id: str) -> dict:
    from refine_ui import background_jobs

    deadline = time.time() + 10
    while time.time() < deadline:
        job = background_jobs.snapshot(job_id)
        if job and job["status"] in {"complete", "failed"}:
            return job
        time.sleep(0.02)
    raise AssertionError(f"job did not finish: {job_id}")


if __name__ == "__main__":
    raise SystemExit(main())

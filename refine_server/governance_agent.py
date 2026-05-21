"""Single-threaded Governance worker.

The Governance agent reviews `todo` Gaps before the Dispatcher can launch
implementation agents. It owns classification writes to latest-round
governance fields and rule auto-maintenance.
"""
from __future__ import annotations

import json
import sqlite3
import threading
import time

from refine_server import activity, db, governance, gaps as shared_gaps
from refine_server.gaps import now_iso

from . import gap_writer


_POLL_INTERVAL_SECONDS = 5.0


class GovernanceAgent:
    def __init__(self, *, get_conn, on_pass=None) -> None:
        self._get_conn = get_conn
        self._on_pass = on_pass
        self._wake = threading.Event()
        self._stop = threading.Event()
        self._thread: threading.Thread | None = None
        self._snap_lock = threading.Lock()
        self._current_gap_id: str | None = None
        self._current_started: float | None = None
        self._last_outcome: str | None = None

    def start(self) -> None:
        self._thread = threading.Thread(
            target=self._loop, name="refine-governance", daemon=True,
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._wake.set()
        if self._thread is not None and self._thread is not threading.current_thread():
            self._thread.join(timeout=5.0)

    def wake(self) -> None:
        self._wake.set()

    def snapshot(self) -> dict:
        paused = bool(db.get_setting_int(self._get_conn(), "paused", 0))
        with self._snap_lock:
            gap_id = self._current_gap_id
            started = self._current_started
            last = self._last_outcome
        elapsed = int(time.monotonic() - started) if started is not None else 0
        queued = 0
        if governance.is_configured(self._get_conn()):
            queued = len(self._find_pending(limit=1000))
        if paused and gap_id is None:
            state = "paused"
        elif gap_id is not None:
            state = "reviewing"
        else:
            state = "idle"
        return {
            "state": state,
            "paused": paused,
            "gap_id": gap_id,
            "elapsed_seconds": elapsed,
            "queued": queued,
            "last_outcome": last,
            "configured": governance.is_configured(self._get_conn()),
        }

    def _loop(self) -> None:
        while not self._stop.is_set():
            self._wake.clear()
            try:
                self._tick()
            except Exception as e:
                try:
                    activity.append(
                        self._get_conn(),
                        message=f"Governance tick error: {e!r}",
                        severity="error", category="governance",
                        actor="runner",
                    )
                except Exception:
                    pass
            self._wake.wait(timeout=_POLL_INTERVAL_SECONDS)

    def _tick(self) -> None:
        conn = self._get_conn()
        if db.get_setting_int(conn, "paused", 0):
            return
        if not governance.is_configured(conn):
            return
        rows = self._find_pending(limit=1)
        if not rows:
            return
        self._review_one(rows[0]["id"])
        if self._find_pending(limit=1):
            self._wake.set()

    def _find_pending(self, *, limit: int) -> list[sqlite3.Row]:
        conn = self._get_conn()
        rows = conn.execute(
            "SELECT id, priority, updated FROM gaps_index "
            "WHERE status = 'todo' "
            "ORDER BY CASE priority "
            "  WHEN 'high' THEN 0 "
            "  WHEN 'medium' THEN 1 "
            "  ELSE 2 "
            "END, updated ASC LIMIT ?",
            (limit,),
        ).fetchall()
        out = []
        for row in rows:
            gap = shared_gaps.read_gap_json(row["id"], include_logs=False)
            latest = (gap.get("rounds") or [])[-1] if gap and gap.get("rounds") else None
            if not latest:
                out.append(row)
                continue
            shared_gaps.normalize_round_governance(latest)
            if latest.get("rule_state") == "unclassified":
                out.append(row)
        return out

    def _review_one(self, gap_id: str) -> None:
        conn = self._get_conn()
        with self._snap_lock:
            self._current_gap_id = gap_id
            self._current_started = time.monotonic()
        try:
            try:
                gap_writer.append_latest_round_log(
                    gap_id=gap_id,
                    severity="info",
                    category="governance",
                    actor="runner",
                    message="Governance review started",
                )
            except Exception:
                pass
            provider = db.get_setting(conn, "agent_cli")
            try:
                result = governance.classify_gap(conn, gap_id, provider=provider)
            except Exception as e:
                result = governance.normalize_classification({
                    "rule_state": "needs_review",
                    "meta_rule_state": "rule_review_needed",
                    "product_state": "fail",
                    "constitution_state": "fail",
                    "message": "Governance review failed; human review is needed.",
                    "details": repr(e),
                    "rule_actions": [],
                })
            passed = (
                result["rule_state"] == "passed"
                and result["product_state"] == "pass"
                and result["constitution_state"] == "pass"
            )
            applied_actions = []
            if passed and result["governance_rule_actions"]:
                applied_actions = governance.apply_rule_actions(
                    conn, result["governance_rule_actions"],
                )
                if applied_actions:
                    details = json.dumps(applied_actions, indent=2)
                    try:
                        gap_writer.append_latest_round_log(
                            gap_id=gap_id,
                            severity="info",
                            category="governance",
                            actor="runner",
                            message=(
                                "Governance auto-applied "
                                f"{len(applied_actions)} rule change"
                                f"{'' if len(applied_actions) == 1 else 's'}"
                            ),
                            details=details,
                        )
                    except Exception:
                        pass
                    activity.append(
                        conn,
                        message=(
                            "Governance auto-applied "
                            f"{len(applied_actions)} rule change"
                            f"{'' if len(applied_actions) == 1 else 's'}"
                        ),
                        severity="info", category="governance",
                        gap_id=gap_id, actor="runner",
                        details=details,
                    )
            fields = {
                "rule_state": result["rule_state"],
                "meta_rule_state": result["meta_rule_state"],
                "product_state": result["product_state"],
                "constitution_state": result["constitution_state"],
                "governance_message": (
                    result["governance_message"]
                    or ("Governance passed." if passed else "Governance review did not pass.")
                ),
                "governance_details": result["governance_details"],
                "governance_checked_at": now_iso(),
                "governance_rule_actions": applied_actions or result["governance_rule_actions"],
            }
            gap = gap_writer.set_latest_round_governance(gap_id, fields)
            round_idx = max(0, len(gap.get("rounds") or []) - 1)
            if passed:
                self._log_result(conn, gap_id, round_idx, fields, severity="info")
                if self._on_pass is not None:
                    self._on_pass(gap_id)
                with self._snap_lock:
                    self._last_outcome = "passed"
            else:
                with db.transaction(conn):
                    conn.execute(
                        "UPDATE gaps_index SET status = 'backlog', updated = ? "
                        "WHERE id = ? AND status = 'todo'",
                        (now_iso(), gap_id),
                    )
                try:
                    gap_writer.update_fields(gap_id, status="backlog")
                    gap_writer.append_latest_round_log(
                        gap_id=gap_id,
                        severity="warn",
                        category="state",
                        actor="runner",
                        message=(
                            "Workflow status changed: todo → backlog; "
                            "governance review did not pass"
                        ),
                    )
                except Exception:
                    pass
                self._log_result(conn, gap_id, round_idx, fields, severity="warn")
                with self._snap_lock:
                    self._last_outcome = result["rule_state"]
        finally:
            with self._snap_lock:
                self._current_gap_id = None
                self._current_started = None

    def _log_result(self, conn, gap_id: str, round_idx: int,
                    fields: dict[str, object], *, severity: str) -> None:
        message = str(fields.get("governance_message") or "Governance reviewed.")
        details = str(fields.get("governance_details") or "")
        try:
            gap_writer.append_round_log(
                gap_id=gap_id, round_idx=round_idx,
                severity=severity, category="governance",
                actor="runner", message=message, details=details or None,
            )
        except Exception:
            pass
        activity.append(
            conn,
            message=message,
            severity=severity,
            category="governance",
            gap_id=gap_id,
            actor="runner",
            details=details or None,
        )

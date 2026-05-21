"""Guidance classification and prompt composition for Gap agent runs."""
from __future__ import annotations

import hashlib
import json
import sqlite3
from typing import Any, Callable

from refine_server import activity, db, project_state
from refine_server.gaps import now_iso

from . import gap_writer, governance


_CLASSIFY_PROMPT = """You are selecting operator guidance for one software Gap.

Return only JSON in this shape:
{{
  "decisions": [
    {{"index": 0, "decision": "accept", "reason": "short reason"}},
    {{"index": 1, "decision": "reject", "reason": "short reason"}}
  ]
}}

Use "accept" only when the guidance rule is relevant to this Gap. Use
"reject" when it is not relevant. Consider every guidance item exactly once.

Gap:
Name: {name}

Current behavior:
{actual}

Desired behavior:
{target}

Guidance items:
{guidance}
"""


def select_for_gap(
    conn: sqlite3.Connection,
    gap: dict[str, Any],
    *,
    run_one_shot: Callable[[str], str] | None = None,
) -> tuple[list[dict[str, Any]], str]:
    """Return guidance items accepted for this Gap.

    If no guidance exists, this intentionally skips the AI round trip.
    """
    items = [
        item for item in project_state.list_guidance()
        if item.get("enabled", True)
    ]
    if not items:
        return [], ""
    rounds = gap.get("rounds") or []
    round_idx = len(rounds) - 1
    if round_idx < 0:
        return [], ""
    latest = rounds[round_idx]
    cached = _cached_selection(conn, gap, round_idx, latest, items)
    if cached is not None:
        return cached, ""
    prompt = _CLASSIFY_PROMPT.format(
        name=gap.get("name", ""),
        actual=latest.get("actual", ""),
        target=latest.get("target", ""),
        guidance=json.dumps([
            {
                "index": idx,
                "name": item["name"],
                "rule": item["rule"],
                "instructions": item["instructions"],
            }
            for idx, item in enumerate(items)
        ], ensure_ascii=False, indent=2),
    )
    if run_one_shot is None:
        provider = (db.get_setting(conn, "agent_cli") or "claude").strip().lower()
        run_one_shot = lambda p: governance._run_one_shot(  # noqa: SLF001
            p, provider=provider, timeout=300.0,
            operation="ai.guidance_select",
        )
    raw = run_one_shot(prompt)
    obj = governance._parse_json_object(raw) or {}  # noqa: SLF001
    accepted = normalize_decisions(obj, items)
    _persist_selection(conn, gap, round_idx, latest, items, accepted, raw)
    return accepted, raw


def normalize_decisions(obj: dict[str, Any], items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    decisions = obj.get("decisions") or obj.get("guidance") or []
    accepted_indexes: set[int] = set()
    name_to_index = {item["name"].strip().lower(): idx for idx, item in enumerate(items)}
    for decision in decisions:
        if not isinstance(decision, dict):
            continue
        verdict = str(
            decision.get("decision")
            or decision.get("classification")
            or decision.get("action")
            or "",
        ).strip().lower()
        if verdict != "accept":
            continue
        idx = _coerce_index(decision.get("index"))
        if idx is None:
            name = str(decision.get("name") or "").strip().lower()
            idx = name_to_index.get(name)
        if idx is not None and 0 <= idx < len(items):
            accepted_indexes.add(idx)
    return [items[idx] for idx in range(len(items)) if idx in accepted_indexes]


def prepend_to_prompt(prompt: str, accepted: list[dict[str, Any]]) -> str:
    if not accepted:
        return prompt
    sections = ["Additional guidance for this Gap:"]
    for item in accepted:
        label = item.get("name") or "Guidance"
        instructions = item.get("instructions") or ""
        if not instructions.strip():
            continue
        sections.append(f"{label}:\n{instructions.strip()}")
    if len(sections) == 1:
        return prompt
    return "\n\n".join(sections) + "\n\n" + prompt


def log_selection(
    conn: sqlite3.Connection,
    gap: dict[str, Any],
    accepted: list[dict[str, Any]],
    raw: str,
    *,
    actor: str = "runner",
) -> None:
    if not raw:
        return
    gap_id = str(gap.get("id") or "")
    if not gap_id:
        return
    accepted_names = [str(item.get("name") or "Guidance") for item in accepted]
    if accepted_names:
        message = "Guidance accepted: " + ", ".join(accepted_names)
    else:
        message = "Guidance reviewed; no guidance matched this Gap"
    details = json.dumps(
        {
            "accepted": accepted_names,
            "classifier_response": raw[:4000],
        },
        ensure_ascii=False,
        indent=2,
    )
    try:
        gap_writer.append_latest_round_log(
            gap_id=gap_id,
            severity="info",
            category="guidance",
            actor=actor,
            message=message,
            details=details,
        )
    except Exception:
        pass
    activity.append(
        conn,
        message=message,
        severity="info",
        category="guidance",
        gap_id=gap_id,
        actor=actor,
        details=details,
    )


def project_gap_guidance_decisions(
    conn: sqlite3.Connection,
    gap: dict[str, Any],
    *,
    use_transaction: bool = True,
) -> int:
    """Mirror canonical round guidance decisions from gap.json into SQLite."""
    gap_id = str(gap.get("id") or "")
    if not gap_id:
        return 0
    projected = 0
    for idx, round_obj in enumerate(gap.get("rounds") or []):
        if not isinstance(round_obj, dict):
            continue
        decision = round_obj.get("guidance_decision")
        if not isinstance(decision, dict):
            continue
        _project_decision(conn, gap_id, idx, decision, use_transaction=use_transaction)
        projected += 1
    return projected


def delete_gap_guidance_decisions(conn: sqlite3.Connection, gap_id: str) -> None:
    conn.execute("DELETE FROM guidance_decisions WHERE gap_id = ?", (gap_id,))


def _cached_selection(
    conn: sqlite3.Connection,
    gap: dict[str, Any],
    round_idx: int,
    round_obj: dict[str, Any],
    items: list[dict[str, Any]],
) -> list[dict[str, Any]] | None:
    expected_round = _round_fingerprint(gap, round_obj)
    expected_guidance = _guidance_fingerprint(items)
    decision = round_obj.get("guidance_decision")
    if isinstance(decision, dict) and _decision_matches(
        decision,
        round_fingerprint=expected_round,
        guidance_fingerprint=expected_guidance,
    ):
        accepted = _accepted_from_decision(decision)
        if accepted is not None:
            return accepted
    gap_id = str(gap.get("id") or "")
    if not gap_id:
        return None
    try:
        row = conn.execute(
            "SELECT accepted_json, round_fingerprint, guidance_fingerprint "
            "FROM guidance_decisions WHERE gap_id = ? AND round_idx = ?",
            (gap_id, round_idx),
        ).fetchone()
    except sqlite3.Error:
        return None
    if row is None:
        return None
    if (
        row["round_fingerprint"] != expected_round
        or row["guidance_fingerprint"] != expected_guidance
    ):
        return None
    try:
        accepted = json.loads(row["accepted_json"] or "[]")
    except (TypeError, ValueError):
        return None
    if not isinstance(accepted, list):
        return None
    return [
        project_state.normalize_guidance_item(item)
        for item in accepted
        if isinstance(item, dict)
    ]


def _persist_selection(
    conn: sqlite3.Connection,
    gap: dict[str, Any],
    round_idx: int,
    round_obj: dict[str, Any],
    items: list[dict[str, Any]],
    accepted: list[dict[str, Any]],
    raw: str,
) -> None:
    gap_id = str(gap.get("id") or "")
    if not gap_id:
        return
    decision = {
        "decided_at": now_iso(),
        "round_fingerprint": _round_fingerprint(gap, round_obj),
        "guidance_fingerprint": _guidance_fingerprint(items),
        "accepted": accepted,
        "accepted_names": [str(item.get("name") or "Guidance") for item in accepted],
        "candidate_count": len(items),
        "candidate_names": [str(item.get("name") or "Guidance") for item in items],
        "classifier_response": raw[:4000],
    }
    try:
        gap_writer.set_round_guidance_decision(gap_id, round_idx, decision)
    except Exception:
        return
    _project_decision(conn, gap_id, round_idx, decision)


def _project_decision(
    conn: sqlite3.Connection,
    gap_id: str,
    round_idx: int,
    decision: dict[str, Any],
    *,
    use_transaction: bool = True,
) -> None:
    accepted = _accepted_from_decision(decision) or []
    details = {
        "accepted_names": decision.get("accepted_names") or [
            str(item.get("name") or "Guidance") for item in accepted
        ],
        "candidate_count": int(decision.get("candidate_count") or 0),
        "candidate_names": decision.get("candidate_names") or [],
        "classifier_response": str(decision.get("classifier_response") or "")[:4000],
    }

    def write() -> None:
        conn.execute(
            "INSERT INTO guidance_decisions "
            "(gap_id, round_idx, decided_at, round_fingerprint, "
            "guidance_fingerprint, accepted_json, details_json) "
            "VALUES (?, ?, ?, ?, ?, ?, ?) "
            "ON CONFLICT(gap_id, round_idx) DO UPDATE SET "
            "decided_at = excluded.decided_at, "
            "round_fingerprint = excluded.round_fingerprint, "
            "guidance_fingerprint = excluded.guidance_fingerprint, "
            "accepted_json = excluded.accepted_json, "
            "details_json = excluded.details_json",
            (
                gap_id,
                round_idx,
                str(decision.get("decided_at") or now_iso()),
                str(decision.get("round_fingerprint") or ""),
                str(decision.get("guidance_fingerprint") or ""),
                json.dumps(accepted, ensure_ascii=False),
                json.dumps(details, ensure_ascii=False),
            ),
        )
    if use_transaction:
        with db.transaction(conn):
            write()
    else:
        write()


def _decision_matches(
    decision: dict[str, Any],
    *,
    round_fingerprint: str,
    guidance_fingerprint: str,
) -> bool:
    return (
        str(decision.get("round_fingerprint") or "") == round_fingerprint
        and str(decision.get("guidance_fingerprint") or "") == guidance_fingerprint
    )


def _accepted_from_decision(decision: dict[str, Any]) -> list[dict[str, Any]] | None:
    accepted = decision.get("accepted")
    if not isinstance(accepted, list):
        return None
    return [
        project_state.normalize_guidance_item(item)
        for item in accepted
        if isinstance(item, dict)
    ]


def _round_fingerprint(gap: dict[str, Any], round_obj: dict[str, Any]) -> str:
    payload = {
        "name": str(gap.get("name") or ""),
        "actual": str(round_obj.get("actual") or ""),
        "target": str(round_obj.get("target") or ""),
    }
    return _hash_json(payload)


def _guidance_fingerprint(items: list[dict[str, Any]]) -> str:
    payload = [
        {
            "name": str(item.get("name") or ""),
            "rule": str(item.get("rule") or ""),
            "instructions": str(item.get("instructions") or ""),
            "enabled": bool(item.get("enabled", True)),
        }
        for item in items
    ]
    return _hash_json(payload)


def _hash_json(value: Any) -> str:
    data = json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8", errors="replace")
    return hashlib.sha256(data).hexdigest()


def _coerce_index(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None

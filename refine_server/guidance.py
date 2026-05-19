"""Guidance classification and prompt composition for Gap agent runs."""
from __future__ import annotations

import json
import sqlite3
from typing import Any, Callable

from refine_server import db, project_state

from . import governance


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
) -> tuple[list[dict[str, str]], str]:
    """Return guidance items accepted for this Gap.

    If no guidance exists, this intentionally skips the AI round trip.
    """
    items = project_state.list_guidance()
    if not items:
        return [], ""
    latest = (gap.get("rounds") or [{}])[-1]
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
        )
    raw = run_one_shot(prompt)
    obj = governance._parse_json_object(raw) or {}  # noqa: SLF001
    accepted = normalize_decisions(obj, items)
    return accepted, raw


def normalize_decisions(obj: dict[str, Any], items: list[dict[str, str]]) -> list[dict[str, str]]:
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


def prepend_to_prompt(prompt: str, accepted: list[dict[str, str]]) -> str:
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


def _coerce_index(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None

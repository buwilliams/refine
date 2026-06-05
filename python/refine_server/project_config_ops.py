"""Shared project configuration operations for API and CLI."""
from __future__ import annotations

import sqlite3
from collections.abc import Callable
from typing import Any

from . import activity, db, governance, project_state, quality, regressions
from .backend_protocol import (
    M_ENFORCE_SCHEDULING,
    M_GOVERNANCE_GENERATE_RULES,
    M_GOVERNANCE_WAKE,
    M_REGRESSION_RUN,
)


RunnerCall = Callable[[str, dict[str, Any], float], dict[str, Any]]


def list_guidance() -> dict[str, Any]:
    return {"guidance": project_state.list_guidance()}


def update_guidance(items: Any) -> dict[str, Any]:
    if not isinstance(items, list):
        raise ValueError("guidance must be a list")
    normalized = []
    for item in items:
        if not isinstance(item, dict):
            raise ValueError("each guidance item must be an object")
        normalized.append(project_state.normalize_guidance_item(item))
    return {"guidance": project_state.write_guidance(normalized)}


def governance_get(conn: sqlite3.Connection) -> dict[str, Any]:
    result = governance.load_settings(conn)
    result["configured"] = governance.is_configured(conn)
    return result


def governance_save(
    conn: sqlite3.Connection,
    body: dict[str, Any],
    *,
    runner_call: RunnerCall | None = None,
) -> dict[str, Any]:
    rules = body.get("rules")
    if rules is not None and not isinstance(rules, list):
        raise ValueError("rules must be a list")
    result = governance.save_settings(
        conn,
        product=body.get("product"),
        constitution=body.get("constitution"),
        rules=rules,
    )
    result["configured"] = governance.is_configured(conn)
    activity.append(
        conn,
        message="Governance settings updated",
        severity="info",
        category="governance",
        actor="refine",
    )
    if runner_call is not None:
        runner_call(M_GOVERNANCE_WAKE, {}, 10.0)
    return result


def governance_generate_rules(runner_call: RunnerCall, body: dict[str, Any]) -> dict[str, Any]:
    product = str(body.get("product") or "").strip()
    constitution = str(body.get("constitution") or "").strip()
    if not product or not constitution:
        raise ValueError("product and constitution are required")
    return runner_call(
        M_GOVERNANCE_GENERATE_RULES,
        {"product": product, "constitution": constitution},
        600.0,
    )


def quality_get(conn: sqlite3.Connection) -> dict[str, Any]:
    result = quality.load_settings(conn)
    result["enabled"] = db.get_setting(conn, "quality_enabled", "0") or "0"
    result["timing"] = quality.timing(conn)
    result["regressions_enabled"] = (
        db.get_setting(conn, "quality_regressions_enabled", "0") or "0"
    )
    result["regressions"] = regressions.list_regressions()
    result["configured"] = quality.is_configured(conn)
    return result


def quality_save(
    conn: sqlite3.Connection,
    body: dict[str, Any],
    *,
    runner_call: RunnerCall | None = None,
) -> dict[str, Any]:
    if "timing" in body:
        raw_timing = str(body.get("timing") or "").strip()
        if raw_timing not in quality.QUALITY_TIMING_VALUES:
            raise ValueError("timing must be one of pre_merge, post_rebuild")
    enabled_changed = False
    timing_changed = False
    if "enabled" in body:
        enabled = (
            "1"
            if str(body.get("enabled")).strip().lower() in {"1", "true", "yes", "on"}
            else "0"
        )
        enabled_changed = enabled != (db.get_setting(conn, "quality_enabled", "0") or "0")
        db.set_setting(conn, "quality_enabled", enabled)
    if "timing" in body:
        timing_changed = str(body.get("timing") or "").strip() != quality.timing(conn)
    if "regressions_enabled" in body:
        regressions.set_enabled(conn, body.get("regressions_enabled"))
    result = quality.save_settings(
        conn,
        business_requirements=body.get("business_requirements"),
        instructions=body.get("instructions"),
        timing_value=body.get("timing"),
    )
    result = quality_get(conn)
    activity.append(
        conn,
        message="Quality settings updated",
        severity="info",
        category="quality",
        actor="refine",
    )
    if runner_call is not None and (enabled_changed or timing_changed):
        runner_call(M_ENFORCE_SCHEDULING, {}, 10.0)
    return result


def regression_create(conn: sqlite3.Connection, body: dict[str, Any]) -> dict[str, Any]:
    title = str(body.get("title") or "").strip()
    prompt = str(body.get("prompt") or "").strip()
    description = str(body.get("description") or "").strip()
    if not title:
        title = prompt[:80].strip() or "Untitled regression"
    reg = regressions.create_regression(
        title=title,
        description=description,
        prompt=prompt,
    )
    activity.append(
        conn,
        message=f"Regression created: {reg['title']}",
        severity="info",
        category="quality",
        actor="refine",
    )
    return {"ok": True, "regression": reg}


def regression_update(regression_id: str, body: dict[str, Any]) -> dict[str, Any]:
    reg = regressions.update_regression(regression_id, body or {})
    if not reg:
        raise LookupError("Regression not found")
    return {"ok": True, "regression": reg}


def regression_delete(regression_id: str) -> dict[str, Any]:
    if not regressions.delete_regression(regression_id):
        raise LookupError("Regression not found")
    return {"ok": True}


def regression_run(runner_call: RunnerCall) -> dict[str, Any]:
    return runner_call(M_REGRESSION_RUN, {"only_enabled": True}, 900.0)

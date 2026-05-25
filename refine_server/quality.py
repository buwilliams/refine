"""Quality gate settings and prompts."""
from __future__ import annotations

from typing import Any

from refine_server import db, regressions


DEFAULT_INSTRUCTIONS = (
    "Execute the e2e tests for this Gap, if none exist, then write them. "
    "Write tests that check how the Gap is supposed to work, not based on how "
    "it is implemented. Failing tests are good when they show true failures. "
    "Certain test frameworks - especially e2e browser-based tests - are very "
    "time and resource heavy and therefore costly. Run the minimal number of "
    "tests to cover the Gap."
)


def load_settings(conn) -> dict[str, Any]:
    return {
        "business_requirements": (
            db.get_setting(conn, "quality_business_requirements", "") or ""
        ),
        "instructions": (
            db.get_setting(conn, "quality_instructions", DEFAULT_INSTRUCTIONS)
            or DEFAULT_INSTRUCTIONS
        ),
    }


def is_configured(conn) -> bool:
    settings = load_settings(conn)
    return bool(
        settings["business_requirements"].strip()
        and settings["instructions"].strip()
    )


def save_settings(
    conn,
    *,
    business_requirements: str | None = None,
    instructions: str | None = None,
) -> dict[str, Any]:
    if business_requirements is not None:
        db.set_setting(
            conn,
            "quality_business_requirements",
            str(business_requirements).strip(),
        )
    if instructions is not None:
        text = str(instructions).strip() or DEFAULT_INSTRUCTIONS
        db.set_setting(conn, "quality_instructions", text)
    return load_settings(conn)


def enabled(conn) -> bool:
    return (db.get_setting(conn, "quality_enabled", "0") or "0") == "1"


def format_prompt(
    gap: dict[str, Any],
    *,
    settings: dict[str, Any],
    regression_result: dict[str, Any] | None = None,
) -> str:
    rounds = gap.get("rounds") or []
    latest = rounds[-1] if rounds else {}
    regression_block = (
        regressions.summarize_for_prompt(regression_result)
        if regression_result is not None
        else "Managed Playwright regression checks were not run."
    )
    return (
        "You are running the pre-merge Quality gate for a software change.\n\n"
        f"Gap name:\n{str(gap.get('name') or '').strip()}\n\n"
        f"Current behavior (actual):\n{str(latest.get('actual') or '').strip()}\n\n"
        f"Desired behavior (target):\n{str(latest.get('target') or '').strip()}\n\n"
        "Business requirements:\n"
        f"{str(settings.get('business_requirements') or '').strip()}\n\n"
        "Quality instructions:\n"
        f"{str(settings.get('instructions') or '').strip()}\n\n"
        "Managed regression checks:\n"
        f"{regression_block}\n\n"
        "Run the minimum meaningful test set needed to validate this Gap. "
        "Prefer behavior-level tests over implementation-coupled tests. Avoid "
        "tests that only validate mocks, stubs, or assumptions. If the current "
        "test suite does not cover the Gap, add focused tests. If you add or "
        "update tests or managed regression specs and they pass, commit those "
        "changes on this branch. If a managed regression spec is stale or "
        "broken, repair `.refine/regressions/` and rerun the relevant check. "
        "If you find a true product or test failure, explain it clearly and "
        "exit with failure. When quality passes, exit successfully."
    )

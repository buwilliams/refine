"""Priority ordering helpers for Gap scheduling."""
from __future__ import annotations

PRIORITY_RANK = {
    "high": 0,
    "medium": 1,
    "low": 2,
}

RANK_PRIORITY = {rank: priority for priority, rank in PRIORITY_RANK.items()}

BLOCKING_STATUSES = ("todo", "in-progress", "ready-merge")
NON_BLOCKING_STATUSES = ("backlog", "review", "done", "failed", "cancelled")


def priority_rank(priority: str | None) -> int:
    return PRIORITY_RANK.get((priority or "low").lower(), PRIORITY_RANK["low"])


def priority_case_sql(column: str = "priority") -> str:
    return (
        f"CASE {column} "
        "WHEN 'high' THEN 0 "
        "WHEN 'medium' THEN 1 "
        "ELSE 2 END"
    )

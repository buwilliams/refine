"""Friendly summaries — stderr / outcome pattern → one-line actionable message.

Canonical catalog from spec.md. Adding a new error class = new entry here +
new row in the per-failure recovery reference.
"""
from __future__ import annotations

import re
from dataclasses import dataclass


@dataclass(frozen=True)
class Summary:
    category: str   # auth | git | cli | io | state | user
    severity: str   # info | warn | error
    message: str    # human-readable; may contain {placeholders}


def fmt(template: str, **kw: object) -> str:
    try:
        return template.format(**kw)
    except (KeyError, IndexError):
        return template


# ---- pattern matchers (return Summary or None) -------------------------------

_AUTH_PATTERNS = [
    re.compile(r"not logged in|authentication required|401|invalid api key", re.I),
]
_PUSH_NONFF = re.compile(r"non-fast-forward|rejected.*fetch first|updates were rejected", re.I)
_PUSH_AUTH = re.compile(r"permission denied|could not read username|authentication failed", re.I)
_PULL_NONFF = re.compile(r"refusing to merge unrelated histories|diverged|cannot fast-forward", re.I)
_PRECOMMIT = re.compile(r"pre-commit hook.*failed|pre-commit:\s*FAILED", re.I)
_PREPUSH = re.compile(r"pre-push hook.*failed|pre-push:\s*FAILED", re.I)
_RATELIMIT = re.compile(r"rate.?limit|429|too many requests", re.I)


def classify_subprocess_failure(
    *,
    stderr: str = "",
    exit_code: int | None = None,
    killed_reason: str | None = None,
    no_new_commits: bool = False,
) -> Summary:
    """Map a finished CLI subprocess outcome to a friendly summary."""
    if killed_reason == "idle":
        return Summary("cli", "error", "Agent appears stuck — no output for {idle_window}")
    if killed_reason == "hard_cap":
        return Summary("cli", "error", "Agent exceeded the {hard_cap} run cap")
    if stderr and any(p.search(stderr) for p in _AUTH_PATTERNS):
        return Summary("auth", "error", "Claude auth issue — run `claude login` on the host")
    if stderr and _RATELIMIT.search(stderr):
        return Summary("cli", "warn", "Claude rate-limited — try again shortly")
    if no_new_commits:
        return Summary(
            "cli", "warn",
            "Agent exited without producing changes — try refining the round",
        )
    # generic fallback
    return Summary("cli", "error", "Agent errored (exit {exit_code})")


def classify_git_failure(stderr: str, *, op: str) -> Summary:
    """Classify a git operation failure. `op` is one of: fetch, pull, merge, push."""
    if op == "push":
        if _PUSH_NONFF.search(stderr or ""):
            return Summary("git", "warn", "Push rejected — another developer pushed first")
        if _PUSH_AUTH.search(stderr or ""):
            return Summary(
                "git", "error",
                "Push auth failed — check SSH agent / git credentials on the host",
            )
    if op == "pull" and _PULL_NONFF.search(stderr or ""):
        return Summary(
            "git", "error",
            "Local branch diverged from remote — manual reconciliation needed",
        )
    if op == "merge" and ("CONFLICT" in (stderr or "") or "conflict" in (stderr or "").lower()):
        return Summary("git", "error", "Merge conflict")
    if _PRECOMMIT.search(stderr or ""):
        return Summary("git", "error", "Pre-commit hook failed")
    if _PREPUSH.search(stderr or ""):
        return Summary("git", "error", "Pre-push hook blocked the push")
    return Summary("git", "error", f"git {op} failed")

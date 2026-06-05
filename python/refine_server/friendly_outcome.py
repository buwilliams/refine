"""Classify the outcome of a finished subprocess into a friendly summary."""
from __future__ import annotations

import re
from dataclasses import dataclass


@dataclass(frozen=True)
class Outcome:
    kind: str          # success | failure
    category: str      # auth | cli | git | resource | state
    severity: str      # info | warn | error
    message: str
    details: str | None = None
    limit_kind: str | None = None  # rate_limit | token_limit


def classify_outcome(*, exit_code: int, killed_reason: str | None,
                     no_new_commits: bool,
                     agent_reported_success: bool | None = None,
                     failure_text: str | None = None) -> Outcome:
    if killed_reason == "idle":
        return Outcome("failure", "cli", "error", "Agent appears stuck — no output during the idle window")
    if killed_reason == "hard_cap":
        return Outcome("failure", "cli", "error", "Agent exceeded the hard wall-clock cap")
    if killed_reason == "memory_limit":
        return Outcome(
            "failure",
            "resource",
            "error",
            (
                "Agent was killed after exceeding the configured memory limit — "
                "break this Gap into a set of smaller-scope Gaps and retry"
            ),
            _trim_details(failure_text),
        )
    if killed_reason == "cpu_limit":
        return Outcome(
            "failure",
            "resource",
            "error",
            (
                "Agent was killed after exceeding a CPU limit — break this Gap "
                "into a set of smaller-scope Gaps and retry"
            ),
            _trim_details(failure_text),
        )
    if killed_reason == "cancel":
        return Outcome("failure", "state", "info", "Agent run cancelled")
    limit_kind = None
    if exit_code != 0 or agent_reported_success is False:
        limit_kind = _classify_limit_failure(failure_text)
    if limit_kind == "rate_limit":
        return Outcome(
            "failure", "cli", "warn",
            "Agent hit a rate limit — pausing agents before continuing",
            _trim_details(failure_text), "rate_limit",
        )
    if limit_kind == "token_limit":
        return Outcome(
            "failure", "cli", "warn",
            "Agent hit a token limit — pausing agents before continuing",
            _trim_details(failure_text), "token_limit",
        )
    if killed_reason == "result_grace":
        # Agent emitted its terminal `result` event but didn't actually
        # exit (almost always: it kicked off a backgrounded subprocess
        # that kept the stdio pipes open). We SIGTERMed the process
        # group; the agent's commits are still in the worktree, so this
        # is a clean wrap-up. If the agent reported success in its
        # `result` event we trust that — even with no new commits, the
        # target may already be met.
        if no_new_commits and not agent_reported_success:
            return Outcome(
                "failure", "cli", "warn",
                "Agent exited without producing changes — try refining the round",
            )
        if no_new_commits:
            return Outcome(
                "success", "cli", "info",
                "Agent reported the target was already met — no changes were needed",
            )
        return Outcome(
            "success", "cli", "info",
            "Agent run completed (backgrounded subprocesses terminated)",
        )
    if exit_code != 0:
        return Outcome("failure", "cli", "error", f"Agent errored (exit {exit_code})")
    if no_new_commits:
        # Trust the agent's own success signal over the "no commits"
        # heuristic. Gap "stop the X application" when X is already
        # stopped is a legitimate no-op success — the agent investigated,
        # confirmed actual already matches target, and exited cleanly.
        if agent_reported_success:
            return Outcome(
                "success", "cli", "info",
                "Agent reported the target was already met — no changes were needed",
            )
        return Outcome(
            "failure", "cli", "warn",
            "Agent exited without producing changes — try refining the round",
        )
    return Outcome("success", "cli", "info", "Agent run completed")


def _classify_limit_failure(text: str | None) -> str | None:
    if not text:
        return None
    lowered = text.lower()
    rate_patterns = (
        r"\brate[-_ ]?limit(?:ed|ing)?\b",
        r"\b429\b",
        r"too many requests",
        r"quota exceeded",
        r"exceeded your current quota",
        r"temporarily unavailable due to capacity",
    )
    for pattern in rate_patterns:
        if re.search(pattern, lowered):
            return "rate_limit"
    token_patterns = (
        r"\btoken[-_ ]?limit\b",
        r"\btoo many tokens\b",
        r"\bmax(?:imum)? tokens?\b",
        r"\bcontext length\b",
        r"\bcontext window\b",
        r"\bmaximum context\b",
        r"\bcontext_limit\b",
        r"\bprompt too long\b",
        r"\binput is too long\b",
        r"exceeds? (?:the )?(?:model'?s? )?(?:maximum )?(?:context|token)",
    )
    for pattern in token_patterns:
        if re.search(pattern, lowered):
            return "token_limit"
    return None


def _trim_details(text: str | None) -> str | None:
    if not text:
        return None
    stripped = text.strip()
    return stripped[:2000] if stripped else None

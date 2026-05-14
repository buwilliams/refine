"""Classify the outcome of a finished subprocess into a friendly summary."""
from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True)
class Outcome:
    kind: str          # success | failure
    category: str      # auth | cli | git | state
    severity: str      # info | warn | error
    message: str
    details: str | None = None


def classify_outcome(*, exit_code: int, killed_reason: str | None,
                     no_new_commits: bool) -> Outcome:
    if killed_reason == "idle":
        return Outcome("failure", "cli", "error", "Agent appears stuck — no output during the idle window")
    if killed_reason == "hard_cap":
        return Outcome("failure", "cli", "error", "Agent exceeded the hard wall-clock cap")
    if killed_reason == "cancel":
        return Outcome("failure", "state", "info", "Agent run cancelled")
    if killed_reason == "result_grace":
        # Agent emitted its terminal `result` event but didn't actually
        # exit (almost always: it kicked off a backgrounded subprocess
        # that kept the stdio pipes open). We SIGTERMed the process
        # group; the agent's commits are still in the worktree, so this
        # is a clean wrap-up modulo the "did anything change?" check.
        if no_new_commits:
            return Outcome(
                "failure", "cli", "warn",
                "Agent exited without producing changes — try refining the round",
            )
        return Outcome(
            "success", "cli", "info",
            "Agent run completed (backgrounded subprocesses terminated)",
        )
    if exit_code != 0:
        return Outcome("failure", "cli", "error", f"Agent errored (exit {exit_code})")
    if no_new_commits:
        return Outcome(
            "failure", "cli", "warn",
            "Agent exited without producing changes — try refining the round",
        )
    return Outcome("success", "cli", "info", "Agent run completed")

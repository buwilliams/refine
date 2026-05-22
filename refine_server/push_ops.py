"""Shared fetch/pull/push flow for target-app commits."""
from __future__ import annotations

import sqlite3
from pathlib import Path
from typing import Callable

from refine_server import activity

from . import conflict_resolver, git_ops


def push_current_after_pull(
    conn: sqlite3.Connection,
    *,
    actor: str = "runner",
    gap_id: str | None = None,
    target: str | None = None,
    conflict_branch: str | None = None,
    merge_message: str | None = None,
    prompt_context: str | None = None,
    log: Callable[..., None] | None = None,
    cwd: Path | None = None,
) -> dict:
    """Fetch, merge-pull upstream into the current branch, then push.

    Call this instead of `git push` directly. It ensures every push first
    reconciles with upstream so local Refine commits do not race remote work.
    """
    repo = cwd or git_ops.client_repo_path()
    branch = target or git_ops.current_branch(cwd=repo)
    if not branch:
        return {
            "ok": False,
            "stage": "precheck",
            "message": "Cannot push while detached from a branch.",
        }
    upstream = git_ops.upstream_branch(branch, cwd=repo)
    if upstream is None:
        return {
            "ok": True,
            "stage": "skipped",
            "branch": branch,
            "upstream": "",
            "pushed": False,
            "message": f"Branch `{branch}` has no upstream; push skipped.",
        }

    fetched = git_ops.fetch(cwd=repo)
    if not fetched.ok:
        _log(conn, gap_id, actor, log, "Fetch before push failed",
             severity="error", details=fetched.stderr or fetched.stdout)
        return {
            "ok": False,
            "stage": "fetch",
            "branch": branch,
            "upstream": upstream,
            "message": "Fetch before push failed",
            "details": fetched.stderr or fetched.stdout,
        }

    pulled = git_ops.pull_merge(cwd=repo)
    if not pulled.ok:
        blob = (pulled.stdout or "") + "\n" + (pulled.stderr or "")
        if git_ops.unmerged_paths(cwd=repo):
            if merge_message:
                _log(conn, gap_id, actor, log,
                     "Pull before push produced conflicts — attempting auto-resolve",
                     severity="warn", details=blob[:2000])
                resolved = conflict_resolver.attempt_auto_resolve(
                    conn,
                    gap_id or "",
                    branch=conflict_branch or upstream,
                    target=branch,
                    merge_message=merge_message,
                    actor=actor,
                    log=lambda message, *, severity, category, details=None: _log(
                        conn, gap_id, actor, log, message,
                        severity=severity, details=details, category=category,
                    ),
                    prompt_context=prompt_context,
                    cwd=repo,
                )
                if not resolved.get("ok"):
                    return {
                        "ok": False,
                        "stage": "pull",
                        "branch": branch,
                        "upstream": upstream,
                        "message": resolved.get("message") or "Pull before push conflicted",
                        "details": resolved.get("details") or blob,
                    }
            else:
                git_ops.merge_abort(cwd=repo)
                _log(conn, gap_id, actor, log,
                     "Pull before push conflicted; aborted merge",
                     severity="error", details=blob[:2000])
                return {
                    "ok": False,
                    "stage": "pull",
                    "branch": branch,
                    "upstream": upstream,
                    "message": "Pull before push conflicted",
                    "details": blob,
                }
        else:
            _log(conn, gap_id, actor, log, "Pull before push failed",
                 severity="error", details=blob[:2000])
            return {
                "ok": False,
                "stage": "pull",
                "branch": branch,
                "upstream": upstream,
                "message": "Pull before push failed",
                "details": blob,
            }

    pushed = git_ops.push_current(cwd=repo)
    if not pushed.ok:
        _log(conn, gap_id, actor, log, "Push failed",
             severity="error", details=pushed.stderr or pushed.stdout)
        return {
            "ok": False,
            "stage": "push",
            "branch": branch,
            "upstream": upstream,
            "message": "Push failed",
            "details": pushed.stderr or pushed.stdout,
        }

    return {
        "ok": True,
        "stage": "pushed",
        "branch": branch,
        "upstream": upstream,
        "pushed": True,
        "message": f"Pushed `{branch}` to `{upstream}`.",
    }


def _log(
    conn: sqlite3.Connection,
    gap_id: str | None,
    actor: str,
    log: Callable[..., None] | None,
    message: str,
    *,
    severity: str,
    details: str | None = None,
    category: str = "git",
) -> None:
    if log is not None:
        log(message, severity=severity, category=category, details=details)
        return
    try:
        activity.append(
            conn,
            message=message,
            severity=severity,
            category=category,
            gap_id=gap_id,
            actor=actor,
            details=details,
        )
    except Exception:
        pass

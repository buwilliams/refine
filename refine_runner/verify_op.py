"""review → done transition: fetch, pull --ff-only, merge, push.

If any step fails, the Gap stays in `review` with an explanatory log entry.
On push failure (after a successful local merge), the Gap remains in `review`
per spec — push failure is an environment issue, not a Gap-completion event.
"""
from __future__ import annotations

import sqlite3

from refine_shared import activity, db
from refine_shared.gaps import now_iso

from . import gap_writer, git_ops


def perform_verify(conn: sqlite3.Connection, gap_id: str, *,
                   actor: str = "refine") -> dict:
    """Run the merge+push sequence for a Gap currently in `review`.

    Returns a dict with keys: ok, stage, message, details.
    """
    row = conn.execute(
        "SELECT status, branch_name FROM gaps_index WHERE id = ?", (gap_id,),
    ).fetchone()
    if not row:
        return {"ok": False, "stage": "lookup", "message": "Gap not found"}
    if row["status"] != "review":
        return {"ok": False, "stage": "lookup",
                "message": f"Gap is not in review (status={row['status']})"}

    branch = row["branch_name"]
    if not branch:
        return {"ok": False, "stage": "lookup",
                "message": "Gap has no branch_name recorded"}

    # Pre-check: resolve the merge target.
    host_branch = git_ops.current_branch()
    configured = (db.get_setting(conn, "merge_target_branch") or "").strip()
    if configured:
        target = configured
        if not git_ops.local_branch_exists(target):
            msg = (f"Configured merge_target_branch `{target}` does not "
                   f"exist locally — create/track it first or clear the setting")
            _log(conn, gap_id, msg, severity="error", category="git", actor=actor)
            return {"ok": False, "stage": "precheck", "message": msg}
    else:
        if host_branch is None:
            msg = ("Client repo is in detached-HEAD state and no "
                   "merge_target_branch is configured")
            _log(conn, gap_id, msg, severity="error", category="git", actor=actor)
            return {"ok": False, "stage": "precheck", "message": msg}
        target = host_branch
    if git_ops.upstream_branch(target) is None:
        msg = f"Branch `{target}` has no upstream"
        _log(conn, gap_id, msg, severity="error", category="git", actor=actor)
        return {"ok": False, "stage": "precheck", "message": msg}

    # If the host's HEAD isn't on the target branch yet, switch to it so
    # the merge lands where the operator configured. Stash any WIP first
    # so the checkout doesn't fail. We restore the host's original branch
    # in `finally` below.
    switched_from: str | None = None
    pre_switch_stash = False
    if host_branch != target:
        if git_ops.working_copy_dirty():
            sr = git_ops.stash_push(f"refine auto-stash before switching to {target}")
            if not sr.ok:
                msg = (f"Could not stash uncommitted changes before "
                       f"switching to {target} for merge")
                _log(conn, gap_id, msg, details=sr.stderr,
                     severity="error", category="git", actor=actor)
                return {"ok": False, "stage": "precheck",
                        "message": msg, "details": sr.stderr}
            pre_switch_stash = True
            _log(conn, gap_id,
                 f"Auto-stashed WIP on `{host_branch}` before switching to `{target}`",
                 severity="info", category="git", actor=actor)
        ck = git_ops.checkout_branch(target)
        if not ck.ok:
            # Best-effort restore of the pre-switch stash before bailing.
            if pre_switch_stash:
                git_ops.stash_pop()
            msg = f"Could not check out merge target `{target}`"
            _log(conn, gap_id, msg, details=ck.stderr,
                 severity="error", category="git", actor=actor)
            return {"ok": False, "stage": "precheck",
                    "message": msg, "details": ck.stderr}
        switched_from = host_branch
        _log(conn, gap_id,
             f"Switched host HEAD: `{host_branch}` → `{target}` for merge",
             severity="info", category="git", actor=actor)

    # Auto-commit refine's own state (`.refine/`) — gap.json and friends
    # are tracked content per the spec, and the runner writes them as it
    # goes, so they show up as dirty between rounds. Commit them on the
    # target branch with a clear marker so the merge sees a clean tree.
    refine_dirty = git_ops.dirty_paths_under(".refine")
    if refine_dirty:
        cr = git_ops.add_and_commit(
            refine_dirty,
            f"refine: persist gap state ({gap_id})",
        )
        if not cr.ok:
            msg = "Could not commit refine state before merge"
            _log(conn, gap_id, msg, details=cr.stderr,
                 severity="error", category="git", actor=actor)
            return _restore_host_branch_and_return(
                conn, gap_id, switched_from, pre_switch_stash, actor,
                {"ok": False, "stage": "precheck",
                 "message": msg, "details": cr.stderr},
            )
        _log(
            conn, gap_id,
            f"Auto-committed refine state ({len(refine_dirty)} path"
            f"{'' if len(refine_dirty) == 1 else 's'}) before merge",
            severity="info", category="git", actor=actor,
        )

    # Any remaining dirty content is foreign to refine — stash it around the
    # merge and pop afterwards so the user's WIP survives the operation.
    stashed = False
    if git_ops.working_copy_dirty():
        sr = git_ops.stash_push(f"refine auto-stash for {gap_id}")
        if not sr.ok:
            msg = "Could not stash uncommitted changes before merge"
            _log(conn, gap_id, msg, details=sr.stderr,
                 severity="error", category="git", actor=actor)
            return _restore_host_branch_and_return(
                conn, gap_id, switched_from, pre_switch_stash, actor,
                {"ok": False, "stage": "precheck",
                 "message": msg, "details": sr.stderr},
            )
        stashed = True
        _log(conn, gap_id, "Auto-stashed remaining uncommitted changes before merge",
             severity="info", category="git", actor=actor)

    try:
        return _verify_body(conn, gap_id, target, branch, actor=actor)
    finally:
        if stashed:
            pop = git_ops.stash_pop()
            if not pop.ok:
                _log(
                    conn, gap_id,
                    "Auto-stash pop failed — your uncommitted changes remain "
                    "in `git stash`; resolve manually with `git stash list`",
                    details=pop.stderr,
                    severity="warn", category="git", actor=actor,
                )
        if switched_from:
            back = git_ops.checkout_branch(switched_from)
            if not back.ok:
                _log(
                    conn, gap_id,
                    f"Could not restore host HEAD to `{switched_from}` after "
                    f"verify — host is still on `{target}`",
                    details=back.stderr,
                    severity="warn", category="git", actor=actor,
                )
            elif pre_switch_stash:
                pop2 = git_ops.stash_pop()
                if not pop2.ok:
                    _log(
                        conn, gap_id,
                        f"Auto-stash pop on `{switched_from}` failed — "
                        f"your WIP remains in `git stash`",
                        details=pop2.stderr,
                        severity="warn", category="git", actor=actor,
                    )


def _restore_host_branch_and_return(conn, gap_id, switched_from,
                                     pre_switch_stash, actor, ret):
    """Pre-merge failure escape hatch: get the host's HEAD back to where
    it was before we touched it, then return the supplied error dict."""
    if switched_from:
        back = git_ops.checkout_branch(switched_from)
        if not back.ok:
            _log(conn, gap_id,
                 f"Could not restore host HEAD to `{switched_from}` after "
                 f"precheck failure",
                 details=back.stderr,
                 severity="warn", category="git", actor=actor)
        elif pre_switch_stash:
            pop = git_ops.stash_pop()
            if not pop.ok:
                _log(conn, gap_id,
                     f"Auto-stash pop on `{switched_from}` failed",
                     details=pop.stderr,
                     severity="warn", category="git", actor=actor)
    return ret


def _verify_body(conn: sqlite3.Connection, gap_id: str, current: str,
                 branch: str, *, actor: str) -> dict:
    # 1. fetch
    r = git_ops.fetch()
    if not r.ok:
        _log(conn, gap_id, "git fetch failed during verify", details=r.stderr,
             severity="error", category="git", actor=actor)
        return {"ok": False, "stage": "fetch", "message": "git fetch failed",
                "details": r.stderr}

    # 2. pull --ff-only
    r = git_ops.pull_ff_only()
    if not r.ok:
        _log(conn, gap_id,
             "Local branch diverged from remote — manual reconciliation needed",
             details=r.stderr, severity="error", category="git", actor=actor)
        return {"ok": False, "stage": "pull",
                "message": "Local branch diverged from remote", "details": r.stderr}

    # 3. merge (idempotent if already merged)
    if git_ops.is_already_merged(branch):
        _log(conn, gap_id, "Branch already merged into current — proceeding to push",
             severity="info", category="git", actor=actor)
    else:
        merge_message = _build_merge_message(conn, gap_id, branch, current)
        r = git_ops.merge_branch(branch, message=merge_message)
        if not r.ok:
            stderr = r.stderr + ("\n" + r.stdout if r.stdout else "")
            if "CONFLICT" in stderr or "conflict" in stderr.lower():
                _log(conn, gap_id, "Merge conflict — leave the worktree intact",
                     details=stderr, severity="error", category="git", actor=actor)
                return {"ok": False, "stage": "merge",
                        "message": "Merge conflict", "details": stderr}
            _log(conn, gap_id, "git merge failed", details=stderr,
                 severity="error", category="git", actor=actor)
            return {"ok": False, "stage": "merge", "message": "git merge failed",
                    "details": stderr}

    # 4. push (one retry on non-FF)
    r = git_ops.push_current()
    if not r.ok and ("non-fast-forward" in r.stderr or
                     "fetch first" in r.stderr or
                     "rejected" in r.stderr):
        # Retry: re-fetch, re-pull --ff-only, re-merge if needed, re-push.
        f2 = git_ops.fetch()
        if f2.ok:
            p2 = git_ops.pull_ff_only()
            if p2.ok and not git_ops.is_already_merged(branch):
                git_ops.merge_branch(branch, message=f"Merge {branch}")
            r = git_ops.push_current()
    if not r.ok:
        _log(conn, gap_id,
             "Push failed — environment issue; Gap stays in `review`",
             details=r.stderr, severity="error", category="git", actor=actor)
        # Per spec: gap STAYS IN REVIEW, does NOT transition to done.
        return {"ok": False, "stage": "push",
                "message": "Push failed", "details": r.stderr}

    # All four steps succeeded → done; clean up branch + worktree.
    with db.transaction(conn):
        conn.execute(
            "UPDATE gaps_index SET status = 'done', updated = ? WHERE id = ?",
            (now_iso(), gap_id),
        )
    git_ops.remove_worktree(gap_id)
    git_ops.delete_branch(branch)
    activity.append(
        conn,
        message="Gap merged + pushed; transitioned to done",
        severity="info", category="state", gap_id=gap_id, actor=actor,
    )
    return {"ok": True, "stage": "done", "message": "Merged and pushed"}


def _log(conn: sqlite3.Connection, gap_id: str, message: str, *,
         severity: str, category: str, actor: str,
         details: str | None = None) -> None:
    # Append to latest round's logs[] + activity
    row = conn.execute(
        "SELECT json_path FROM gaps_index WHERE id = ?", (gap_id,),
    ).fetchone()
    if row:
        from refine_shared.gaps import read_gap_json
        gap = read_gap_json(gap_id)
        if gap and gap.get("rounds"):
            try:
                gap_writer.append_round_log(
                    gap_id=gap_id, round_idx=len(gap["rounds"]) - 1,
                    severity=severity, category=category,
                    message=message, details=details, actor=actor,
                )
            except Exception:
                pass
    activity.append(
        conn, message=message, severity=severity, category=category,
        gap_id=gap_id, actor=actor, details=details,
    )


def _build_merge_message(conn: sqlite3.Connection, gap_id: str,
                          branch: str, current: str) -> str:
    """Build a descriptive merge commit message:

        Merge refine/<gap_id> into <current>: <gap name>

        <latest round target — what we asked the agent to do>

        Commits on this branch:
        - <commit 1 subject>
        - <commit 2 subject>
        ...

        Refine Gap: <gap_id>
    """
    from refine_shared.gaps import read_gap_json
    from . import git_ops

    # Gap name from SQLite.
    row = conn.execute(
        "SELECT name FROM gaps_index WHERE id = ?", (gap_id,),
    ).fetchone()
    gap_name = (row["name"] if row else "") or gap_id

    # Latest round target (the asked-for behavior).
    target = ""
    try:
        gap_json = read_gap_json(gap_id) or {}
        rounds = gap_json.get("rounds") or []
        if rounds:
            target = (rounds[-1].get("target") or "").strip()
    except Exception:
        pass

    # Commit subjects on the branch (relative to its merge-base with current).
    subjects: list[str] = []
    base = git_ops._run(["merge-base", current, branch])
    if base.ok and base.stdout.strip():
        log = git_ops._run([
            "log", "--no-merges", "--reverse", "--pretty=%s",
            f"{base.stdout.strip()}..{branch}",
        ])
        if log.ok:
            subjects = [s for s in log.stdout.splitlines() if s.strip()]

    lines: list[str] = [f"Merge {branch} into {current}: {gap_name}"]
    if target:
        truncated = target if len(target) <= 500 else target[:500] + "…"
        lines += ["", truncated]
    if subjects:
        lines += ["", "Commits on this branch:"]
        for s in subjects[:20]:
            lines.append(f"- {s}")
        if len(subjects) > 20:
            lines.append(f"- … and {len(subjects) - 20} more")
    lines += ["", f"Refine Gap: {gap_id}"]
    return "\n".join(lines)

"""Git operations against the client repo.

Operational assumption (per spec): the host running refine is dedicated to
refine; no human edits the working copy directly; all local commits on the
client's current branch come from refine's agent runs.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


# Merge commits made by refine's Merge agent end with a `Refine Gap: <id>`
# trailer (see refine_server.verify_op._build_merge_message). We use the
# trailer to recover gap_id from a merge commit on the target branch.
_REFINE_GAP_FOOTER = re.compile(
    r"^\s*Refine Gap:\s*([0-9A-Za-z]{26})\s*$", re.MULTILINE,
)
_LOG_RECORD_SEP = "\x1e"
_LOG_FIELD_SEP = "\x1f"
REFINE_SQLITE_PATHS = (
    ".refine/index.sqlite",
    ".refine/index.sqlite-shm",
    ".refine/index.sqlite-wal",
)
REFINE_RUNTIME_EXACT_PATHS = (
    *REFINE_SQLITE_PATHS,
    ".refine/app.log",
    ".refine/app.pid",
)


def client_repo_path() -> Path:
    """The client repo, as configured in refine.toml."""
    from refine_server import config
    return config.get().client_repo


def worktrees_dir() -> Path:
    return client_repo_path() / ".git" / "refine-worktrees"


@dataclass
class GitResult:
    ok: bool
    stdout: str
    stderr: str
    code: int


def _run(args: list[str], *, cwd: Path | None = None, env: dict | None = None,
         timeout: float | None = 120.0) -> GitResult:
    # Default to the client repo. Without this, callers that forget to pass
    # cwd inherit the runner's process cwd — which is usually the refine
    # source clone (itself a git repo), causing `git worktree add` and friends
    # to operate on refine instead of the target project.
    run_cwd = cwd if cwd is not None else client_repo_path()
    proc = subprocess.run(
        ["git", *args],
        cwd=str(run_cwd),
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    return GitResult(
        ok=(proc.returncode == 0),
        stdout=proc.stdout,
        stderr=proc.stderr,
        code=proc.returncode,
    )


# ---- pre-checks --------------------------------------------------------------

def current_branch(cwd: Path | None = None) -> str | None:
    r = _run(["symbolic-ref", "--quiet", "--short", "HEAD"], cwd=cwd or client_repo_path())
    if not r.ok:
        return None  # detached HEAD
    return r.stdout.strip()


def upstream_branch(branch: str, cwd: Path | None = None) -> str | None:
    r = _run(
        ["rev-parse", "--abbrev-ref", f"{branch}@{{upstream}}"],
        cwd=cwd or client_repo_path(),
    )
    if not r.ok:
        return None
    return r.stdout.strip()


def working_copy_dirty(cwd: Path | None = None) -> bool:
    r = _run(["status", "--porcelain"], cwd=cwd or client_repo_path())
    return bool(r.ok and r.stdout.strip())


def dirty_paths(cwd: Path | None = None) -> list[str]:
    r = _run(["status", "--porcelain"], cwd=cwd or client_repo_path())
    if not r.ok:
        return []
    paths: list[str] = []
    for line in r.stdout.splitlines():
        if len(line) < 3:
            continue
        path_part = line[3:]
        if " -> " in path_part:
            path_part = path_part.split(" -> ", 1)[1]
        paths.append(path_part.strip().strip('"'))
    return paths


def fetch(cwd: Path | None = None) -> GitResult:
    return _run(["fetch", "--prune"], cwd=cwd or client_repo_path(), timeout=300.0)


def ensure_info_exclude(pattern: str, cwd: Path | None = None) -> None:
    root = cwd or client_repo_path()
    r = _run(["rev-parse", "--git-dir"], cwd=root)
    if not r.ok:
        return
    git_dir = (root / r.stdout.strip()).resolve()
    path = git_dir / "info" / "exclude"
    path.parent.mkdir(parents=True, exist_ok=True)
    existing = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    if pattern not in existing:
        existing.append(pattern)
        path.write_text("\n".join(existing).rstrip() + "\n", encoding="utf-8")


def rev_parse(ref: str = "HEAD", cwd: Path | None = None) -> str | None:
    r = _run(["rev-parse", "--verify", ref], cwd=cwd or client_repo_path())
    if not r.ok:
        return None
    return r.stdout.strip()


def rev_list_count(base_ref: str, tip_ref: str,
                   cwd: Path | None = None) -> int:
    r = _run(
        ["rev-list", "--count", f"{base_ref}..{tip_ref}"],
        cwd=cwd or client_repo_path(),
    )
    if not r.ok:
        return 0
    try:
        return int(r.stdout.strip())
    except ValueError:
        return 0


def stash_push(message: str, *, cwd: Path | None = None) -> GitResult:
    """Stash all uncommitted changes (incl. untracked) so we can run a clean
    git operation. Returns a result whose stdout we don't really care about;
    callers should test `ok` and pair with `stash_pop`.
    """
    return _run(
        ["stash", "push", "--include-untracked", "-m", message],
        cwd=cwd or client_repo_path(),
    )


def stash_pop(cwd: Path | None = None) -> GitResult:
    return _run(["stash", "pop"], cwd=cwd or client_repo_path())


def reset_unmerged_index_preserving_wip(
    message: str,
    *,
    cwd: Path | None = None,
) -> dict:
    """Clear sentinel-less unmerged index state without silently dropping WIP.

    `git stash push` refuses to run while the index has unmerged entries. To
    keep cleanup recoverable, capture tracked changes as a binary patch first,
    reset the index/worktree, then reapply that patch and stash the resulting
    ordinary dirty tree together with untracked files.
    """
    repo = cwd or client_repo_path()
    patch = _run(["diff", "HEAD", "--binary"], cwd=repo)
    if not patch.ok:
        return {
            "ok": False,
            "message": "Could not snapshot dirty worktree before reset",
            "details": patch.stderr or patch.stdout,
        }

    patch_path: Path | None = None
    if patch.stdout.strip():
        git_dir = _git_dir(repo)
        if git_dir is None:
            return {
                "ok": False,
                "message": "Could not locate git dir for cleanup rescue patch",
            }
        rescue_dir = git_dir / "refine-rescue"
        rescue_dir.mkdir(parents=True, exist_ok=True)
        patch_path = (
            rescue_dir
            / f"{int(time.time())}-{os.getpid()}-cleanup.patch"
        )
        patch_path.write_text(patch.stdout, encoding="utf-8")

    reset = _run(["reset", "--hard", "HEAD"], cwd=repo)
    if not reset.ok:
        return {
            "ok": False,
            "message": "Could not reset unmerged index state",
            "details": reset.stderr or reset.stdout,
            "patch_path": str(patch_path) if patch_path else "",
        }

    if patch_path is None:
        return {
            "ok": True,
            "stashed": False,
            "message": "Reset unmerged index state",
            "details": reset.stdout,
        }

    apply = _run(
        ["apply", "--whitespace=nowarn", str(patch_path)],
        cwd=repo,
    )
    if not apply.ok:
        return {
            "ok": True,
            "stashed": False,
            "message": "Reset unmerged index state; dirty worktree patch preserved",
            "details": (
                f"Patch: {patch_path}\n"
                f"git apply failed:\n{apply.stderr or apply.stdout}"
            ),
            "patch_path": str(patch_path),
        }

    stash = stash_push(message, cwd=repo)
    if stash.ok:
        try:
            patch_path.unlink()
        except OSError:
            pass
        return {
            "ok": True,
            "stashed": True,
            "message": "Reset unmerged index state; dirty worktree saved to stash",
            "details": stash.stdout or stash.stderr,
        }

    cleanup = _run(["reset", "--hard", "HEAD"], cwd=repo)
    return {
        "ok": True,
        "stashed": False,
        "message": "Reset unmerged index state; dirty worktree patch preserved",
        "details": (
            f"Patch: {patch_path}\n"
            f"git stash failed:\n{stash.stderr or stash.stdout}\n"
            f"cleanup reset:\n{cleanup.stderr or cleanup.stdout}"
        ),
        "patch_path": str(patch_path),
    }


def unmerged_paths(cwd: Path | None = None) -> list[str]:
    """Files left in conflict state by a half-finished merge.

    Returns repo-relative paths that `git diff --name-only --diff-filter=U`
    reports — i.e., entries that still have <<<<<<< markers and aren't
    staged as resolved.
    """
    r = _run(["diff", "--name-only", "--diff-filter=U"],
             cwd=cwd or client_repo_path())
    if not r.ok:
        return []
    return [ln.strip() for ln in r.stdout.splitlines() if ln.strip()]


def commit_pending_merge(message: str, *,
                          cwd: Path | None = None) -> GitResult:
    """Commit the in-progress merge — assumes `MERGE_HEAD` is set and
    all conflicting files have already been staged. Produces a proper
    two-parent merge commit so the `Refine Gap:` trailer in `message`
    lands on a commit the Changes screen can list."""
    return _run(["commit", "--no-edit", "-m", message],
                 cwd=cwd or client_repo_path())


def head_parents(cwd: Path | None = None) -> list[str]:
    """SHA list of HEAD's parents. Empty if HEAD doesn't resolve."""
    r = _run(["log", "-1", "--format=%P"], cwd=cwd or client_repo_path())
    if not r.ok:
        return []
    return [p for p in r.stdout.strip().split() if p]


def in_progress_op(cwd: Path | None = None) -> tuple[str, str] | None:
    """Detect a half-finished git operation in the client repo.

    Returns `(op_name, recovery_hint)` when one of merge / rebase /
    cherry-pick / revert / am / bisect has left state behind in `.git/`,
    or when the index still has unmerged entries from an operation like
    a conflicted `git stash apply`. Returns `None` when the repo is in a
    clean operational state.

    Catches the common refine failure mode where an earlier verify
    merged a Gap's branch into the target, hit code-level conflicts,
    and the conflicts were never resolved — every subsequent verify
    then trips on `git commit` (MERGE_HEAD blocks non-merge commits).
    """
    root = cwd or client_repo_path()
    git_dir = _git_dir(root)
    if git_dir is None:
        return None
    checks = (
        ("MERGE_HEAD",            "merge",
         "Run `git merge --abort` to discard, or resolve conflicts and "
         "`git commit` to finish."),
        ("REBASE_HEAD",           "rebase",
         "Run `git rebase --abort` to discard, or resolve and "
         "`git rebase --continue`."),
        ("rebase-merge",          "rebase",
         "Run `git rebase --abort` to discard, or resolve and "
         "`git rebase --continue`."),
        ("rebase-apply",          "rebase",
         "Run `git rebase --abort` to discard, or resolve and "
         "`git rebase --continue`."),
        ("CHERRY_PICK_HEAD",      "cherry-pick",
         "Run `git cherry-pick --abort` to discard, or resolve and "
         "`git cherry-pick --continue`."),
        ("REVERT_HEAD",           "revert",
         "Run `git revert --abort` to discard, or resolve and "
         "`git revert --continue`."),
        ("BISECT_LOG",            "bisect",
         "Run `git bisect reset` when finished."),
    )
    for name, op, hint in checks:
        if (git_dir / name).exists():
            return (op, hint)
    unmerged = unmerged_paths(cwd=root)
    if unmerged:
        hint = (
            "Resolve the conflicted paths and `git add` them, or run "
            "`git reset --hard HEAD` to discard the unmerged index state."
        )
        return ("unmerged-index", hint)
    return None


def _git_dir(root: Path) -> Path | None:
    # Locate the actual `.git` dir; in a worktree `.git` is a file
    # pointing at the real gitdir. Use `rev-parse --git-dir` to resolve.
    r = _run(["rev-parse", "--git-dir"], cwd=root)
    if not r.ok:
        return None
    return (root / r.stdout.strip()).resolve()


def dirty_paths_under(prefix: str) -> list[str]:
    """Return repo-relative paths reported by `git status --porcelain` that
    sit under `prefix`. `prefix` is matched as a path segment.
    """
    r = _run(["status", "--porcelain", "--", prefix])
    if not r.ok:
        return []
    paths: list[str] = []
    for line in r.stdout.splitlines():
        # Porcelain format: "XY <path>" (or "XY <path> -> <newpath>" for renames)
        if len(line) < 3:
            continue
        path_part = line[3:]
        if " -> " in path_part:
            path_part = path_part.split(" -> ", 1)[1]
        paths.append(path_part)
    return paths


def is_refine_sqlite_path(path: str) -> bool:
    return path.strip().strip('"') in REFINE_SQLITE_PATHS


def is_refine_runtime_path(path: str) -> bool:
    clean = path.strip().strip('"')
    return (
        clean in REFINE_RUNTIME_EXACT_PATHS
        or clean.startswith(".refine/logs/")
        or clean.startswith(".refine/run/")
        or (clean.startswith(".refine/") and clean.endswith("/logs.jsonl"))
    )


def syncable_refine_paths(paths: list[str]) -> list[str]:
    """Return dirty .refine paths worth committing for cross-instance sync."""
    out: list[str] = []
    for path in paths:
        clean = path.strip().strip('"')
        if not (clean == ".refine" or clean.startswith(".refine/")):
            continue
        if is_refine_runtime_path(clean):
            continue
        if clean not in out:
            out.append(clean)
    return out


def untrack_refine_sqlite(*, cwd: Path | None = None) -> GitResult:
    """Remove disposable SQLite cache files from Git while leaving them on disk."""
    return _run(
        ["rm", "--cached", "-f", "--ignore-unmatch", "--", *REFINE_SQLITE_PATHS],
        cwd=cwd or client_repo_path(),
    )


def tracked_refine_runtime_paths(*, cwd: Path | None = None) -> list[str]:
    r = _run(["ls-files", "--", ".refine"], cwd=cwd or client_repo_path())
    if not r.ok:
        return []
    return [
        line.strip()
        for line in r.stdout.splitlines()
        if line.strip() and is_refine_runtime_path(line.strip())
    ]


def untrack_refine_runtime(*, cwd: Path | None = None) -> GitResult:
    """Remove disposable Refine runtime files from Git while leaving them on disk."""
    paths = tracked_refine_runtime_paths(cwd=cwd or client_repo_path())
    if not paths:
        return GitResult(ok=True, stdout="", stderr="", code=0)
    return _run(
        ["rm", "--cached", "-f", "--ignore-unmatch", "--", *paths],
        cwd=cwd or client_repo_path(),
    )


def staged_refine_runtime_removals(*, cwd: Path | None = None) -> list[str]:
    r = _run(
        [
            "diff", "--cached", "--name-only", "--diff-filter=D",
            "--", ".refine",
        ],
        cwd=cwd or client_repo_path(),
    )
    if not r.ok:
        return []
    return [
        line.strip()
        for line in r.stdout.splitlines()
        if line.strip() and is_refine_runtime_path(line.strip())
    ]


def staged_paths(*, cwd: Path | None = None) -> list[str]:
    r = _run(["diff", "--cached", "--name-only"], cwd=cwd or client_repo_path())
    if not r.ok:
        return []
    return [line.strip() for line in r.stdout.splitlines() if line.strip()]


def staged_refine_sqlite_removals(*, cwd: Path | None = None) -> list[str]:
    return [
        path for path in staged_refine_runtime_removals(cwd=cwd)
        if is_refine_sqlite_path(path)
    ]


def commit_refine_sync_state(
    paths: list[str],
    *,
    state_message: str = "refine: sync project state",
    cleanup_message: str = "refine: stop tracking runtime state",
    cwd: Path | None = None,
) -> GitResult:
    """Commit only cross-instance Refine state, plus one-time runtime untracking.

    Per-round logs, process logs, PID files, and SQLite are high-churn local
    runtime files. They may need to be removed from the index once, but they
    should not be re-added or drive recurring sync commits.
    """
    repo = cwd or client_repo_path()
    rm = untrack_refine_runtime(cwd=repo)
    if not rm.ok:
        return rm
    state_paths = syncable_refine_paths(paths)
    cleanup_paths = staged_refine_runtime_removals(cwd=repo)
    commit_paths = list(dict.fromkeys([*state_paths, *cleanup_paths]))
    if not commit_paths:
        return GitResult(ok=True, stdout="", stderr="(nothing to commit)", code=0)
    if state_paths:
        add = _run(["add", "--", *state_paths], cwd=repo)
        if not add.ok:
            return add
    staged_now = staged_paths(cwd=repo)
    allowed_staged = set(syncable_refine_paths(staged_now))
    allowed_staged.update(staged_refine_runtime_removals(cwd=repo))
    unexpected_staged = [path for path in staged_now if path not in allowed_staged]
    if unexpected_staged:
        return GitResult(
            ok=False,
            stdout="",
            stderr=(
                "Refusing to commit unrelated staged paths: "
                + ", ".join(unexpected_staged)
            ),
            code=1,
        )
    message = state_message if state_paths else cleanup_message
    return _run([
        "-c", "user.email=refine@localhost",
        "-c", "user.name=refine",
        "commit", "-m", message,
    ], cwd=repo)


def add_and_commit(paths: list[str], message: str,
                   *, cwd: Path | None = None) -> GitResult:
    """Stage the given paths and commit them. No-op if nothing to commit
    (we don't try `commit --allow-empty`)."""
    should_untrack_sqlite = any(
        p == ".refine" or p.startswith(".refine/") for p in paths
    )
    commit_paths = list(paths)
    if should_untrack_sqlite:
        rm = untrack_refine_sqlite(cwd=cwd or client_repo_path())
        if not rm.ok:
            return rm
        commit_paths = [
            p for p in commit_paths if not is_refine_sqlite_path(p)
        ]
        commit_paths.extend(staged_refine_sqlite_removals(cwd=cwd or client_repo_path()))
    add_paths = [p for p in paths if not is_refine_sqlite_path(p)]
    if not add_paths and not commit_paths:
        return GitResult(ok=True, stdout="", stderr="(nothing to commit)", code=0)
    if add_paths:
        add = _run(["add", "--", *add_paths], cwd=cwd or client_repo_path())
        if not add.ok:
            return add
    return _run(
        ["commit", "-m", message, "--", *commit_paths],
        cwd=cwd or client_repo_path(),
    )


# ---- worktree management -----------------------------------------------------

def gap_worktree_path(gap_id: str) -> Path:
    return worktrees_dir() / gap_id.upper()


def apply_agent_subpath(root: Path, subpath: str | None,
                         *, log: Callable[[str], None] | None = None) -> Path:
    """Resolve the agent/chat working directory for a given base `root`.

    `subpath` is the operator-configured `agent_subpath` setting (a
    repo-relative path). When non-empty and the resolved subdir exists
    under `root`, return that joined path; otherwise return `root` and
    optionally log a warning. Git plumbing always stays at `root` —
    only the agent subprocess `cwd` changes.
    """
    if not subpath:
        return root
    candidate = (root / subpath).resolve()
    # Confine to `root` — don't follow a symlink or `..` outside the worktree.
    try:
        candidate.relative_to(root.resolve())
    except ValueError:
        if log:
            log(f"agent_subpath {subpath!r} resolves outside {root}; using root")
        return root
    if not candidate.is_dir():
        if log:
            log(f"agent_subpath {subpath!r} does not exist under {root}; using root")
        return root
    return candidate


def worktree_exists(gap_id: str) -> bool:
    return gap_worktree_path(gap_id).exists()


def create_worktree(gap_id: str, base_ref: str, branch_name: str) -> GitResult:
    """Create a worktree at .git/refine-worktrees/<GAP_ID> tracking branch_name based on base_ref.

    If the branch already exists, reuse it. If the target path already exists,
    either reuse it (when git already knows it as a worktree) or remove the
    orphan directory and create fresh — typical after a runner crash, or as
    leftover from the pre-fix cwd bug that registered worktrees against the
    refine source clone instead of the client repo.
    """
    worktrees_dir().mkdir(parents=True, exist_ok=True)
    wt = gap_worktree_path(gap_id)
    if wt.exists():
        if _is_registered_worktree(wt):
            return GitResult(
                ok=True, stdout="", stderr="(reused existing worktree)", code=0,
            )
        try:
            shutil.rmtree(wt)
        except OSError as e:
            return GitResult(
                ok=False, stdout="",
                stderr=f"orphan worktree at {wt} could not be removed: {e}",
                code=1,
            )
    # is the branch already created?
    exists = _run(["rev-parse", "--verify", "--quiet", f"refs/heads/{branch_name}"]).ok
    if exists:
        return _run(["worktree", "add", str(wt), branch_name])
    return _run(["worktree", "add", "-b", branch_name, str(wt), base_ref])


def _is_registered_worktree(path: Path) -> bool:
    """Is `path` listed by `git worktree list` in the client repo?"""
    r = _run(["worktree", "list", "--porcelain"])
    if not r.ok:
        return False
    try:
        target = str(path.resolve())
    except OSError:
        return False
    for line in r.stdout.splitlines():
        if not line.startswith("worktree "):
            continue
        wt_path = line[len("worktree "):].strip()
        try:
            if str(Path(wt_path).resolve()) == target:
                return True
        except OSError:
            continue
    return False


def remove_worktree(gap_id: str, *, force: bool = True) -> GitResult:
    wt = gap_worktree_path(gap_id)
    if not wt.exists():
        return GitResult(ok=True, stdout="", stderr="(no worktree)", code=0)
    args = ["worktree", "remove"]
    if force:
        args.append("--force")
    args.append(str(wt))
    return _run(args)


def delete_branch(branch_name: str, *, force: bool = True) -> GitResult:
    args = ["branch", "-D" if force else "-d", branch_name]
    return _run(args)


def commits_on_branch_since(base_ref: str, cwd: Path) -> int:
    r = _run(["rev-list", "--count", f"{base_ref}..HEAD"], cwd=cwd)
    if not r.ok:
        return 0
    try:
        return int(r.stdout.strip())
    except ValueError:
        return 0


# ---- merge & push (ready-merge → awaiting-rebuild) ----------------------------

def pull_ff_only(cwd: Path | None = None) -> GitResult:
    return _run(["pull", "--ff-only", "--no-rebase"], cwd=cwd or client_repo_path())


def pull_merge(cwd: Path | None = None) -> GitResult:
    return _run(["pull", "--no-rebase", "--no-edit"], cwd=cwd or client_repo_path())


def merge_abort(cwd: Path | None = None) -> GitResult:
    return _run(["merge", "--abort"], cwd=cwd or client_repo_path())


def merge_branch(branch: str, *, cwd: Path | None = None,
                 message: str | None = None,
                 no_ff: bool = False) -> GitResult:
    args = ["merge", "--no-edit"]
    if no_ff:
        args.append("--no-ff")
    if message:
        args.extend(["-m", message])
    args.append(branch)
    return _run(args, cwd=cwd or client_repo_path())


def push_current(cwd: Path | None = None) -> GitResult:
    return _run(["push"], cwd=cwd or client_repo_path(), timeout=300.0)


def _parse_refine_merge_log(stdout: str, branch: str) -> list[dict]:
    out: list[dict] = []
    for chunk in stdout.split(_LOG_RECORD_SEP):
        chunk = chunk.strip("\x00\n ")
        if not chunk:
            continue
        parts = chunk.split(_LOG_FIELD_SEP, 3)
        if len(parts) != 4:
            continue
        sha, committed, subject, body = parts
        m = _REFINE_GAP_FOOTER.search(body)
        if not m:
            continue
        out.append({
            "commit": sha,
            "committed": committed,
            "subject": subject,
            "gap_id": m.group(1),
            "branch": branch,
        })
    return out


def list_refine_merges(branch: str, limit: int = 50,
                        *, offset: int = 0,
                        cwd: Path | None = None) -> list[dict]:
    """Walk `branch` for merge commits refine produced.

    A refine merge has the trailer `Refine Gap: <gap_id>` in its body
    (verify_op._build_merge_message). Returns the matching commits as
    `[{commit, committed, subject, gap_id, branch}]`, newest first.
    """
    fmt = _LOG_FIELD_SEP.join(["%H", "%cI", "%s", "%B"]) + _LOG_RECORD_SEP
    r = _run([
        "log", "--first-parent", "--merges",
        f"--max-count={int(limit)}",
        f"--skip={max(0, int(offset))}",
        f"--pretty=format:{fmt}",
        branch,
    ], cwd=cwd or client_repo_path())
    if not r.ok:
        return []
    return _parse_refine_merge_log(r.stdout, branch)


def list_all_refine_merges(branch: str, *,
                           cwd: Path | None = None) -> list[dict]:
    """Walk all first-parent merge commits on `branch` and return refine ones."""
    fmt = _LOG_FIELD_SEP.join(["%H", "%cI", "%s", "%B"]) + _LOG_RECORD_SEP
    r = _run([
        "log", "--first-parent", "--merges",
        f"--pretty=format:{fmt}",
        branch,
    ], cwd=cwd or client_repo_path())
    if not r.ok:
        return []
    return _parse_refine_merge_log(r.stdout, branch)


def refine_merge_for_commit(commit_sha: str, *, branch: str,
                            cwd: Path | None = None) -> dict | None:
    """Return refine merge metadata for one commit, or None if it is not one."""
    fmt = _LOG_FIELD_SEP.join(["%H", "%cI", "%s", "%B"]) + _LOG_RECORD_SEP
    r = _run([
        "show", "-s", f"--pretty=format:{fmt}", commit_sha,
    ], cwd=cwd or client_repo_path())
    if not r.ok:
        return None
    rows = _parse_refine_merge_log(r.stdout, branch)
    return rows[0] if rows else None


def rev_parse(ref: str, *, cwd: Path | None = None) -> str | None:
    r = _run(["rev-parse", "--verify", ref],
             cwd=cwd or client_repo_path())
    if not r.ok:
        return None
    return r.stdout.strip()


def count_refine_merges_for_gap(gap_id: str, branch: str, *,
                                  cwd: Path | None = None) -> int:
    """Count merge commits on `branch` whose body carries
    `Refine Gap: <gap_id>`. One merge commit per completed round, so
    the dispatcher uses `count >= len(rounds)` as the "this round's
    work is already merged" signal."""
    gap_id_upper = gap_id.strip().upper()
    if not gap_id_upper:
        return 0
    # Walk first-parent merges; match the trailer in the body. Cheaper
    # than parsing every commit — git filters server-side.
    r = _run([
        "log", "--first-parent", "--merges",
        f"--grep=^Refine Gap: {gap_id_upper}$",
        "--extended-regexp", "--pretty=format:%H",
        branch,
    ], cwd=cwd or client_repo_path())
    if not r.ok:
        return 0
    return sum(1 for ln in r.stdout.splitlines() if ln.strip())


def gap_id_from_commit(commit_sha: str, *,
                       cwd: Path | None = None) -> str | None:
    """Read the body of `commit_sha` and pull out its `Refine Gap:` trailer."""
    r = _run(["show", "-s", "--pretty=%B", commit_sha],
             cwd=cwd or client_repo_path())
    if not r.ok:
        return None
    m = _REFINE_GAP_FOOTER.search(r.stdout)
    return m.group(1) if m else None


def revert_merge_commit(commit_sha: str, *,
                         cwd: Path | None = None) -> GitResult:
    """`git revert -m 1 --no-edit <merge>` on the current branch. The
    caller is responsible for being on the right branch and for handling
    conflicts (look at `stderr` / `stdout` for "CONFLICT")."""
    return _run(
        ["revert", "-m", "1", "--no-edit", commit_sha],
        cwd=cwd or client_repo_path(),
    )


def revert_abort(cwd: Path | None = None) -> GitResult:
    return _run(["revert", "--abort"], cwd=cwd or client_repo_path())


def local_branch_exists(branch: str, cwd: Path | None = None) -> bool:
    """True if `branch` exists as a local ref in the client repo."""
    r = _run(
        ["show-ref", "--verify", "--quiet", f"refs/heads/{branch}"],
        cwd=cwd or client_repo_path(),
    )
    return r.ok


def checkout_branch(branch: str, cwd: Path | None = None) -> GitResult:
    """`git checkout <branch>` in the client repo. Fails if the working
    copy is dirty in ways that conflict — callers should stash first."""
    return _run(["checkout", branch], cwd=cwd or client_repo_path())


def is_already_merged(branch: str, cwd: Path | None = None) -> bool:
    """Check if `branch` is reachable from current HEAD (i.e., already merged)."""
    r = _run(
        ["merge-base", "--is-ancestor", branch, "HEAD"],
        cwd=cwd or client_repo_path(),
    )
    return r.ok  # exit 0 = is ancestor, 1 = not

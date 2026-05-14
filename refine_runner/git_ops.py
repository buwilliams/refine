"""Git operations against the client repo.

Operational assumption (per spec): the host running refine is dedicated to
refine; no human edits the working copy directly; all local commits on the
client's current branch come from refine's agent runs.
"""
from __future__ import annotations

import os
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


def client_repo_path() -> Path:
    """The client repo, as configured in refine.toml."""
    from refine_shared import config
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


def fetch(cwd: Path | None = None) -> GitResult:
    return _run(["fetch", "--prune"], cwd=cwd or client_repo_path(), timeout=300.0)


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


def add_and_commit(paths: list[str], message: str,
                   *, cwd: Path | None = None) -> GitResult:
    """Stage the given paths and commit them. No-op if nothing to commit
    (we don't try `commit --allow-empty`)."""
    if not paths:
        return GitResult(ok=True, stdout="", stderr="(nothing to commit)", code=0)
    add = _run(["add", "--", *paths], cwd=cwd or client_repo_path())
    if not add.ok:
        return add
    return _run(
        ["commit", "-m", message, "--", *paths],
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
    only the Claude subprocess `cwd` changes.
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


# ---- merge & push (review → done) --------------------------------------------

def pull_ff_only(cwd: Path | None = None) -> GitResult:
    return _run(["pull", "--ff-only", "--no-rebase"], cwd=cwd or client_repo_path())


def merge_branch(branch: str, *, cwd: Path | None = None,
                 message: str | None = None) -> GitResult:
    args = ["merge", "--no-edit"]
    if message:
        args.extend(["-m", message])
    args.append(branch)
    return _run(args, cwd=cwd or client_repo_path())


def push_current(cwd: Path | None = None) -> GitResult:
    return _run(["push"], cwd=cwd or client_repo_path(), timeout=300.0)


def is_already_merged(branch: str, cwd: Path | None = None) -> bool:
    """Check if `branch` is reachable from current HEAD (i.e., already merged)."""
    r = _run(
        ["merge-base", "--is-ancestor", branch, "HEAD"],
        cwd=cwd or client_repo_path(),
    )
    return r.ok  # exit 0 = is ancestor, 1 = not

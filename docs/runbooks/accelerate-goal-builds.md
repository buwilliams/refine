# Accelerate Goal Builds With A Shared Build Cache

Use this runbook when Goal Rounds on a node spend most of their implementation
and Quality time building the target app. Every Round works in an isolated
worktree, so without a shared cache each fresh Round pays a cold build. The
fix is host toolchain configuration, not Refine configuration: Refine invokes
agents with the host's shell environment, so whatever build caching the
target app's toolchain supports simply starts applying to every Round.

Recovery Rounds drafted from Quality or Governance findings already reuse the
source Round's worktree and its warm build state automatically. This runbook
adds cache sharing for fresh Rounds and across Goals.

## Pick the cache for the target app's toolchain

- **Interpreted stacks (JavaScript/TypeScript, Python, Ruby)** usually need
  nothing: npm, pip, and bundler keep host-global download caches, so a fresh
  worktree's dependency install is already warm. Check that first before
  adding anything.
- **Compiled stacks** need a compiler- or build-level cache configured on the
  host: sccache for Rust, ccache for C/C++, the Gradle build cache for JVM
  projects, and equivalents elsewhere. Each worktree keeps its own build
  output directory — build-tool locking is untouched — while compiled
  artifacts are shared through the cache.

Ask the user before installing new system packages on the node.

## Apply (Rust example — Refine's own self-hosted development)

1. Install the cache once per node:

   ```text
   cargo install sccache --locked
   ```

2. Export it from the shell startup files of the user that runs the node
   (for example `~/.bashrc`):

   ```text
   export RUSTC_WRAPPER=sccache
   export SCCACHE_CACHE_SIZE=20G
   ```

3. Restart the Refine daemon. Refine reads the login-shell environment once
   per daemon lifetime, so the restart is what picks the exports up — for both
   the agents Refine spawns and the Quality gate's supervised proof commands,
   which inherit the daemon's own environment.

## Verify

Let one Goal Round complete, then inspect the cache tool's statistics (for
sccache, `sccache --show-stats`). Cache hits should climb on the second and
subsequent Rounds that touch the same dependencies. The Quality gate re-runs
tests in the worktree the implementation agent already built, so most of its
build work short-circuits as well.

## Undo

Remove the exports from the shell startup files and restart the daemon. The
next agent launch reverts to plain toolchain invocations; the cache tool's
on-disk cache can be deleted at any time.

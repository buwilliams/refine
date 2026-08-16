# Accelerate Goal Builds With A Shared Compile Cache

Use this runbook when Goal Rounds on a node spend most of their implementation
and Quality time compiling. Every Round works in an isolated worktree, so
without a cache each fresh Round pays a cold build of the target app. Refine
captures the node user's login-shell environment and hands it to every agent
it spawns — ordinary shell configuration is all this needs.

Recovery Rounds drafted from Quality or Governance findings already reuse the
source Round's worktree and its warm build state automatically. This runbook
adds cache sharing for fresh Rounds and across Goals.

## Preconditions

- The target app has a compiler-level cache tool available. For Rust that is
  [sccache](https://github.com/mozilla/sccache); other toolchains have
  equivalents (ccache, Gradle build cache, Turborepo remote cache).
- Ask the user before installing new system packages on the node.

## Apply (Rust example)

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

   Each worktree keeps its own `target/` directory — cargo's own locking is
   untouched — while compiled artifacts are shared through the cache.

## Verify

Let one Goal Round complete, then run `sccache --show-stats`. Cache hits should
climb on the second and subsequent Rounds that touch the same dependencies. The
Quality gate re-runs tests in the worktree the implementation agent already
built, so most of its compile work short-circuits as well.

## Undo

Remove the two exports from the shell startup files and restart the daemon.
The next agent launch reverts to plain compiler invocations; caches under
`~/.cache/sccache` can be deleted at any time.

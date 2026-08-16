# Accelerate Goal Builds With A Shared Compile Cache

Use this runbook when Goal Rounds on a node spend most of their implementation
and Quality time compiling. Every Round works in an isolated worktree, so
without a cache each Round pays a cold build of the target app. Refine already
injects the variables you declare in `~/.config/refine/agent.env` into every
agent it spawns — no Refine configuration changes are needed.

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

2. Declare it for every Refine-spawned agent by appending to
   `~/.config/refine/agent.env` (create the file if absent):

   ```text
   RUSTC_WRAPPER=sccache
   SCCACHE_CACHE_SIZE=20G
   ```

   `agent.env` is read on every agent launch, so no daemon restart is needed.
   Each worktree keeps its own `target/` directory — cargo's own locking is
   untouched — while compiled artifacts are shared through the cache.

3. Optional, for full coverage: `agent.env` reaches agent CLIs and every
   command those agents run, which is where most compile time lives. The
   Quality gate's supervised proof commands run outside the agent environment
   and inherit the daemon's own environment instead, so to cache those too,
   export the same variables in the environment that starts the Refine daemon
   (its systemd unit or launching shell profile) and restart the daemon.

## Verify

Let one Goal Round complete, then run `sccache --show-stats`. Cache hits should
climb on the second and subsequent Rounds that touch the same dependencies.
With the daemon-environment step applied, the Quality gate's supervised test
commands hit the same cache; the gate re-runs tests in the worktree the
implementation agent already built, so most of its compile work short-circuits
either way.

## Undo

Remove the two lines from `~/.config/refine/agent.env`. The next agent launch
reverts to plain compiler invocations; caches under `~/.cache/sccache` can be
deleted at any time.

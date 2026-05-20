# refine — Spec

## Purpose

A web application for managing background Claude Code agents that autonomously modify an existing application.

Deployed once on a host by a consulting firm; that one refine checkout can know about and switch between multiple client codebases. The client's QA / Product team uses refine to describe Gaps between current and desired behavior; refine drives Claude Code agents to close those Gaps so the consulting firm is not the bottleneck for issues that domain experts can describe and that admit manual verification.

Refine is always available: when a Gap enters `todo`, refine launches an agent CLI subprocess for it immediately (subject to a configurable parallel-run cap and priority ordering).

## Architecture

- **Runtime split:** The UI backend runs natively on the host, either as a detached background process from `refine start` or as a persistent systemd --user service from `refine install`, and owns the runner in-process. Agent CLI subprocesses, git operations, web requests, and flat-file I/O all use host paths and host credentials. See **Runtime topology** below.
- **Backend:** Python (stdlib-first; minimal external deps).
- **Frontend:** Static HTML and vanilla JavaScript. No JS framework or build step.
- **Storage:** Canonical project state lives in JSON under the client repo's `.refine/` directory. SQLite is a disposable per-application cache rebuilt from JSON on startup and app switch.
- **System:** Settings are split between project-wide JSON and active-instance JSON, editable from the UI and scoped by the active target app/instance.
- **External dependencies:** None at runtime beyond the Claude Code CLI and the local git binary, both installed on the host. Stand-alone — no third-party SaaS.
- **Auth:** None for refine itself. Refine assumes deployment on a trusted private network.

## Runtime topology

Refine runs as one host-native UI backend on the same machine as the target app. The backend owns a runner object in-process.

**Host UI service (`refine_ui`):**

- Python web server (UI + JSON API + SSE).
- **Reads** the SQLite cache and canonical JSON files directly from the active app's `.refine/` directory. **Writes** settings, reporters, instance state, and Gap workflow fields through JSON-backed helpers that refresh SQLite projections. User-action activity-feed entries are runtime history in SQLite only.
- Calls the runner directly for everything that touches `gap.json` (round submissions, round edits, log appends) and for agent subprocess lifecycle (launch, cancel, status query).

**On the host (`refine_server`):**

- Spawns Claude Code CLI subprocesses (`claude --print ...`) directly on the host — they inherit the host's `~/.claude/` auth, PATH, and shell.
- Runs every `git` operation against the client repo — fetch, branch, worktree, merge, push — using the host's SSH keys and git config.
- **Owns all `gap.json` writes.** Serializes them per-Gap with an atomic temp-file-plus-rename. Applies round submissions, round edits, and log appends requested by HTTP handlers, plus its own log appends from agent subprocesses and git events.
- Reports state changes by updating canonical `gap.json` where durable Gap state changes and by writing runtime history to SQLite for SSE and dashboard observability.

**Shared filesystem state:**

- The client repository lives on the host. Inside it sits refine's volume root (`.refine/`). The UI backend and in-process runner both access it natively. JSON files are authoritative for project config, instances, instance settings/reporters, and Gap workflow state; SQLite is a shared cache plus runtime-history store. WAL mode plus short transactions keep runtime-history writers from contending.

**Why this split:** The runner owns subprocess and file-write decisions while the UI backend owns HTTP/UI concerns. Running everything natively in one backend process avoids container path mapping, interprocess transport overhead, duplicated environment setup, and credential forwarding; refine reuses whatever auth and tooling the operator already has set up.

## Core entity — Gap

`Gap` is the only first-class entity in refine.

### Fields

| Field      | Type     | Notes                                                            |
|------------|----------|------------------------------------------------------------------|
| `id`       | string   | ULID. Used for hash sharding and as primary key in SQLite.        |
| `name`     | string   | LLM-generated from the first round and any original import text.  |
| `status`   | enum     | Happy path: `todo` → `in-progress` → `ready-merge` → `review` → `done`. The merger lands the merge but always parks the Gap in `review` for human approval; only an explicit user Verify click moves it to `done`. Failure: `in-progress` → `failed` → `todo`. Any non-terminal status → `cancelled`. |
| `rounds`    | array    | Human-submitted; runner appends `logs[]` entries as the agent runs. |
| `created`  | datetime | UTC.                                                              |
| `updated`  | datetime | UTC.                                                              |

### `rounds[]` shape

```json
{
  "reporter": "Jane Doe",
  "actual":   "Current behavior, written by QA/Product",
  "target":   "Desired behavior, written by QA/Product",
  "created":  "2026-05-13T00:00:00Z",
  "updated":  "2026-05-13T00:00:00Z",
  "logs": [
    {
      "datetime": "2026-05-13T00:00:01Z",
      "severity": "info",
      "category": "cli",
      "message":  "agent started"
    }
  ]
}
```

- **Authoring (`actual` / `target`):** Written by humans (QA / Product). Refine does not modify them.
- **`reporter`:** Name of the person submitting this round, chosen at submission time from a dropdown of known reporters (with an inline option to add a new name). Stored as a string on the round; renaming a reporter later does not rewrite historical rounds. See **Reporters**.
- **Lifecycle:** Each round bundles one human submission with the agent run it triggers. The **first round** *is* the Gap (the original ask). **Subsequent rounds** are follow-ups submitted by the human after reviewing the prior round and deciding more work is needed.
- **Execution:** Only the **latest** round drives the next agent invocation. **Each round is independent** — the agent runs fresh with no memory of prior rounds. Continuity carries through the **state of the Gap's branch**: each round's agent sees the commits earlier rounds produced. Round descriptions must therefore be self-contained.
- **`logs[]`:** Append-only entries written by refine during the agent's run for that round. Each entry has shape `{datetime, severity, category, message, details?, actions?}` — same structured shape as the global activity feed. See **Feedback & recovery** for the full schema and the catalog of friendly summaries. Sources include model output, tool-call summaries, git events, and errors.

### Statuses

| Status         | Meaning                                                                                                                                                                 |
|----------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `todo`         | Gap has a latest round that needs an agent run (initial submission, retry after failure, or follow-up after verification). Awaiting an available subprocess slot.        |
| `in-progress`  | A CLI subprocess is running for the Gap. Its branch + worktree exist.                                                                                                     |
| `ready-merge`  | Agent run finished successfully. Gap is queued for the Merger — a single-threaded worker that owns the host worktree and processes merges one at a time. System-owned: the user never sets or clears this status. |
| `review`       | The change is on the target branch and awaits human approval. Reached two ways: (a) auto-merge succeeded and the Merger parked the Gap here for human verification, or (b) auto-merge couldn't complete (real conflict, push race, dirty working copy) and the Merger surfaced it for human resolution. In both cases the human **manually tests** the change, then either Verifies (→ `done`) or submits a new round (→ `todo`). |
| `done`         | Human has Verified. Refine has merged the Gap's branch into the client repo's current branch and pushed upstream. Terminal.                                                                                                  |
| `failed`       | The agent run did not produce a successful result — errored, hit the idle timeout, exceeded the hard cap, exited with no commits, or was reconciled by a runner restart. Worktree and branch retained for a Retry. |
| `cancelled`    | User cancelled the Gap. Worktree and branch cleaned up. Terminal.                                                                                                         |

Priorities gate executable work. `high` blocks `medium` and `low`; `medium` blocks `low`. A higher-priority Gap blocks lower-priority agent work only while it is in `todo`, `in-progress`, or `ready-merge`; `backlog`, `review`, `done`, `failed`, and `cancelled` do not block. If higher-priority blocking work appears while a lower-priority agent is running, refine kills the lower-priority CLI process, discards its partial worktree/branch, and moves that Gap back to `todo` so it can rerun after higher-priority dependencies land.

### Governance

Governance is an optional pre-dispatch review layer. It is configured from
System → Governance with three inputs:

- **Product:** who the product is for, what problems it solves, and what
  success looks like.
- **Constitution:** non-negotiable project principles.
- **Rules:** one-line rules the Governance agent applies to submitted Gaps.

Governance is enabled only when Product and Constitution are both non-empty.
Until then, Gap execution behaves as before. When enabled, a single
Governance agent reviews `todo` Gaps before implementation agents can launch.
The latest round records:

- `rule_state`: `unclassified`, `passed`, `failed`, `blocked`,
  `needs_review`, `needs_context`, or `exception_requested`.
- `meta_rule_state`: `unclassified`, `none`, `candidate_rule`,
  `rule_review_needed`, `ambiguous_rule`, `stale_rule`, or
  `conflicting_rules`.
- `product_state` / `constitution_state`: `unclassified`, `pass`, or `fail`.
- Governance message/details, check timestamp, and any rule actions.

Only rounds with `rule_state=passed`, `product_state=pass`, and
`constitution_state=pass` may proceed to implementation. Failed or unclear
governance reviews move the Gap back to `backlog` with a governance message;
the user edits the latest round and resubmits it to `todo`. Governance may
auto-apply rule add/edit/remove actions when Product and Constitution both
pass. Rules can also be generated from Product + Constitution in System and
reviewed before saving.

Transitions:
- `todo → in-progress` — automatic when refine launches an agent subprocess for the Gap.
- `in-progress → ready-merge` — automatic on successful agent completion. The Gap joins the merger's queue.
- `in-progress → failed` — automatic on agent error, idle timeout, hard cap, exit-0-with-no-commits, or runner-restart reconciliation.
- `ready-merge → review` — automatic. Reached two ways with the same destination: (a) the Merger successfully completed verify (fetch → pull → merge → push, including any auto-resolve), or (b) verify couldn't complete cleanly (real conflict the auto-resolver couldn't fix, push race, dirty working copy). Either way the Gap sits in `review` awaiting human approval — the Merger never moves Gaps to `done` directly.
- `failed → todo` — explicit user action ("Retry"). The worktree and branch are reused. **Blocked** if a prior `category: auth` failure remains unresolved — see Failure handling → Retry pre-flight.
- `review → done` — explicit user action ("Verify"). Routes through the Merger. Bookkeeping-only when the auto-merge already landed (the Gap's branch has been cleaned up); runs the full merge pipeline if the branch is still around (auto-merge had failed and the operator resolved manually).
- `review → todo` — explicit user action; triggered by appending a new round (human submitting follow-up work). The branch is retained.
- any non-terminal status → `cancelled` — explicit user action ("Cancel"). For `in-progress`, refine kills the CLI subprocess first. Worktree and branch are cleaned up.

Back-and-forth between human and agent loops through `review → todo → in-progress → ready-merge → review` (or `done`) until the merge completes cleanly.

## Reporters

Every round records who submitted it. Refine has no authentication; reporters are **identification only** — any user can select any other person as the reporter. The mechanism exists so the team can attribute Gaps and rounds, not to enforce identity.

**Data model:**

- Each round stores a `reporter` string — the name chosen at submission time. Renaming or removing a reporter does not rewrite historical rounds.
- The set of known reporter names lives in SQLite (a `reporters` table). This list backs the dropdown on the round-submission UI; it is not itself the source of truth for who-submitted-what (the rounds are).

**UI behavior:**

- Round-submission forms include a required **Reporter** dropdown of known names.
- The default selection is the **last-used reporter** stored per-browser in `localStorage`. First-time users have no default and must pick before submitting.
- An **Add new reporter** option in the dropdown lets the user type a new name; it is inserted into the `reporters` table and selected immediately.
- No authorization checks — any user may select any reporter, including someone else.
- **Scope:** Reporter attribution is per-round only. Other state transitions (Verify, Retry, Cancel) are unattributed by design.

**Management:**

- System includes a Reporters page for renaming or removing names from the dropdown. Historical rounds retain their original reporter string.

## Storage layout

Two-character ID-prefix sharded directory layout under the volume root on the host filesystem — same directory shape as git's `.git/objects/`, but used purely for filesystem performance with many Gaps (not for content-addressability). The shard width is fixed at two characters. The SQLite index handles all lookup and search:

```
<volume-root>/
  config.json                  # schema version, project-wide settings, governance
  guidance.json                # project-wide guidance rules and enabled state
  instances.json               # canonical instance registry
  index.sqlite                 # disposable cache + runtime history
  gaps/
    ab/                        # first 2 chars of Gap ULID
      cdef…/                   # remaining ULID
        gap.json               # id, name, status, priority, branch, instance, rounds/logs
  instances/
    <instance_id>/
      application.json
      reporters.json
      runtime.json
      target-app.json
```

- **One JSON file per Gap.** All round content and logs live in `gap.json`.
- **`gap.json` holds** identity, workflow ownership, and content: `id`, `name`, `status`, `priority`, `branch_name`, `instance_id`, `created`, `updated`, and the `rounds` array (each round with `reporter`, `actual`, `target`, timestamps, and `logs[]`).
- **Instance JSON holds** active-instance scoped application/runtime/target-app settings and reporter dropdown entries. `instances.json` is the source of truth for instance IDs and display names.
- **Project JSON holds** schema version and project-wide Governance settings. `guidance.json` holds project-wide Guidance entries with `name`, `rule`, `instructions`, and `enabled`.
- **SQLite holds rebuildable projections** for Gap lists, filters, counts, settings, and reporter dropdown reads. It also stores disposable runtime history and observability tables: `activity`, `runs`, `preflight`, and `target_app_operations`. Deleting `index.sqlite` loses that runtime history but not canonical project, instance, reporter, settings, or Gap state.
- The volume root lives inside the client repo and is committed there.

## Agent execution

### Mechanism

- The **host runner** (see **Runtime topology**) shells out to the **Claude Code CLI** (`claude --print` / headless mode) directly on the host. The UI backend calls the runner directly; the runner spawns and supervises the subprocess.
- **Pre-flight check.** At runner startup, and before spawning a subprocess after a prior auth failure, the runner issues a fast `claude --version` (or equivalent no-op) to confirm CLI presence and valid auth. Failure surfaces as the **Claude auth pre-flight failed** banner; humans can re-run it on demand from System.
- **One fresh CLI invocation per unaddressed round.** No session is resumed; the agent has no memory of prior rounds. The latest round's `actual` / `target` becomes the prompt.
- **Cross-round continuity carries through the Gap's branch**, not through a Claude session. Each round's agent starts with the worktree exactly as the prior round's commits left it.
- Consequence: round descriptions must be self-contained. A follow-up round cannot say "do what we discussed before" — humans must restate the relevant context in the round body.

### Concurrency

- **Parallel-run cap** — at most N agent CLI subprocesses may run at once across all Gaps (default `3`, configurable from the System page). There is no persistent worker pool; subprocesses are spawned on demand when there is work to do, and the process exits when its round finishes.
- **Per-Gap lock** — a Gap may have at most one running subprocess at a time.
- When a Gap enters `todo`, refine launches a subprocess for it immediately if a slot is available under the cap. Otherwise the Gap waits in `todo` and is picked up the moment a running subprocess finishes.

### Git integration

- The client repository lives on the host filesystem; the runner accesses it natively (see **Runtime topology**).
- **Operational assumption:** the host running refine is dedicated to refine — no human edits the working copy directly, and all local commits on the client's current branch come from refine's agent runs. The client's developers push from their own machines; refine sees their work via `fetch`.
- Each Gap gets its **own branch in its own git worktree** so concurrent agent subprocesses do not contend on `HEAD`.
- Branch naming: `refine/<gap-id>` (configurable).
- The Gap's branch is retained across all back-and-forth rounds; each agent run appends commits to it.
- Refine fetches and pulls at two points:

**On Gap pickup (`todo → in-progress`):**

0. **Pre-check** — the client repo must be on a named branch with an upstream (`git rev-parse --abbrev-ref HEAD@{upstream}` resolves). If not, abort pickup; surface a banner (*"Branch `<n>` has no upstream — run `git push -u origin <n>` on the host"*); leave the Gap in `todo` to be retried when the operator fixes it.
1. `git fetch` from the remote.
2. Record the client repo's current branch name.
3. Create the Gap's worktree and branch off `origin/<current-branch>` so the agent starts from the freshest remote tip (not stale local state).

**On `review → done` (merge + push):**

1. `git fetch` from the remote.
2. Fast-forward the client repo's current branch to `origin/<current-branch>` (`git pull --ff-only`) — picks up any commits other developers landed while the agent worked.
3. Merge the Gap's branch into the current branch — fast-forward when possible, standard merge commit otherwise.
4. Push the current branch upstream.

Conflict & failure paths:

- **Pull cannot fast-forward** (local diverged from remote): abort, leave the Gap in `review`, surface in the activity feed for human resolution.
- **Merge conflict**: leave the Gap in `review`, mark the latest round's `logs[]` with the conflicting paths, leave the worktree intact for humans to inspect or resolve.
- **Push fails** (e.g. race with another developer's push, or a remote/credential issue): refine retries once — re-fetch, re-merge if needed, re-push. Persistent failure: the Gap **stays in `review`** (does **not** transition to `done`) — push failure is an environment issue, not a Gap-completion event. The failure is surfaced in the UI for human resolution. Recommended recovery: attach Chat to the Gap to investigate / fix the underlying issue, then Verify again.
- **On successful push**: the Gap's branch and worktree are cleaned up.

### Failure handling

- A failed CLI invocation moves the Gap to `failed` and appends an error entry to the latest round's `logs[]`.
- The UI surfaces failed Gaps with a **Retry** action, which moves the Gap back to `todo`; refine will spawn a new subprocess as soon as a slot is available. No automatic retry.
- The worktree and branch persist across retries; the next invocation runs fresh against whatever state the prior run left in the worktree.
- **Stuck-detection.** Two independent signals can move a running subprocess to `failed`. They are deliberately separate because elapsed time alone is a poor stuck-signal — large Gaps can legitimately run for many hours. Both signals produce friendly summaries distinct from "agent errored" (see **Feedback & recovery**).
  - **Idle timeout** (primary) — if the CLI subprocess emits no stdout/stderr for the configured idle window (default `15 min`), refine treats it as stuck and kills it. Activity is the real "alive" signal; Claude Code prints continuously when working.
  - **Hard wall-clock cap** (ultimate stop-gap) — if a single invocation has been running longer than the configured cap (default `24 h`), refine kills it. Intended to fire only on pathological runs that produce output but never finish. Can be disabled by setting to `0`.
- **Retry pre-flight**: when retrying a Gap whose previous failure was `category: auth`, refine re-runs the runner's Claude auth pre-flight first. If it still fails, the Retry is blocked with the auth banner restated — saves burning an agent run on a known-broken environment.
- **Runner restart**: on startup, the host runner reconciles state — any Gap in `in-progress` without a live subprocess is moved to `failed` with a "runner restarted" log entry. Its worktree and branch are preserved so the human can Retry.
- **Mid-merge / mid-push crash**: a Gap in `review` whose Verify started but didn't finish (runner crashed mid-merge or mid-push) remains in `review` with a log entry recording how far it got. Clicking Verify again retries the entire fetch → pull → merge → push sequence; refine skips steps already completed locally (a fresh merge against an already-merged branch is a no-op fast-forward), so retries are safe to repeat.

## Feedback & recovery

Every failure must tell the user what happened in plain language and offer a recovery path. Six principles drive the design:

1. **Status carries the story** — a Gap's status plus its latest log entry should answer "what happened?" without hunting through the activity feed.
2. **Every error offers at least one action** — never just "Error." Always summary + ≥1 recovery action (Retry, Open Chat, Edit round, View details).
3. **Two reading levels** — friendly summary on top; **Show details** reveals raw stderr / paths / exit code.
4. **Self-heal silently** — stale `localStorage` reporters, orphan worktrees, runner-restart fail-marking — fixed without notifying.
5. **Chat is the universal escape hatch** — for anything refine cannot auto-resolve (merge conflict, dirty working copy, push race, hook failure, auth issue), the recovery affordance is always **Open Chat** attached to the Gap.
6. **Banners for systemic problems** — auth missing, backend runner unavailable, disk near full — visible on every page until resolved.

### Activity entry shape

Both the global activity feed and each round's `logs[]` use the same structured entry shape:

```json
{
  "datetime": "2026-05-13T00:00:01Z",
  "severity": "info | warn | error",
  "category": "auth | git | cli | io | state | user",
  "gap_id":   "ULID or null",
  "actor":    "<reporter name> | refine | runner",
  "message":  "human-readable summary",
  "details":  "expandable raw stderr / paths / exit code",
  "actions":  [{ "label": "Retry", "endpoint": "/gaps/<id>/retry" }]
}
```

`actor` semantics:

- **Round submission** — the chosen reporter name.
- **Other user-triggered transitions** (Verify, Retry, Cancel) — `refine`. Unattributed by design; see **Reporters → Scope**.
- **Automatic system events** — `refine` for state transitions and auto-recovery; `runner` for subprocess output, git events, and pre-flight checks.

Within a round's `logs[]`, `gap_id` is implicit and may be omitted. `details` and `actions` are optional.

**Where events appear.** Per-round events (agent output, git activity, run failures) appear in **both** the round's `logs[]` (carrying `details`) and the activity feed (summary plus a back-link to the Gap). Cross-Gap or systemic events (banner conditions, settings changes, reporter management) appear only in the activity feed.

### Friendly summaries (stderr → summary catalog)

The runner pattern-matches CLI / git stderr against canonical error classes and prepends a one-line, actionable summary. The user sees the summary; **Show details** reveals raw stderr. The `category` drives which recovery action set the UI shows.

| Pattern matched | Category | Summary message |
|-----------------|----------|-----------------|
| Claude auth error | `auth` | `Claude auth issue — run \`claude login\` on the host` |
| `git push` non-fast-forward | `git` | `Push rejected — another developer pushed first` |
| `git push` auth failure | `git` | `Push auth failed — check SSH agent / git credentials on the host` |
| `git pull` non-fast-forward | `git` | `Local branch diverged from remote — manual reconciliation needed` |
| Pre-commit hook exit ≠ 0 | `git` | `Pre-commit hook \`<name>\` failed` |
| Pre-push hook exit ≠ 0 | `git` | `Pre-push hook \`<name>\` blocked the push` |
| No subprocess output for idle window | `cli` | `Agent appears stuck — no output for Xm` |
| Hard wall-clock cap exceeded | `cli` | `Agent exceeded the X-hour run cap` |
| Exit 0, no new commits | `cli` | `Agent exited without producing changes — try refining the round` |
| Anthropic rate-limit response | `cli` | `Claude rate-limited — try again shortly` |

This is the canonical catalog. Adding a new error class is one new pattern entry plus one new row in the recovery table below.

### Failure-state UI contract

When a Gap is in `failed`, or in `review` with a known recovery condition (dirty working copy, push race, hook block), the Gap detail page **must** render:

- **Top banner** with the category-specific summary message.
- **Latest log entry** inline.
- **Action row** in priority order. Typical sets:
  - `failed`: **Retry · Edit latest round · Open Chat · Cancel · Show details**
  - `review` w/ push race: **Verify · Open Chat · Cancel · Show details**
  - `review` w/ dirty working copy: **Open Chat · Verify (after fix) · Cancel · Show details**
  - `review` w/ merge conflict: **Open Chat · Verify (after resolution) · Cancel · Show details**
  - `review` w/ pull cannot fast-forward: **Open Chat · Verify · Cancel · Show details**
- **Full logs** expandable below the banner.

No "Error: failed" dead-ends.

### Global banners

Sticky banners visible on every page until the underlying condition clears:

- **Backend runner unavailable** — *"Backend runner unavailable. Restart refine-ui and check logs."*
- **Claude auth pre-flight failed** — *"Refine cannot reach Claude — run `claude login` on the host."* Action: **Re-check auth**.
- **Volume root not writable** — *"Volume root not writable. Check filesystem permissions for `<path>`."*
- **Disk usage critical** — *"Volume root partition is N% full."* Warn at 85%, error at 95%.

### Per-failure recovery reference

| Failure | Surface | Plain-language message | Recovery actions |
|---------|---------|------------------------|------------------|
| Agent idle (no output) | Gap detail (`failed`) | "Agent appears stuck — no output for Xm" | Retry / Edit round / Open Chat |
| Hard cap exceeded | Gap detail (`failed`) | "Agent exceeded the X-hour run cap" | Retry / Edit round / Open Chat |
| Exit 0, no commits | Gap detail (`failed`) | "Agent finished without producing changes" | Edit round / Retry / Open Chat |
| Claude auth missing | Banner | "Refine cannot reach Claude — run `claude login`" | Re-check auth |
| Pre-commit hook fail | Gap detail (`failed`) | "Pre-commit hook `<n>` failed" | Open Chat / Edit round / Retry |
| Pre-push hook fail | Gap detail (`review`) | "Pre-push hook `<n>` blocked the push" | Open Chat / Verify after fix |
| Push race | Gap detail (`review`) | "Another developer pushed; merge needs to be redone" | Verify |
| Dirty working copy | Inline on Verify | "Working copy has uncommitted changes on `<branch>`" | Open Chat / Verify once clean |
| No upstream / detached HEAD | Banner + Gap (`todo`) | "Branch `<n>` has no upstream — run `git push -u origin <n>`" | (operator fix; auto-resumes) |
| Backend runner unavailable | Banner | "Backend runner unavailable" | Restart refine-ui / check logs |
| Permission mismatch | Webapp or runner won't start | "Volume root not writable" | Fix host filesystem ownership/permissions |
| Race / stale UI | Inline toast | "This Gap changed since you loaded it — refreshing" | (auto-refresh; user retries) |
| Import extraction failed | Import dialog | "Could not extract Gaps — review raw output and create manually" | Show raw output / Manual entry |
| Stale `localStorage` reporter | Reporter dropdown | (silent) | (clear; force fresh pick) |

## Features

### Dashboard

Single landing view summarizing:
- **Needs attention** — aggregates anything requiring human action: systemic banners (auth, runner, disk) and counts of failed Gaps / Gaps stuck in `review` with known recovery conditions. Each count is a clickable filter.
- Counts per status.
- Currently running agent subprocesses (which Gap each is on, elapsed time).
- Recent activity feed — structured entries (see **Feedback & recovery** for the entry shape). Includes round submissions with reporter, state transitions, completions, and failures.

### Gap Manager

- **CRUD** — Gaps are fully CRUD. Rounds are **append-only**: only the **latest** round can be edited, and only while it is still unaddressed (Gap status `todo` or `failed`); all prior rounds are sealed, read-only.
- **Cancel** — available on the Gap detail page for any non-terminal status (`todo`, `in-progress`, `review`, `failed`); moves the Gap to `cancelled` and cleans up worktree+branch. Agent Manager's Cancel is a shortcut for the `in-progress` case.
- **Delete by status** — deleting an `in-progress` Gap cancels it first (kills the subprocess, releases the lock) and then removes the record. Deleting from `review` or `failed` cleans up the Gap's worktree and branch. Deleting from `todo` or `cancelled` removes just the record (no worktree to clean up — `todo` may not have one yet, `cancelled` was already cleaned up). Deleting from `done` removes only the Gap record; the merged commits stay in the client repo.
- **Search** — over name, round `actual` / `target` / `reporter`, status, dates.
- **Import** — paste free-form text (meeting transcript, bug report, feedback dump); an LLM call extracts a list of discrete Gaps, each with a seeded initial round. The user picks a reporter (defaulting to the last-used name) to attribute the imported rounds to, then reviews and confirms before persisting.

### Agent Manager

- View currently running agent subprocesses (which Gap each is on, elapsed time).
- Configure the parallel-run cap at runtime.
- Pause / resume agent spawning. While paused, `todo` Gaps wait; running subprocesses are not killed. The pause flag is stored in SQLite, so it survives runner restarts.
- Cancel an in-flight Gap — kills the CLI subprocess, releases the lock, and moves the Gap to `cancelled` (worktree and branch are cleaned up).

### Chat

Two modes, both backed by Claude Code CLI:

1. **Standalone, codebase-scoped.** Ad-hoc chat against the client repo. No Gap context. Useful for exploration or manual fixes outside the Gap workflow.
2. **Attached to a Gap.** Opens a fresh CLI chat session in the Gap's worktree, so the chat sees that Gap's branch. Useful for exploring on top of the agent's work, running it locally, making manual tweaks, or resolving environment issues like a stuck merge or push. Available when the Gap's worktree exists — typically status `review` or `failed`, or briefly in `todo` between rounds — and no agent subprocess is currently running for the Gap. An **Open Chat** affordance appears among the recovery actions on every failed Gap and every Gap stuck in `review` with a known recovery condition (see **Feedback & recovery**).

Chat sessions are human-driven, not rounds: they do **not** count toward the parallel-run cap.

Both modes share the same chat UI; entry points differ (top nav vs Gap detail page).

### System

- **Runtime configuration** — parallel-run cap, branch name pattern, merge target. See the catalog under **Application settings** below.
- **Reporters** — rename or remove names from the dropdown of known reporters. See the **Reporters** section.
- **Re-check auth** — on-demand re-run of the host runner's Claude auth pre-flight; clears the auth banner if it now passes.
- **Backend diagnostics** — in-process runner mode, last call timestamp, recent backend errors.
- Changes take effect immediately.

## UI

- Static HTML + vanilla JavaScript served by the Python backend.
- No JS framework, no build step, no transpilation.
- **Live updates via Server-Sent Events (SSE).** Single EventSource stream per page delivers status changes, round events (new round, run start/finish), round-log appends (entries written into the open round's `logs[]`), and activity-feed entries.

## Application settings

Application settings live in `.refine/config.json` for project-wide policy and `.refine/instances/<instance_id>/` for instance-scoped runtime/application/reporters/target-app settings. SQLite mirrors the active instance's effective settings for fast reads and legacy code paths.

**Runtime configuration:**

| Setting               | Default                                    | Notes |
|-----------------------|--------------------------------------------|-------|
| Parallel-run cap      | `3`                                        | Max agent subprocesses running concurrently. |
| Branch name pattern   | `refine/<gap-id>`                          | `<gap-id>` is substituted at branch creation. |
| Merge target          | client repo's current branch at merge time | **Fixed policy** — not configurable. Refine always merges into whatever branch is checked out at merge time. |
| Agent idle timeout    | `15 min`                                   | Kill the subprocess if it produces no output for this long. Primary stuck-detector. Set to `0` to disable. |
| Agent hard cap        | `24 h`                                     | Maximum wall-clock runtime per agent invocation. Ultimate stop-gap for runaway runs. Set to `0` to disable. |

**Reporters list:** the set of known reporter names that backs the round-submission dropdown. Managed from the System → Reporters page. See **Reporters** for the full semantics.

**Deploy-time configuration** (set by `refine init`, `refine start <port>`, and `refine install <port>`, not in SQLite):

- Active app binding — checkout-local `.refine-binding` points to the active target app.
- Active instance selection — checkout-local `run/active-instances.json` records each target app's selected instance for this Refine checkout.
- Background UI process — `refine start <port>` launches a detached `uv run refine ui` process and records its PID/log under checkout-local `run/`.
- UI backend unit — `refine install <port>` writes a checkout-local, per-port systemd user unit that starts `uv run refine ui`, restarts on failure, and survives terminal close. `refine uninstall <port>` stops and removes it.

## Out of scope (initial version)

- Authentication / multi-user accounts. (Reporters provide unverified identification; see **Reporters**.)
- Integrations with external issue trackers (GitHub Issues, Linear, Jira).
- Cross-instance sync or export/import between refine deployments.
- Automatic round generation outside the Import workflow. Import seeds the first round of each extracted Gap via an LLM; follow-up rounds are always human-authored.
- Automatic retries on agent failure.

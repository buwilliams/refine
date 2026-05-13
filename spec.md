# refine — Spec

## Purpose

A web application for managing background Claude Code agents that autonomously modify an existing application.

Deployed per-client by a consulting firm. The client's QA / Product team uses refine to describe Gaps between current and desired behavior; refine drives Claude Code agents to close those Gaps so the consulting firm is not the bottleneck for issues that domain experts can describe and that admit manual verification.

Refine is always available: when a Gap enters `todo`, refine launches an agent CLI subprocess for it immediately (subject to a configurable parallel-run cap).

## Architecture

- **Runtime split:** Docker hosts only the webapp. Agent CLI subprocesses, git operations, and flat-file I/O run natively on the host, so they inherit the host's Claude Code auth, SSH credentials, git config, and filesystem permissions. See **Runtime topology** below.
- **Backend:** Python (stdlib-first; minimal external deps).
- **Frontend:** Static HTML and vanilla JavaScript. No JS framework or build step.
- **Storage:** Single SQLite file (index, settings, run state, reporters) plus flat JSON files (one per Gap). All committed to the client application's repository — **including the SQLite file** — so refine's state travels with the codebase.
- **Settings:** All application settings live in SQLite, editable from the UI, scoped per refine instance / client project.
- **External dependencies:** None at runtime beyond the Claude Code CLI and the local git binary, both installed on the host. Stand-alone — no third-party SaaS.
- **Auth:** None for refine itself. Refine assumes deployment on a trusted private network.

## Runtime topology

Refine runs as two co-located components on the same machine: a Dockerized webapp and a host-native runner.

**In Docker (`refine-web`):**

- Python web server (UI + JSON API + SSE).
- Reads and writes the SQLite database and gap JSON files via a bind mount onto the host filesystem.
- Talks to the host runner over a local IPC channel (e.g. Unix domain socket mounted into the container — concrete wire protocol TBD) to launch / cancel agent subprocesses and query their state.

**On the host (`refine-runner`):**

- Spawns Claude Code CLI subprocesses (`claude --print ...`) directly on the host — they inherit the host's `~/.claude/` auth, PATH, and shell.
- Runs every `git` operation against the client repo — fetch, branch, worktree, merge, push — using the host's SSH keys and git config.
- Writes round `logs[]` entries and updates the gap JSON files as work progresses.
- Reports state changes back to the webapp via the IPC channel and by writing to SQLite (which the webapp's SSE stream surfaces).

**Shared filesystem state:**

- The client repository lives on the host. Inside it sits refine's volume root (SQLite + gap JSON files). The webapp container bind-mounts the volume root; the runner accesses it natively. Both processes read and write the same files.

**Why this split:** Running CLI subprocesses inside the webapp container would mean either baking host credentials into the image or volume-mounting `~/.claude`, SSH keys, and git config in — fragile and surprising. Running them natively on the host means refine reuses whatever auth and tooling the operator already has set up.

## Core entity — Gap

`Gap` is the only first-class entity in refine.

### Fields

| Field      | Type     | Notes                                                            |
|------------|----------|------------------------------------------------------------------|
| `id`       | string   | ULID. Used for hash sharding and as primary key in SQLite.        |
| `name`     | string   | LLM-generated from the first round and any original import text.  |
| `status`   | enum     | Happy path: `todo` → `in-progress` → `review` → `done`. Failure: `in-progress` → `failed` → `todo`. Any non-terminal status → `cancelled`. |
| `rounds`    | array    | Human-authored. Agent executes; agent may append `logs` per round. |
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
    { "datetime": "2026-05-13T00:00:01Z", "message": "agent started" }
  ]
}
```

- **Authoring (`actual` / `target`):** Written by humans (QA / Product). Refine does not modify them.
- **`reporter`:** Name of the person submitting this round, chosen at submission time from a dropdown of known reporters (with an inline option to add a new name). Stored as a string on the round; renaming a reporter later does not rewrite historical rounds. See **Reporters**.
- **Lifecycle:** Each round bundles one human submission with the agent run it triggers. The **first round** *is* the Gap (the original ask). **Subsequent rounds** are follow-ups submitted by the human after reviewing the prior round and deciding more work is needed.
- **Execution:** Only the **latest** round drives the next agent invocation. **Each round is independent** — the agent runs fresh with no memory of prior rounds. Continuity carries through the **state of the Gap's branch**: each round's agent sees the commits earlier rounds produced. Round descriptions must therefore be self-contained.
- **`logs[]`:** Append-only entries written by refine during the agent's run for that round. Each entry has shape `{datetime, message}`. Sources include model output, tool-call summaries, git events, and errors.

### Statuses

| Status         | Meaning                                                                                                                                                                 |
|----------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `todo`         | Gap has a latest round that needs an agent run (initial submission, retry after failure, or follow-up after verification). Awaiting an available subprocess slot.        |
| `in-progress`  | A CLI subprocess is running for the Gap. Its branch + worktree exist.                                                                                                     |
| `review`       | Agent has finished the latest round. Changes sit on the Gap's branch. Human **manually tests** the change (typically by checking out the Gap's branch locally and exercising the app) and then either Verifies (→ `done`) or submits a new round (→ `todo`).                                                      |
| `done`         | Human has verified. Refine has merged the Gap's branch into the client repo's current branch and pushed upstream. Terminal.                                              |
| `failed`       | The agent's CLI invocation errored, or the runner restarted while the Gap was in-progress. Worktree and branch retained for a Retry.                                      |
| `cancelled`    | User cancelled the Gap. Worktree and branch cleaned up. Terminal.                                                                                                         |

Transitions:
- `todo → in-progress` — automatic when refine launches an agent subprocess for the Gap.
- `in-progress → review` — automatic on successful agent completion.
- `in-progress → failed` — automatic on agent error.
- `failed → todo` — explicit user action ("Retry"). The worktree and branch are reused.
- `review → done` — explicit user action ("Verify"). Refine performs the merge and push as part of the transition.
- `review → todo` — explicit user action; triggered by appending a new round (human submitting follow-up work). The branch is retained.
- any non-terminal status → `cancelled` — explicit user action ("Cancel"). For `in-progress`, refine kills the CLI subprocess first. Worktree and branch are cleaned up.

Back-and-forth between human and agent loops through `review → todo → in-progress → review` until the human verifies.

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

- Settings includes a Reporters page for renaming or removing names from the dropdown. Historical rounds retain their original reporter string.

## Storage layout

Two-character ID-prefix sharded directory layout under the volume root on the host filesystem — same directory shape as git's `.git/objects/`, but used purely for filesystem performance with many Gaps (not for content-addressability). The shard width is fixed at two characters. The SQLite index handles all lookup and search:

```
<volume-root>/
  index.sqlite                 # ID → path, search index, settings, run state, reporters
  gaps/
    ab/                        # first 2 chars of Gap ULID
      cdef…/                   # remaining ULID
        gap.json               # full Gap record (rounds, logs, metadata)
```

- **One JSON file per Gap.** All round content and logs live in `gap.json`.
- **SQLite indexes** Gaps for list, filter, and search. It is also the source of truth for ephemeral run state (per-Gap locks, currently-running subprocesses), runtime settings, and the reporters list. `gap.json` is the source of truth for Gap content (rounds, logs, each round's reporter).
- The volume root lives inside the client repo and is committed there.

## Agent execution

### Mechanism

- The **host runner** (see **Runtime topology**) shells out to the **Claude Code CLI** (`claude --print` / headless mode) directly on the host. The webapp requests a launch over IPC; the runner spawns and supervises the subprocess.
- **One fresh CLI invocation per unaddressed round.** No session is resumed; the agent has no memory of prior rounds. The latest round's `actual` / `target` becomes the prompt.
- **Cross-round continuity carries through the Gap's branch**, not through a Claude session. Each round's agent starts with the worktree exactly as the prior round's commits left it.
- Consequence: round descriptions must be self-contained. A follow-up round cannot say "do what we discussed before" — humans must restate the relevant context in the round body.

### Concurrency

- **Parallel-run cap** — at most N agent CLI subprocesses may run at once across all Gaps (default `3`, configurable from the Settings page). There is no persistent worker pool; subprocesses are spawned on demand when there is work to do, and the process exits when its round finishes.
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
- **Runner restart**: on startup, the host runner reconciles state — any Gap in `in-progress` without a live subprocess is moved to `failed` with a "runner restarted" log entry. Its worktree and branch are preserved so the human can Retry.

## Features

### Dashboard

Single landing view summarizing:
- Counts per status.
- Currently running agent subprocesses (which Gap each is on, elapsed time).
- Recent activity feed (round submissions with reporter, state transitions, completions, failures).

### Gap Manager

- **CRUD** — Gaps are fully CRUD. Rounds are **append-only**: only the **latest** round can be edited, and only while it is still unaddressed (Gap status `todo` or `failed`); all prior rounds are sealed, read-only.
- **Delete by status** — deleting an `in-progress` Gap cancels it first (kills the subprocess, releases the lock) and then removes the record. Deleting from `review` or `failed` cleans up the Gap's worktree and branch. Deleting from `done` removes only the Gap record — the merged commits stay in the client repo.
- **Search** — over name, round `actual` / `target` / `reporter`, status, dates.
- **Import** — paste free-form text (meeting transcript, bug report, feedback dump); an LLM call extracts a list of discrete Gaps, each with a seeded initial round. The user picks a reporter (defaulting to the last-used name) to attribute the imported rounds to, then reviews and confirms before persisting.

### Agent Manager

- View currently running agent subprocesses (which Gap each is on, elapsed time).
- Configure the parallel-run cap at runtime.
- Pause / resume agent spawning. While paused, `todo` Gaps wait; running subprocesses are not killed.
- Cancel an in-flight Gap — kills the CLI subprocess, releases the lock, and moves the Gap to `cancelled` (worktree and branch are cleaned up).

### Chat

Two modes, both backed by Claude Code CLI:

1. **Standalone, codebase-scoped.** Ad-hoc chat against the client repo. No Gap context. Useful for exploration or manual fixes outside the Gap workflow.
2. **Attached to a Gap.** Opens a fresh CLI chat session in the Gap's worktree, so the chat sees that Gap's branch. Useful for exploring on top of the agent's work, running it locally, making manual tweaks, or resolving environment issues like a stuck merge or push. Available when the Gap's worktree exists — typically status `review` or `failed`, or briefly in `todo` between rounds — and no agent subprocess is currently running for the Gap.

Chat sessions are human-driven, not rounds: they do **not** count toward the parallel-run cap.

Both modes share the same chat UI; entry points differ (top nav vs Gap detail page).

### Settings

- **Runtime configuration** — parallel-run cap, branch name pattern, merge target. See the catalog under **Application settings** below.
- **Reporters** — rename or remove names from the dropdown of known reporters. See the **Reporters** section.
- Changes take effect immediately.

## UI

- Static HTML + vanilla JavaScript served by the Python backend.
- No JS framework, no build step, no transpilation.
- **Live updates via Server-Sent Events (SSE).** Single EventSource stream per page delivers status changes, round events (new round, run start/finish), and log appends.

## Application settings

All application settings live in the SQLite database and are editable from the **Settings** page in the UI, so each client project has its own configuration. Defaults are seeded on first run.

**Runtime configuration:**

| Setting               | Default                                    | Notes |
|-----------------------|--------------------------------------------|-------|
| Parallel-run cap      | `3`                                        | Max agent subprocesses running concurrently. |
| Branch name pattern   | `refine/<gap-id>`                          | `<gap-id>` is substituted at branch creation. |
| Merge target          | client repo's current branch at merge time | |

**Reporters list:** the set of known reporter names that backs the round-submission dropdown. Managed from the Settings → Reporters page. See **Reporters** for the full semantics.

**Deploy-time configuration** (set in docker-compose / runner startup, not in SQLite):

- Volume root — host path mounted into the webapp container; lives inside the client repo.
- Host runner IPC path — where the webapp can reach the host runner.

## Out of scope (initial version)

- Authentication / multi-user accounts. (Reporters provide unverified identification; see **Reporters**.)
- Integrations with external issue trackers (GitHub Issues, Linear, Jira).
- Cross-instance sync or export/import between refine deployments.
- Automatic round generation outside the Import workflow. Import seeds the first round of each extracted Gap via an LLM; follow-up rounds are always human-authored.
- Automatic retries on agent failure.

# refine

refine turns behavior bug reports into merged commits. QA, Product, and
support describe a **Gap** — what the app does today versus what it should
do instead — and refine launches a Claude Code agent in a git worktree
against the codebase. The agent makes the change, commits, and refine
merges the work back to the branch you have checked out. Gaps move
`todo → in-progress → review → done`, with `failed` and `cancelled` for the
unhappy paths; multiple Gaps run in parallel up to a configurable cap.

You drive everything from a web UI:

- A **status dashboard** with a Reporter stats card — click a row to deep-link
  into the Gaps list filtered by that reporter.
- A **Gaps list** with search + status + reporter + severity / category / actor
  / entries-limit filters, sortable columns, and bulk-update actions for
  priority, status, and reporter that respect whatever filter is active.
- A **filterable Logs view** with the same Logs-style filter set.
- A **persistent Chat dock** at the bottom of every page — collapsible,
  vertically resizable, with a fullscreen toggle. Tabs for a standalone chat
  and one per Gap; opening Chat against an in-progress Gap eagerly primes the
  claude session with the Gap's context so the user's first message gets a
  context-aware answer. Transcripts render markdown.
- Import-from-text uses an **LLM call** (via the host `claude` CLI) to extract
  `{name, actual, target}` drafts from free-form paste-dumps; a loading
  indicator shows while the model runs.

Active filters are surfaced visually: matching dropdowns/inputs pick up an
accent border, a "FILTERED" pill appears next to the count, and the results
table grows an accent stripe + tinted header.

Refine handles the git plumbing — worktrees, fetch, merge, push,
auto-committing its own state — and inherits Claude Code auth from the host,
so operators rarely need to think about either.

## Components

- **`refine-web`** — Python webapp in Docker (UI + JSON API + SSE).
- **`refine-runner`** — host-native daemon that owns CLI subprocesses, git
  operations, and `gap.json` writes. Runs natively so it inherits the host's
  `~/.claude/` auth, SSH keys, and git config. Chat and Import subprocesses
  additionally strip `ANTHROPIC_API_KEY` / `CLAUDE_API_KEY` (+ a few related
  vars) and resolve `claude` via the user's interactive login-shell `PATH`,
  so they behave like the user's terminal `claude` regardless of the
  systemd-user manager's stripped env.
- They communicate over a Unix-domain socket inside the volume root, which is
  bind-mounted into the webapp container.

## Layout

```
refine/
├── refine/               # the `refine` CLI: init, start, stop, status, runner, web, doctor
├── refine_shared/        # storage, IPC, friendly summaries, config loader
├── refine_runner/        # host-native daemon (subprocess + git + gap.json owner)
├── refine_web/           # Dockerized webapp + static HTML/JS
├── Dockerfile            # builds refine-web
├── docker-compose.yml    # runs refine-web (relative bind mounts, no env vars)
├── pyproject.toml        # makes `refine` a real console script
└── spec.md               # the design document
```

## Quick start

### 1. Clone refine once per client

```bash
git clone https://github.com/buwilliams/refine.git /opt/refine-acme
```

(You can have multiple clones if you're working across clients —
`/opt/refine-acme`, `/opt/refine-globex`, etc. Each is paired with one
client.)

### 2. Bind the clone to the client repo

```bash
cd /opt/refine-acme
uv run refine init /srv/clients/acme-app
```

This:
- Creates `/srv/clients/acme-app/.refine/refine.toml` + `run/` + `gaps/` +
  `.gitignore` (the client's volume root — hidden by convention, since it's
  system-utility state, not project source).
- Writes `/opt/refine-acme/.refine-binding` so future commands from
  `/opt/refine-acme` target this client.
- Writes `/opt/refine-acme/.env` so `docker compose` reads the bind-mount path.
- Installs and enables `~/.config/systemd/user/refine-acme.service` so the
  runner is managed by systemd (survives terminal close; one unit per clone,
  named after the clone basename).

Commit the new files in the client repo when you're ready:

```bash
cd /srv/clients/acme-app
git add .refine/refine.toml .refine/.gitignore
git commit -m "add refine"
```

### 3. Run from the refine source dir

```bash
cd /opt/refine-acme

claude login                       # one-time, as the operator user
uv run refine start                # webapp + runner, one command
uv run refine status               # check it's healthy
uv run refine stop                 # tear it all down
```

Open <http://localhost:8080>.

`refine start` rebuilds the web image if any source file is newer than the
image, brings the webapp up with `docker compose up -d`, starts the runner
via `systemctl --user start refine-acme`, and waits for both to be reachable
before returning. Runner logs go to journald — tail with
`journalctl --user -u refine-acme -f`.

To survive logout / reboot, run once:

```bash
loginctl enable-linger $USER       # systemd keeps user units alive across logout
```

UI edits are picked up live — `refine_web/static/` is bind-mounted into
the container, so changes to `index.html`, `app.js`, or `style.css` are
visible on the next browser refresh without rebuilding the image.

For a different client, `cd /opt/refine-globex` (or wherever) and run the
same commands. Each clone tracks its own binding and its own systemd unit
(named after the clone's directory basename).

### Re-binding

To point an existing refine clone at a different client:

```bash
cd /opt/refine-acme
uv run refine init /srv/clients/other-client --force
```

`--force` is required because a binding already exists. The unit file is
rewritten in place; the clone's directory name — and thus its unit name —
does not change.

## How it talks to itself

```
┌─────────────────┐                ┌─────────────────────┐
│  refine-web     │ ── IPC ──────► │  refine-runner      │
│  (Docker)       │  (Unix sock)   │  (host process)     │
│                 │ ◄── SQLite ──► │                     │
└─────────────────┘   (bind mount) └─────────────────────┘
        ▲                                    │
        │                                    ▼
        └─── reads gap.json ─────────► writes gap.json (sole writer)
```

- **`gap.json` writes** are runner-only — the webapp sends round
  submissions / edits / log appends to the runner over IPC.
- **SQLite** is shared (WAL + busy retry). Webapp owns settings, reporters,
  pause flag, and `status` for non-runner user transitions; runner owns run
  state, agent-driven status changes, and most activity entries.
- **SSE** is fed by a webapp-side poller that tails the SQLite `activity`
  table and watches `gaps_index` status changes.

## Configuration

A single TOML file is the only thing operators edit:

```toml
# .refine/refine.toml (created by `refine init`)
client_repo  = ".."                  # relative to this file (= the client repo root)
runner_socket = "./run/runner.sock"
[web]
host = "0.0.0.0"
port = 8080
```

Almost everything else — parallel-run cap, idle timeout, hard cap, branch
naming, reporters — lives in the SQLite settings table and is editable from
the UI's Settings page.

## Operational assumptions

- The host running refine is dedicated to refine — no human edits the client
  repo's working copy directly; all local commits come from refine agents.
- The client's developers push from their own machines; refine sees those
  commits via `fetch` and folds them in during `verify`.

## Auth model

- **Refine** has no authentication (no login). Deploy on a trusted private
  network.
- **Reporters** provide unverified identification: each round records who
  submitted it via a free-form name selected from a dropdown. Anyone can pick
  anyone — by design (see `spec.md → Reporters`). Renaming a reporter in
  Settings cascades through every Gap's `rounds[].reporter` strings so the
  dropdown and historical data stay in sync; removing a reporter deliberately
  does *not* cascade, so audit history of who submitted what is preserved.
- **Claude auth** uses the OAuth login persisted under `~/.claude/` by
  `claude login` on the host. Chat and Import explicitly strip API-key env
  vars so a stale `ANTHROPIC_API_KEY` can't override the OAuth session.

## CLI reference

| Command                       | What it does                                                                                                |
|-------------------------------|-------------------------------------------------------------------------------------------------------------|
| `uv run refine init <path>`   | Write `.refine/refine.toml` + `run/` + `gaps/`, bind this clone, install + enable a systemd --user unit.    |
| `uv run refine start`         | Rebuild image if stale → `docker compose up -d` → `systemctl --user start <unit>` → wait for both healthy.  |
| `uv run refine stop`          | `systemctl --user stop <unit>` + `docker compose down`.                                                     |
| `uv run refine status`        | Read-only: show webapp + runner state and where to tail logs.                                               |
| `uv run refine runner`        | Run the runner in the foreground (what the systemd unit invokes).                                           |
| `uv run refine web`           | Start the webapp in-process (rarely used directly — Docker wraps it).                                       |
| `uv run refine doctor`        | Deeper diagnostic snapshot: config, IPC, claude auth, git status.                                           |

All commands accept `--config /path/to/refine.toml` to bypass discovery.

## Running the tests

```bash
uv run python tests/smoke_test.py        # data-layer + storage
uv run python tests/integration_test.py   # runner + webapp end-to-end
```

The integration test boots a real runner and webapp on a temp directory and
exercises the full HTTP + IPC stack (excluding actual Claude CLI subprocesses
and git remotes, both of which need a configured host).

## Caveats / known scope

This is a v1 implementation tracking [`spec.md`](spec.md). Several
**out-of-scope** items from the spec remain out of scope: authentication,
external-tracker integrations, cross-instance sync, automatic retries.

# refine

refine turns behavior bug reports into merged commits. QA, Product, and
support describe a **Gap** — what the app does today versus what it should
do instead — and refine launches a Claude Code agent in a git worktree
against the codebase. The agent makes the change, commits, and refine
merges the work back to the branch you have checked out. Gaps move
`todo → in-progress → review → done`, with `failed` and `cancelled` for the
unhappy paths; multiple Gaps run in parallel up to a configurable cap.

You drive everything from a web UI: a status dashboard, per-Gap activity
feeds, a filterable Logs view, and an interactive Chat that can be
standalone or attached to a Gap's worktree with that Gap's context
pre-loaded. Refine handles the git plumbing — worktrees, fetch, merge,
push, auto-committing its own state — and inherits Claude Code auth from
the host, so operators rarely need to think about either.

## Components

- **`refine-web`** — Python webapp in Docker (UI + JSON API + SSE).
- **`refine-runner`** — host-native daemon that owns CLI subprocesses, git
  operations, and `gap.json` writes. Runs natively so it inherits the host's
  `~/.claude/` auth, SSH keys, and git config.
- They communicate over a Unix-domain socket inside the volume root, which is
  bind-mounted into the webapp container.

## Layout

```
refine/
├── refine/               # the `refine` CLI: init, runner, web, doctor
├── refine_shared/        # storage, IPC, friendly summaries, config loader
├── refine_runner/        # host-native daemon (subprocess + git + gap.json owner)
├── refine_web/           # Dockerized webapp + static HTML/JS
├── scripts/              # refine-runner.service (example systemd unit)
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
uv run refine runner               # daemonizes; prints pid, socket, log path
docker compose up                  # webapp, reads the .env
uv run refine doctor               # config + IPC + claude + git status report
```

Open <http://localhost:8080>.

UI edits are picked up live — `refine_web/static/` is bind-mounted into
the container, so changes to `index.html`, `app.js`, or `style.css` are
visible on the next browser refresh without rebuilding the image.

For a different client, `cd /opt/refine-globex` (or wherever) and run the
same commands. Each clone tracks its own binding.

### Re-binding

To point an existing refine clone at a different client:

```bash
cd /opt/refine-acme
uv run refine init /srv/clients/other-client --force
```

`--force` is required because a binding already exists.

### Production runner

Install `scripts/refine-runner.service` as a systemd unit (edit `User`,
`Group`, and `WorkingDirectory` first). The unit runs `uv run refine runner`
from your refine clone. Zero env vars.

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
  anyone — by design (see `spec.md → Reporters`).

## CLI reference

| Command                       | What it does                                                       |
|-------------------------------|--------------------------------------------------------------------|
| `uv run refine init <path>`   | Write `.refine/refine.toml` + `run/` + `gaps/` in `<path>`; bind this clone. |
| `uv run refine runner`        | Start the host-native runner daemon.                               |
| `uv run refine stop`          | Stop the running runner (SIGTERM, escalates to SIGKILL on timeout). |
| `uv run refine web`           | Start the webapp (rarely used directly — Docker wraps it).         |
| `uv run refine doctor`        | Show config, IPC, claude auth, and git status.                     |

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

This is a v1 implementation tracking [`spec.md`](spec.md). Notable simplifications:

- **Import** uses paragraph-based extraction rather than an LLM call. The
  user reviews and edits each extracted draft before persisting; the structure
  (a runner IPC method for "extract") is in place to upgrade to LLM-driven
  extraction without touching the webapp.
- Several **out-of-scope** items from the spec remain out of scope:
  authentication, external-tracker integrations, cross-instance sync,
  automatic retries.

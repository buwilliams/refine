# refine

Background Claude Code agents that close behavior **Gaps** in client codebases,
driven by QA/Product. See [`spec.md`](spec.md) for the design — the rest of
this README is just how to run it.

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
├── scripts/              # run-runner.sh, refine-runner.service
├── Dockerfile            # builds refine-web
├── docker-compose.yml    # runs refine-web (relative bind mounts; no env vars)
└── spec.md               # the design document
```

## Quick start

There are no environment variables. Configuration lives in one file —
`refine/refine.toml` inside the client repo — created by `refine init`.

### 1. Initialize the client repo

```bash
cd /srv/clients/acme-app   # the client's git repo
python -m refine init       # writes refine/refine.toml + refine/run + refine/gaps
```

Commit `refine/refine.toml` and any `refine/gaps/*/gap.json` files to the
client repo. The auto-generated `refine/.gitignore` excludes the SQLite file
and the runner socket.

### 2. Start the host runner

Native on the host so it inherits `claude` auth, SSH keys, and git config.

```bash
cd /srv/clients/acme-app
claude login                # once, as the operator user
python -m refine runner     # finds refine.toml automatically
```

For production, install `scripts/refine-runner.service` as a systemd unit
(edit `WorkingDirectory` and `PYTHONPATH` first).

### 3. Start the webapp

In a separate terminal, from the same client-repo directory:

```bash
cd /srv/clients/acme-app
docker compose --file /opt/refine/docker-compose.yml up
```

(Or symlink `docker-compose.yml` into the client repo so you can just
`docker compose up`.)

Open <http://localhost:8080>.

### 4. Verify

```bash
cd /srv/clients/acme-app
python -m refine doctor
```

This prints config paths, IPC reachability, claude auth status, and git
state. Use it whenever something doesn't behave as expected.

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

A single TOML file is the only thing to edit:

```toml
# refine/refine.toml
client_repo  = ".."                  # relative to this file (= the client repo root)
runner_socket = "./run/runner.sock"
[web]
host = "0.0.0.0"
port = 8080
```

Almost everything else — parallel-run cap, idle timeout, hard cap, branch
naming, reporters — lives in the SQLite settings table, editable from the UI's
Settings page.

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

| Command              | What it does                                                       |
|----------------------|--------------------------------------------------------------------|
| `refine init`        | Write `refine.toml` + `run/` + `gaps/` in the chosen directory.    |
| `refine runner`      | Start the host-native runner daemon.                               |
| `refine web`         | Start the webapp (rarely used directly — Docker wraps it).         |
| `refine doctor`      | Show config, IPC, claude auth, and git status — your first stop when something breaks. |

All commands accept `--config /path/to/refine.toml` to bypass discovery.

## Running the tests

```bash
PYTHONPATH=. python3 tests/smoke_test.py        # data-layer + storage
PYTHONPATH=. python3 tests/integration_test.py   # runner + webapp end-to-end
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

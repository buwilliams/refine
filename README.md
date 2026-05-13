# refine

Background Claude Code agents that close behavior **Gaps** in client codebases,
driven by QA/Product. See [`spec.md`](spec.md) for the design — the rest of
this README is just how to run it.

## Components

- **`refine-web`** — Python webapp in Docker (UI + JSON API + SSE).
- **`refine-runner`** — host-native daemon that owns CLI subprocesses, git
  operations, and `gap.json` writes. Runs natively so it inherits the host's
  `~/.claude/` auth, SSH keys, and git config.
- They communicate over a Unix-domain socket mounted into the webapp container.

## Layout

```
refine/
├── refine_shared/        # storage, IPC protocol, friendly summaries (used by both)
├── refine_runner/        # host-native daemon
├── refine_web/           # Dockerized webapp + static HTML/JS
├── scripts/              # run-runner.sh, refine-runner.service
├── Dockerfile            # builds refine-web
├── docker-compose.yml    # runs refine-web
└── spec.md               # the design document
```

## Quick start

### 1. Prepare the client repo

Pick a host directory that *is* the client's git repo. Inside it, create a
volume root directory:

```bash
cd /srv/clients/acme-app
mkdir -p refine          # volume root for SQLite + gap JSON files
```

`refine/` ends up committed alongside the client's source (see `spec.md` →
*Storage*).

### 2. Start the host runner

The runner runs natively so it inherits the host's `claude` CLI auth, SSH keys,
and git config.

```bash
# One-time: log in to Claude on the host as the operator user.
claude login

# Run the runner.
export REFINE_CLIENT_REPO=/srv/clients/acme-app
export REFINE_VOLUME_ROOT=/srv/clients/acme-app/refine
export REFINE_RUNNER_SOCKET=/var/run/refine/runner.sock
sudo mkdir -p /var/run/refine && sudo chown $USER:$USER /var/run/refine
./scripts/run-runner.sh
```

For production, install `scripts/refine-runner.service` as a systemd unit.

### 3. Start the webapp

In a separate terminal:

```bash
export REFINE_HOST_VOLUME_ROOT=/srv/clients/acme-app/refine
export REFINE_HOST_SOCKET_DIR=/var/run/refine
export REFINE_UID=$(id -u)
export REFINE_GID=$(id -g)
docker compose up --build
```

Open <http://localhost:8080>.

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

## Operational assumptions (from spec)

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

## Where things live

| Setting               | Default                          | Where                            |
|-----------------------|----------------------------------|----------------------------------|
| Parallel-run cap      | 3                                | SQLite `settings` (UI)           |
| Idle timeout          | 15 min                           | SQLite `settings` (UI)           |
| Hard wall-clock cap   | 24 h                             | SQLite `settings` (UI)           |
| Branch name pattern   | `refine/<gap-id>`                | SQLite `settings` (UI)           |
| Merge target          | client repo's current branch     | (fixed policy)                   |
| Reporters list        | —                                | SQLite `reporters` (UI)          |
| Volume root           | (required)                       | env var (deploy-time)            |
| Runner IPC socket     | `/var/run/refine/runner.sock`    | env var (deploy-time)            |

## Caveats / known scope

This is a v1 implementation tracking [`spec.md`](spec.md). Notable simplifications:

- **Import** uses paragraph-based extraction rather than an LLM call. The
  user reviews and edits each extracted draft before persisting; the structure
  (a runner IPC method for "extract") is in place to upgrade to LLM-driven
  extraction without touching the webapp.
- Several **out-of-scope** items from the spec remain out of scope:
  authentication, external-tracker integrations, cross-instance sync,
  automatic retries.

## Running the tests

```bash
PYTHONPATH=. python3 tests/smoke_test.py        # data-layer + storage
PYTHONPATH=. python3 tests/integration_test.py   # runner + webapp end-to-end
```

The integration test boots a real runner and webapp on a temp directory and
exercises the full HTTP + IPC stack (excluding actual Claude CLI subprocesses
and git remotes, both of which need a configured host).

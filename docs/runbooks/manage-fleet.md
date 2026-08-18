# Manage the Fleet

A node is a self-contained Refine install: its own Refine checkout, daemon,
and agent provider, attached to the target app. A fleet is the collection of
nodes recorded in the target app's Refine state and synchronized through the
target repository's Git remote. Refine owns node identity, work distribution,
workflow, and state synchronization. The operating agent owns machine
creation and deletion on whatever infrastructure the user chooses — there is
no blessed provider and no provisioning script.

Use this document when an agent is responsible for managing a user's fleet:
inspecting it, adding workers on new or existing machines, moving work, or
retiring nodes. `refine fleet manage "<request>"` opens exactly this kind of
session with this runbook as context — as do the shorthand
`refine fleet "<request>"` and `refine fleet distribute "<instructions>"`
forms. Ask only the questions needed for the requested change, and do not
claim an operation succeeded until the fleet readback shows it.

## Preconditions

- A target project is attached and has the configured **Git remote** (default
  `origin`) where Refine publishes its dedicated `refine/state` branch. Run
  `refine project status` and `refine sync` before fleet changes; do
  not infer publication from local state alone.
- The target repository is reachable from every worker, including credentials
  for a private repository. Interactive Git prompting is disabled on workers.
- **Git 2.42 or newer on every node.** Refine's state merge is `git
  merge-tree`, so a node with an older Git cannot synchronize at all: its
  daemon refuses to start with a message naming the required and observed
  versions, and `refine fleet sync` reports it as `unsupported_git`. Upgrading
  Git on that node is the whole fix, and the rest of the fleet keeps
  converging in the meantime.
- For any machine you create or destroy, the user has approved the
  infrastructure, account, size, and expected cost.

## Ask If You Cannot Infer

- What should change: add capacity, retire a node, move work, or inspect
  health?
- Where should a new worker run: an existing machine reachable over SSH, or a
  new VM or container on infrastructure the user picks?
- What stable lowercase node id should the worker use?
- Which agent provider should the worker run, and how will its credential or
  subscription login be supplied?
- For cloud machines: which account, region, size, and cost ceiling?

## Inspect the fleet

```bash
refine fleet list
refine fleet show <node-id>
refine node list
refine next
```

`refine fleet list` reports each node's enablement, connection, and health —
including `pending_upgrade` for a node that has not been upgraded yet.
`refine next` recommends the next fleet operations with exact commands.

## Add a worker

1. Register the node identity first:

```bash
refine fleet add-node <node-id>
```

2. Create or choose the machine. This step is agent-owned and
   provider-specific: an existing host over SSH, a VM, or a container all
   work. The machine needs a Linux or macOS environment, network access to
   the target repository's Git remote, and the standard dependencies
   (`curl`, `git`, a C compiler/linker, Rust Cargo).
3. Install Refine on the machine by following
   [the install runbook](install.md) there.
4. Turn the machine into this fleet's worker. `refine node init` is
   idempotent and reads arguments or `REFINE_*` environment variables
   (`REFINE_NODE_ID`, `REFINE_TARGET_REPO_URL`, `REFINE_AGENT_PROVIDERS`,
   `REFINE_TARGET_PATH`):

```bash
cd <refine-checkout>
./r node init --node-id <node-id> --repo-url <target-repo-url> \
  --agent-providers <provider> --port <port>
./r system start --port <port>
```

   For a worker Refine reaches over SSH, record the connection first and use
   bootstrap instead of running commands by hand:

```bash
refine fleet edit-node <node-id> --ssh-host <host> --ssh-user <user> \
  --ssh-identity-path <key> --refine-checkout <path> --target-app-path <path> \
  --refine-port <port>
refine fleet bootstrap <node-id> --dry-run
refine fleet bootstrap <node-id>
```

5. Enable the node and verify it from the control machine:

```bash
refine fleet enable-node <node-id>
refine fleet show <node-id>
refine fleet run <node-id> "refine project status"
refine fleet run <node-id> "refine agent detect"
```

No agent API key is required to create a worker. Without one, node identity,
repository attachment, synchronization, and daemon operation still work;
agent execution waits until an appropriate credential or subscription login
exists.

## Move work

Moving work never touches machines: it reassigns Goal ownership between
already-registered nodes and syncs state through Git. No SSH is involved.

For an even spread or convergence, follow
[Distribute and converge work](distribute-and-converge.md). The short form:

```bash
refine fleet distribute --dry-run
refine fleet distribute
refine fleet distribute --converge --to <node-id>
```

When the user gives placement instructions the built-in spread cannot express
(for example: "distribute the backlog close to evenly, but keep related Goals
together on one node"), plan the assignments yourself and apply them
Goal by Goal:

1. Read the current state: `refine fleet list` for enabled healthy nodes,
   `refine goal list` for eligible Goals (captured or actionable) and their
   contents.
2. Group and place the Goals per the instructions, respecting workflow policy
   limits and Feature ordering.
3. Apply each assignment: `refine fleet transfer <node-id> <goal-id>`.
4. Publish the new ownership so every node observes it: `refine fleet sync`.
   Its per-node statuses say which nodes were asked and what each answered —
   `queued` is that node's receipt for the request, not proof it has already
   reconciled. A node reported `pending_upgrade` still received the ownership
   change through `refine/state`.
5. Read back `refine fleet list` and `refine goal list`, and report the
   resulting placement to the user.

## Upgrade the fleet

Upgrade nodes **one at a time, in any order**. There is no fleet-wide upgrade
step, no window in which every node must be on the same build, and no need to
stop the fleet. Each node is upgraded by following
[the install runbook](install.md) on that node; a node's CLI and daemon are one
binary, so upgrading a node upgrades both at once and no node is ever running a
mixture.

While a rollout is in progress the fleet is mixed, and that is a supported
state:

- Synchronization keeps converging. Upgraded and not-yet-upgraded nodes publish
  to the same `refine/state` branch and each reads the other's commits; neither
  deletes nor loses the other's records.
- `refine fleet sync` reports one status per node and succeeds. A node whose
  daemon is still on the previous API contract is reported as
  `pending_upgrade`, with the contract version it speaks and the one this node
  speaks. The rest of the fleet still syncs. The statuses are that command's
  output, not a rewrite of the fleet's registry: `refine fleet list` keeps
  showing each node's provisioning health, which no sync answer changes.
- A `pending_upgrade` node keeps its work and keeps receiving distributed work.
  It is a working node whose turn has not come, not a broken one.

To roll out:

1. Pick a node and check the fleet first: `refine fleet list`.
2. Upgrade that node and restart its daemon on the same port.
3. Confirm from the control machine: `refine fleet sync`. The upgraded node's
   status in that output must no longer be `pending_upgrade`. That answer is
   what proves the build changed; the node's own reconciliation is reported by
   that node, through its own `refine sync` and its state-sync health.
4. Repeat for the next node. Stop and report if a node's status becomes
   `failed` or stays `unreachable` after its daemon is back. A node reported
   `unsupported_git` needs its Git upgraded to 2.42 or newer before it can
   sync; like `pending_upgrade` it is that node's own condition and does not
   hold up the rest of the rollout.

Do not treat `pending_upgrade` on the nodes you have not reached yet as a
failure, and do not "fix" it by editing synchronized state, forcing a sync, or
upgrading everything at once.

## Retire a worker

Pause new admission, move both ordinary open work and Review work away, and
only then destroy the machine. Distribution cannot move a Goal in an automated
state; wait for it to settle or stop it through the supported Goal/process
lifecycle first.

```bash
refine fleet disable-node <node-id>
refine fleet distribute --to default --dry-run
refine fleet distribute --to default
refine fleet distribute --converge --to default --dry-run
refine fleet distribute --converge --to default
refine fleet show <node-id>
```

When the final readback shows no open or reviewable work owned by the node,
destroy the machine with the user's infrastructure tooling, then:

```bash
refine fleet remove-node <node-id>
```

`disable-node` prevents new distribution but does not cancel or transfer
already-running Goal work.

## Verify

After any change, confirm from the control machine that `refine fleet list`
shows the expected nodes, enablement, and health, and that
`refine goal list` shows the expected ownership. For a new worker, confirm
its daemon answers, the target project is attached, and the registered fleet
state appears on it before distributing work.

## Common failures

- `node init` cannot clone the target: provide repository credentials the
  worker can use; interactive Git prompting is disabled.
- The daemon starts without a project: inspect the `REFINE_*` values given to
  `node init` and rerun it on the worker.
- The agent CLI exists but cannot execute: install or inject the selected
  provider's authentication without changing the node identity.
- Fleet state does not appear: inspect `refine system status` and the System
  process diagnostics. Reconciliation retries automatically; users should not
  edit, stash, commit, or force-push application files to repair Refine state.
- A Goal never schedules on its owning node: read the Goal's own round logs
  (`runtime/goals/<shard>/<id>/logs.jsonl` under that node's live-state
  directory) — settled failures and retries land there, in the Goal's durable
  state. See "Refine-owned durable state" in `docs/runbooks/install.md` for
  the other Refine-owned artifacts an operator may encounter.

## Recover a contested first contact

Use this procedure only when a node joining an existing fleet fails closed:
its live store already carries non-bootstrap records that contest content on
the configured remote's `refine/state` branch, and there is no shared history
to merge from. Ordinary divergence between nodes that share history is decided
automatically (see `docs/runbooks/state-sync-recovery.md`).

The daemon normally converges this automatically with remote authority. On an
opted-out node, review the divergence and settle it in one command:

```bash
refine sync --preview
refine sync --authority remote
# or: --authority live, with --path exceptions taken from the other side
```

The preview is read-only JSON — the classification (`join` for a first
contact), per-path sides, and a domain summary for each contested path. It is
not a token; the terminal run re-derives everything itself.

`refine sync --authority remote` retains the node's complete pre-recovery live
store under `refs/refine/retained/live-<head>` before hydrating the remote
branch into live, then joins the branch. `refine sync --authority live` keeps
the live store, hydrates remote-only records additively, and publishes.
Rerunning after success finds converged heads and is a no-op. The daemon API
equivalent is `GET /api/sync/preview` and `POST /api/sync` with an
`{"authority": ...}` body, which also settles state-sync health.

Never run this live recovery from a Goal candidate or other isolated worktree.
Run it through the daemon attached to the production target app. Recovery never
accepts a remote override, rewrites application history, or changes the
application branch, index, or worktree.

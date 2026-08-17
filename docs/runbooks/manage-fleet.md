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
  `refine project status` and `refine project sync` before fleet changes; do
  not infer publication from local state alone.
- The target repository is reachable from every worker, including credentials
  for a private repository. Interactive Git prompting is disabled on workers.
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

`refine fleet list` reports each node's enablement, connection, and health.
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
5. Read back `refine fleet list` and `refine goal list`, and report the
   resulting placement to the user.

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

## Recover a missing synchronization baseline

Use this procedure only when sync explicitly reports that its persisted
three-way baseline is missing while non-bootstrap live state and the configured
remote's existing `refine/state` branch are both present. Ordinary sync remains
fail-closed in this topology and does not choose either side.

When the authority choice is already made — for fleet convergence that is
normally `remote` — prefer the consolidated one-shot command and skip the rest
of this procedure:

```bash
refine project state-recovery run --authority remote
```

It performs sync, preview, apply, and the verifying sync as one operation with
bounded internal race retries (see `docs/runbooks/state-sync-recovery.md`). Do
not wrap it in your own retry loop. Use the manual preview flow below when an
operator needs to review the comparison before choosing authority.

1. Run the preview through the daemon-backed CLI and save the complete JSON:

```bash
refine project state-recovery preview > /tmp/refine-state-recovery-preview.json
```

   The preview is read-only. Confirm its target identity, configured remote,
   exact local and remote state heads, missing baseline status, live-only,
   remote-only, equal, and differing counts, and its bounded conflicting path
   list. Do not edit the preview: the complete object is stale-fenced evidence.
2. Choose authority explicitly:

   - `live` anchors at the observed remote head, then uses normal state delta
     semantics to publish live additions, modifications, and deletions as one
     linear non-force commit. A path present only on the remote is deleted.
   - `remote` first commits the complete pre-recovery live durable state to a
     dedicated recovery ref, then compare-and-swap hydrates the observed remote
     state into the live store.

3. Apply the reviewed evidence:

```bash
refine project state-recovery apply --authority live \
  --preview-file /tmp/refine-state-recovery-preview.json
# or: --authority remote
```

   The equivalent daemon API is
   `GET /api/project/state-recovery/preview` followed by
   `POST /api/project/state-recovery/apply` with `authority` and the complete
   `preview` object. When authoritative Dashboard health reports recovery kind
   `missing_baseline`, the Dashboard exposes the same neutral preview and
   requires a separate authority choice and exact-preview confirmation. It
   never recommends or preselects authority.
4. Read the result and inspect the reported recovery ref and manifest under the
   repository's Git common directory in `refine-state-recoveries/`. The bounded
   manifest records authority, node, target and remote identity, before/after
   heads, counts, timestamps, outcome, and recovery location. Only a successful
   result creates the baseline.
5. If apply fails or is interrupted, do not delete its recovery ref or
   manifest and do not fabricate a baseline. A `409` reason of `git_busy`
   retains the same preview for retry, but requires deliberate confirmation
   again. A `409` reason of `stale_preview` requires a new preview, authority
   choice, and confirmation. Any changed target, repository, remote, remote
   head, or unrelated live write likewise requires new operator review. After
   success, confirm that Dashboard state-sync health cleared and retain the
   displayed audit ref, manifest, published and local heads, evidence identity,
   counts, authority, and detail.

Never run this live recovery from a Goal candidate or other isolated worktree.
Run it through the daemon attached to the production target app. Recovery never
accepts a remote override, force-pushes, rewrites history, or changes the
application branch, index, or worktree.

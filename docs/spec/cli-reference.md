# CLI Reference

Generated from the clap command tree by `cargo run --manifest-path xtask/Cargo.toml -- cli-reference`.
Do not edit by hand — a unit test fails when this file drifts from the binary.

Agents: `refine commands` emits this same tree as JSON; `refine next` recommends
which of these commands to run from current state.

## `refine project`

Manage which target application this Refine instance operates on. Attach, clone, switch, register, and diagnose target app repositories

### `refine project status`

Show which target app is currently attached and the state of the project registry

- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state

### `refine project attach`

Attach an existing local repository as the current target app. The path is registered and becomes the app Refine operates on

- `<PATH>` (required) — Filesystem path to the target app repository
- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state

### `refine project switch`

Switch the current target app to another registered project by name. Migrates the project's Refine state if it uses an older schema

- `<NAME>` (required) — Registered project name to make current
- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state

### `refine project detach`

Detach the current target app so no project is active. Registered projects are kept; nothing is deleted from disk

- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state

### `refine project register`

Register a local repository as a named project without making it current

- `<NAME>` (required) — Project name to register under
- `<PATH>` (required) — Filesystem path to the target app repository
- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state

### `refine project clone`

Clone a git repository to a local destination and register it as a project. Use --make-current to also attach it as the current target app

- `<SOURCE>` (required) — Git URL or path to clone from
- `<DESTINATION>` (required) — Local directory to clone into
- `--name` — Project name to register (derived from the source when omitted)
- `--make-current` — Also switch to the cloned project as the current target app
- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state

### `refine project remove`

Remove a project from the registry by name. Files on disk are not deleted

- `<NAME>` (required) — Registered project name to remove
- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state

### `refine project migrate`

Migrate the current project's on-disk Refine state to the latest schema and report what changed

- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state

### `refine project sync`

Commit durable Refine state, pull/rebase and push the current branch, then rebuild projections. Uncommitted target-app files are left untouched; optionally persists the projection snapshot

- `--cache-dir` — Cache directory to persist the rebuilt projection snapshot into

### `refine project doctor`

Run project-level diagnostics against the attached target app and report problems

- `--runtime-root` — Runtime directory where Refine keeps daemon and registry state
- `--repo-root` — Path to the Refine checkout used for repository diagnostics

## `refine goal`

Create and drive Goals — prompt-driven units of work for the active app. Covers the full lifecycle: create, round, start, retry, verify, merge, undo

### `refine goal create`

Create a new prompt-driven Goal. It starts in the backlog; add a round to describe the behavior, then `goal start` to begin work

- `<NAME>` (required) — Human-readable Goal name
- `--id` — Explicit Goal id (generated when omitted)

### `refine goal list`

List all Goals with their status and ownership

### `refine goal show`

Show full detail for one Goal: status, rounds, notes, and ownership

- `<ID>` (required) — Goal id

### `refine goal edit`

Edit a Goal's metadata (name and/or priority). Only valid while the Goal's status allows editing

- `<ID>` (required) — Goal id
- `--name` — New Goal name
- `--priority` — New priority value

### `refine goal note`

Append a free-form note to a Goal for context that agents and humans should see

- `<ID>` (required) — Goal id
- `<BODY>` (required) — Note text
- `--author` — Author label recorded on the note

### `refine goal note-edit`

Replace the body of an existing note on a Goal

- `<ID>` (required) — Goal id
- `<NOTE_ID>` (required) — Id of the note to edit
- `<BODY>` (required) — Replacement note text

### `refine goal note-delete`

Delete a note from a Goal

- `<ID>` (required) — Goal id
- `<NOTE_ID>` (required) — Id of the note to delete

### `refine goal round`

Record an actionable prompt as a round on a Goal. Requires --reporter and --prompt unless --edit-latest amends the newest round

- `<ID>` (required) — Goal id
- `--reporter` — Who is reporting this round
- `--prompt` — The work prompt for the agent
- `--edit-latest` — Edit the most recent round instead of appending a new one

### `refine goal start`

Start work on a Goal: moves it from backlog/todo to in-progress so the agent workflow picks it up

- `<ID>` (required) — Goal id

### `refine goal cancel`

Cancel a Goal: any not-yet-done Goal becomes cancelled. Done Goals cannot be cancelled (use undo first)

- `<ID>` (required) — Goal id

### `refine goal retry`

Retry a failed stage for a Goal: --stage quality returns it to QA, --stage merge to ready-merge

- `<ID>` (required) — Goal id
- `--stage` — Stage to retry: "quality" (back to QA) or "merge" (back to ready-merge)

### `refine goal verify`

Approve a Goal that is in review: marks it done after the change has been verified

- `<ID>` (required) — Goal id

### `refine goal merge`

Merge a ready-merge Goal and mark it done. Requires the Goal to be in the ready-merge status

- `<ID>` (required) — Goal id

### `refine goal undo`

Walk a Goal's status backwards: done goes to review; review or cancelled goes to todo

- `<ID>` (required) — Goal id

### `refine goal delete`

Permanently delete a Goal record from project state. Irreversible; prefer cancel to keep history

- `<ID>` (required) — Goal id

### `refine goal assign-feature`

Assign a Goal to a Feature so it is grouped and ordered with related work

- `<ID>` (required) — Goal id
- `<FEATURE_ID>` (required) — Feature id to assign the Goal to

### `refine goal remove-feature`

Remove a Goal from its Feature. The Goal itself is kept

- `<ID>` (required) — Goal id

## `refine feature`

Manage Features — named groups of ordered Goals delivered together. Group, order, move, transfer, and bulk-import Goals under a Feature

### `refine feature create`

Create a Feature — a named group of ordered Goals delivered together

- `<NAME>` (required) — Human-readable Feature name
- `--id` — Explicit Feature id (generated when omitted)
- `--description` — Feature description
- `--reporter` — Reporter recorded on the Feature

### `refine feature list`

List all Features with their rollup status

### `refine feature show`

Show one Feature with its Goals and rollup status

- `<ID>` (required) — Feature id

### `refine feature edit`

Edit a Feature's metadata: name, description, or reporter

- `<ID>` (required) — Feature id
- `--name` — New Feature name
- `--description` — New Feature description
- `--reporter` — New reporter value

### `refine feature add-goal`

Add an existing Goal to a Feature

- `<ID>` (required) — Feature id
- `<GOAL_ID>` (required) — Goal id to add to the Feature

### `refine feature remove-goal`

Remove a Goal from a Feature. The Goal itself is kept

- `<ID>` (required) — Feature id
- `<GOAL_ID>` (required) — Goal id to remove from the Feature

### `refine feature reorder-goal`

Set a Goal's position within the Feature's ordered delivery sequence

- `<ID>` (required) — Feature id
- `<GOAL_ID>` (required) — Goal id to reposition
- `<ORDER>` (required) — New position in the Feature's ordered Goal sequence

### `refine feature order-goal`

Add a Goal to the Feature's ordered delivery sequence

- `<ID>` (required) — Feature id
- `<GOAL_ID>` (required) — Goal id to add to the ordered sequence

### `refine feature unorder-goal`

Remove a Goal from the Feature's ordered delivery sequence while keeping it in the Feature

- `<ID>` (required) — Feature id
- `<GOAL_ID>` (required) — Goal id to remove from the ordered sequence

### `refine feature move`

Move all of a Feature's eligible Goals to a workflow stage (backlog or todo)

- `<ID>` (required) — Feature id
- `<TARGET>` (required) — Target status for the Feature's Goals: "backlog" or "todo"

### `refine feature transfer`

Transfer ownership of a Feature and its Goals to another node in the fleet

- `<ID>` (required) — Feature id
- `<NODE_ID>` (required) — Destination node id

### `refine feature cancel`

Cancel a Feature: its cancellable Goals are cancelled as well

- `<ID>` (required) — Feature id

### `refine feature delete`

Permanently delete a Feature and its Goals. Irreversible; prefer cancel to keep history

- `<ID>` (required) — Feature id

### `refine feature import`

Bulk-import Goal drafts from text, structured JSON, or CSV, optionally attaching them to a Feature

- `--text` — Inline import source text (alternative to --file)
- `--file` — File to read the import source from (alternative to --text)
- `--csv` — Parse the input as CSV instead of structured or free text
- `--reporter` — Reporter recorded on the imported Goals
- `--feature-id` — Feature id to attach the imported Goals to

## `refine workflow`

Control the agent automation engine that advances Goals through their workflow (pause/resume)

### `refine workflow pause`

Pause the agent automation engine: no new Goal work is claimed until resumed

- `--runtime-root` — Runtime directory where Refine keeps daemon state

### `refine workflow resume`

Resume the agent automation engine after a pause so agents claim Goal work again

- `--runtime-root` — Runtime directory where Refine keeps daemon state

## `refine node`

Manage nodes — the machines that own active work — including turning this machine into a fleet node

### `refine node list`

List all nodes in the registry and show which one is active on this machine

### `refine node init`

Turn this machine into a working fleet node: clone or attach the target repo (from env or flags), activate the node identity, and select an agent provider. Runs at worker boot; idempotent

- `--node-id` — Node identity to activate for this machine
- `--repo-url` — Git URL of the target app repository to clone
- `--target-path` — Local path for the target app checkout
- `--agent-providers` — Comma-separated agent providers to enable (e.g. "claude")
- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--port` — Daemon port for this node

### `refine node show`

Show one node's record and whether it is the active node on this machine

- `<ID>` (required) — Node id

### `refine node create`

Create a new node record in the registry with default settings. Fails if the id already exists

- `<ID>` (required) — Node id to create

### `refine node activate`

Set the given node as this machine's active node identity. The node must exist and not be archived

- `<ID>` (required) — Node id to activate

### `refine node archive`

Archive a node so it can no longer be activated or receive work. The active node cannot be archived

- `<ID>` (required) — Node id to archive

### `refine node rename`

Change a node's display name

- `<ID>` (required) — Node id
- `<NAME>` (required) — New display name

### `refine node settings`

Print a node's settings object

- `<ID>` (required) — Node id

### `refine node transfer`

Transfer ownership of a Goal or Feature (by item id) to the given node

- `<ID>` (required) — Destination node id
- `<ITEM_ID>` (required) — Goal or Feature id to transfer

## `refine cluster`

Operate the cluster (the fleet of nodes): register and bootstrap nodes, distribute unclaimed Goal ownership, and run remote commands

### `refine cluster list`

List the cluster: every fleet node with its enablement, connection, and health details

### `refine cluster show`

Show one fleet node's full cluster record

- `<ID>` (required) — Node id

### `refine cluster add-node`

Register a new node in the cluster so it can be configured and receive distributed work

- `<ID>` (required) — Node id to add

### `refine cluster edit-node`

Edit a cluster node's connection settings: SSH details, paths, and ports

- `<ID>` (required) — Node id to edit
- `--display-name` — New display name
- `--ssh-host` — SSH hostname or address for reaching the node
- `--ssh-user` — SSH username
- `--ssh-identity-path` — Path to the SSH identity (private key) file
- `--ssh-port` — SSH port
- `--refine-checkout` — Path to the Refine checkout on the node
- `--target-app-path` — Path to the target app checkout on the node
- `--refine-port` — Port the node's Refine daemon listens on
- `--enabled` — Enable or disable the node for work distribution

### `refine cluster enable-node`

Enable a node so distribute can assign it work

- `<ID>` (required) — Node id to enable

### `refine cluster disable-node`

Disable a node so it stops receiving distributed work

- `<ID>` (required) — Node id to disable

### `refine cluster remove-node`

Remove a node from the cluster registry

- `<ID>` (required) — Node id to remove

### `refine cluster bootstrap`

SSH-bootstrap a manually configured node by git-pulling its Refine checkout. Requires the node's SSH settings to be configured; use --dry-run to preview the commands

- `<ID>` (required) — Node id to bootstrap
- `--dry-run` — Print the commands that would run without executing them

### `refine cluster distribute`

Reassign eligible unclaimed Goal ownership across the fleet. Spreads across enabled healthy nodes by default, fills one node with --to, or converges reviewable Goals home with --converge --to <node>

- `--to` — Send all moves to this node instead of spreading across the fleet
- `--converge` — Converge reviewable Goals back to the node given by --to
- `--dry-run` — Plan the moves without applying them

### `refine cluster sync`

Commit durable Refine state, pull/rebase upstream changes, and push the current branch

### `refine cluster run`

Run an authorized command on a node over SSH and print the result

- `<ID>` (required) — Node id to run the command on
- `<COMMAND>` (required) — Command line to execute on the node

### `refine cluster transfer`

Transfer ownership of a Goal or Feature (by item id) to the given node, updating cluster records

- `<ID>` (required) — Destination node id
- `<ITEM_ID>` (required) — Goal or Feature id to transfer

### `refine cluster maintenance`

Put the cluster into maintenance mode and report the updated cluster state

## `refine log`

Inspect the activity log: list, tail, query, and export entries, or build a support bundle

### `refine log list`

List recent activity log entries

- `--limit` — Maximum number of entries to return

### `refine log tail`

Show the most recent activity log entries (a short tail of the log)

- `--limit` — Maximum number of entries to return

### `refine log show`

Show one activity log entry by id

- `<ID>` (required) — Activity log entry id

### `refine log query`

Search the activity log with a text query and optional filters, with pagination

- `<Q>` (required) — Text to search for
- `--limit` — Maximum number of entries to return
- `--offset` — Number of matching entries to skip (for pagination)
- `--goal-id` — Only return entries for this Goal id
- `--severity` — Only return entries with this severity
- `--category` — Only return entries in this category
- `--actor` — Only return entries recorded by this actor

### `refine log export`

Export activity log entries as JSON with an exported count

### `refine log bundle`

Build a support bundle of diagnostics and logs for troubleshooting, redacting secrets by default

- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--repo-root` — Path to the Refine checkout to include repository diagnostics from
- `--redact-secrets` — Redact secrets from bundle contents

## `refine agent`

Manage coding agent providers (e.g. claude): detect, configure, authenticate, diagnose, and invoke directly

### `refine agent detect`

Detect which agent provider CLIs are installed and available on this host

### `refine agent configure`

Configure an agent provider so workflows can invoke it

- `--provider` — Agent provider name (e.g. "claude")

### `refine agent auth`

Check or initiate authentication for an agent provider

- `--provider` — Agent provider name (e.g. "claude")

### `refine agent diagnose`

Run diagnostics for an agent provider and report configuration or auth problems

- `--provider` — Agent provider name (e.g. "claude")

### `refine agent invoke`

Invoke an agent once with a prompt and print the result. Useful for testing provider setup

- `<PROMPT>` (required) — Prompt text to send to the agent
- `--provider` — Agent provider name (e.g. "claude")
- `--cwd` — Working directory for the agent run

### `refine agent resume`

Resume a previous agent session by session id, keeping its context

- `<SESSION_ID>` (required) — Agent session id to resume
- `--provider` — Agent provider name (e.g. "claude")

## `refine system`

Install, update, and operate the Refine daemon and service on this machine

### `refine system install`

Install Refine on this machine (macOS app bundle, Windows installer, or Linux CLI/web)

- `--port` (required) — Daemon port to configure for the installation
- `--target` — Install target; auto-detects the operating system by default
- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--version` — Version string to record for the installation

### `refine system repair`

Repair an existing installation: recreate launchers and services for the recorded version

- `--port` (required) — Daemon port the installation is configured for
- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--version` — Version string to record for the installation

### `refine system update`

Self-update Refine to the latest available version

- `--yes` — Skip the confirmation prompt
- `--runtime-root` — Runtime directory where Refine keeps daemon state

### `refine system rollback`

Roll the installation back to a previously installed version

- `--port` (required) — Daemon port the installation is configured for
- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--version` — Version string to roll back around

### `refine system uninstall`

Uninstall Refine from this machine

- `--port` (required) — Daemon port the installation is configured for
- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--version` — Version string of the installation to remove

### `refine system start`

Start the Refine daemon (background by default; --foreground or --once run it in-process)

- `--port` — Port for the daemon to listen on
- `--bind-address` — IP address to bind the listener to
- `--cache-dir` — Directory for the projection cache
- `--static-root` — Directory of static web assets to serve
- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--once` — Serve a single request then exit (useful for smoke tests)
- `--foreground` — Run in the foreground instead of spawning a background daemon

### `refine system stop`

Stop the Refine daemon running on the given port

- `--port` — Port the daemon is listening on
- `--runtime-root` — Runtime directory where Refine keeps daemon state

### `refine system restart`

Restart the Refine daemon on the given port

- `--port` — Port the daemon is listening on
- `--runtime-root` — Runtime directory where Refine keeps daemon state

### `refine system status`

Report daemon status for the given port: health, worker state, and target app state

- `--port` — Port the daemon is listening on
- `--runtime-root` — Runtime directory where Refine keeps daemon state

### `refine system ps`

List running Refine daemon processes; optionally stop one with --stop

- `--port` — Only inspect the daemon on this port
- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--stop` — Identifier of the process to stop
- `--signal` — Signal to send when stopping ("terminate" or "kill")

### `refine system doctor`

Run system-level diagnostics covering the daemon, runtime, and repository, and report problems

- `--runtime-root` — Runtime directory where Refine keeps daemon state
- `--repo-root` — Path to the Refine checkout used for repository diagnostics

### `refine system api-groups`

Print the daemon HTTP API groups and the capability each one requires

## `refine next`

Recommend the next operations from current project and fleet state, each with the exact command to run. Start here when unsure what to do

## `refine commands`

Print a machine-readable JSON catalog of every CLI command with descriptions. Load this once instead of exploring --help per subcommand

## `refine website`

Serve the Refine website as a local static file server (no daemon or project state required)

- `--port` — Port to listen on
- `--bind-address` — IP address to bind the listener to
- `--static-root` — Directory containing the static website files to serve
- `--once` — Serve a single request then exit (useful for smoke tests)

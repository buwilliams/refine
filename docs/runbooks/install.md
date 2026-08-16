# Install or Update Refine

Refine is an agentic software delivery system that runs locally against a user's application repository. It coordinates agents and humans through Goals, workflow state, provider CLIs, local processes, and a browser UI so software changes can move from request to implementation to human review.

Use this document when an agent is responsible for installing or updating Refine. Follow the steps in order, ask the user only the questions needed for the chosen path, confirm where Refine should be installed when you cannot infer it, and do not claim installation succeeded until the CLI reports a healthy running system or you have reported the exact blocker.

## Prerequisites

- Run on Linux, macOS, or Ubuntu/WSL. Windows users should open Ubuntu through WSL first.
- Use a `bash` shell with network access.
- Determine which dependency sources are available on this host before installing anything: system package managers, existing corporate mirrors, preinstalled toolchains, or manual user setup.
- Make sure the user can approve dependency installation from the available source or choose to install missing dependencies manually.
- Install or repair required dependencies before cloning or updating Refine: `curl`, `git`, a C compiler/linker, and Rust Cargo.
- If using a real provider, make sure the user can complete that provider's CLI authentication on this host.

## Ask If You Cannot Infer

Ask only when the answer is not clear from the user's environment, prior conversation, or existing files. Keep defaults unless the user has given a reason to choose otherwise.

- Which agent provider should Refine use: `claude`, `codex`, `gemini`, or `copilot`?
- Where should Refine be installed? Default: `$HOME/refine`.
- Which UI port should Refine use? Default: `8082`.
- Which available dependency source should the agent use for missing tools?
- Should missing provider CLI installation or provider authentication happen now, or should the user complete it later?

## Install Refine

1. Resolve the Refine checkout path before running install commands. If an existing Refine checkout or a user preference is not clear, ask where to install Refine and use `$HOME/refine` as the default.
2. Check for required tools, identify reachable dependency sources, and install missing dependencies only from a source the user approves; the agent should make dependency choices explicitly.

```bash
curl --version
git --version
cc --version
cargo --version
```

3. If Refine is already installed, follow the [Update Refine](#update-refine) section instead of a fresh clone, then continue with provider configuration below.
4. For a fresh install, copy the latest published release files without a `.git` directory:

```bash
latest="$(
  git ls-remote --tags --refs https://github.com/buwilliams/refine.git \
    | awk -F/ '/refs\/tags\/[0-9]+\.[0-9]+\.[0-9]+$/ { print $NF }' \
    | sort -t. -k1,1n -k2,2n -k3,3n \
    | tail -n 1
)"
tmp="$(mktemp -d)"
git clone --depth 1 --branch "$latest" https://github.com/buwilliams/refine.git "$tmp/refine"
mkdir -p <refine-checkout>
tar -C "$tmp/refine" --exclude .git -cf - . | tar -C <refine-checkout> -xf -
rm -rf "$tmp"
```

5. Install Refine for the selected port. This command assumes the prerequisites above are available, builds the locked release binary, atomically publishes it as `bin/refine`, marks the checkout as deployed, and only then registers and activates the service:

```bash
cd <refine-checkout>
./r system install --port <port>
```

6. Configure the selected provider:

```bash
cd <refine-checkout>
./r agent configure --provider <provider>
./r agent detect
```

7. If the selected provider CLI is missing, install or authenticate it only after the user approves. Treat Refine installation and provider readiness separately.
8. Use Refine's provider adapter when the user approves authentication now,
   then diagnose the same provider. This avoids baking provider-specific login
   syntax into the installation contract:

```bash
./r agent auth --provider <provider>
./r agent diagnose --provider <provider>
```

Do not offer `smoke-ai` during installation. It is reserved for deterministic tests.

## Product and runtime ownership

The directory selected above is the product home. `./r` always runs the
stable production binary `<refine-checkout>/bin/refine` — never a debug
build. `./r system start`, `./r system install`, and `./r system build`
create or refresh that binary (rebuilding only when the source has changed
since the last production build) and say so on stdout; every other command
requires it to already exist. The base runtime is `<refine-checkout>/run`,
and port `<port>` owns only `<refine-checkout>/run/<port>`. `./r` anchors the
invocation to that product home; the directory from which a user launches it
is not an ownership signal. Do not configure HOME, XDG, platform support
directories, or a neighboring checkout as the runtime root.

Manage the production binary directly with:

```bash
./r system build   # rebuild bin/refine from source
./r system clean   # remove bin/refine and the deployed marker
```

These are checkout-launcher operations, not `refine system` subcommands, so
they are intentionally absent from `refine system --help` and `refine commands`.
Use `./r system build --help` or `./r system clean --help` for their direct help.

A published installation is intentionally gitless and is identified by
`.refine-deployed` plus `bin/refine`. It supports ordinary daemon, web, MCP,
provider, and published-update operations. Source status and source promotion
require an actual Git checkout and will fail closed for a gitless product home.

`./r system install` is the complete fresh-install boundary; callers do not
prebuild or copy the binary separately. Current service registrations launch the checkout-local binary with the exact
port and checkout-local runtime and use the checkout as their working
directory. The current targets are `macos_daemon`, `windows_daemon`, and
`linux_cli_web`. The historical JSON values `mac_os_app_bundle` and
`windows_installer` are accepted only when reading migration-era state; new
state is always written with current names.

Ordinary install/status reports a conflicting legacy external runtime or
registration without changing it. `./r system repair --port <port>` is the
explicit migration boundary: it leaves external runtime and binary trees
untouched, stores the exact original registration bytes, SHA-256, parsed
identity, and final outcome under
`run/<port>/installation-migrations/`, then atomically publishes only the new
registration. If activation or byte verification fails, Refine restores the
exact original registration and retains the journal.

## Update Refine

`./r system update` is the deterministic one-command update for a Git source
checkout. It first fetches and checks the configured upstream. If there are no
new upstream commits, it exits without stopping Refine or modifying the
checkout. When an update is available, it runs these commands in order and
stops immediately if any command fails:

```bash
./r system stop
git stash && git pull
./r system build
./r system start
```

The stash is not reapplied automatically. Because this path intentionally uses
plain `git stash`, untracked files are not included. The command uses the
default daemon port and runtime root and accepts no arguments.

The web UI's Controls > Management > Update Refine control remains a separate,
restart-safe source-promotion workflow with durable progress and recovery.

The steps below remain for **gitless published installations** (a checkout
without a usable `.git` directory), where the Git commands in `./r system
update` cannot succeed.

1. Stop the running daemons first:

```bash
cd <refine-checkout>
./r system stop --port <port>
```

2. Fetch the latest release and copy it over the checkout without a `.git` directory:

```bash
latest="$(
  git ls-remote --tags --refs https://github.com/buwilliams/refine.git \
    | awk -F/ '/refs\/tags\/[0-9]+\.[0-9]+\.[0-9]+$/ { print $NF }' \
    | sort -t. -k1,1n -k2,2n -k3,3n \
    | tail -n 1
)"
tmp="$(mktemp -d)"
git clone --depth 1 --branch "$latest" https://github.com/buwilliams/refine.git "$tmp/refine"
tar -C "$tmp/refine" --exclude .git -cf - . | tar -C <refine-checkout> -xf -
rm -rf "$tmp"
```

3. Rebuild the release binary and mark the checkout as deployed:

```bash
cd <refine-checkout>
./r system build
```

4. Restart Refine and verify it is healthy:

```bash
./r system start --port <port>
./r system status --port <port>
```

## After Install

1. Start and check Refine:

```bash
cd <refine-checkout>
./r system start --port <port>
./r system status --port <port>
./r system doctor --repo-root .
```

2. Open the UI at `http://localhost:<port>`. The default is `http://localhost:8082`.
3. Attach or create the target app if the target is already clear:

```bash
./r project attach /path/to/app
./r project clone <remote-url> /path/to/app --make-current
```

4. If creating a new app, run the user-approved starter command in the new app directory, make the initial git commit, then attach the app.
5. Finish with the Refine checkout path, UI URL, selected provider, target app status, and summaries from `./r system status` and `./r system doctor`.
6. If no target app is clear, leave Refine running without one and report that the target app still needs to be selected.
7. Ask only the app guidance needed to continue: should Refine update an existing local app, clone an existing remote app, create a new app, or wait with no target app yet?

## CLI Management

Use `./r --help` and `./r <group> --help` as the source of truth for
production-binary management commands, plus the direct launcher help described
above for `system build` and `system clean`. There is no generic `./r status`;
use the specific command group.

Core management commands:

```bash
cd <refine-checkout>
./r system status --port <port>
./r system doctor --repo-root .
./r project status
./r project doctor
./r agent detect
./r agent diagnose --provider <provider>
```

Runtime lifecycle commands:

```bash
./r system start --port <port>
./r system stop --port <port>
./r system restart --port <port>
./r system repair --port <port>
./r system update
```

Use `--runtime-root run` only as compatibility syntax for this checkout's
canonical `run` directory. Any other relative value, or an absolute path that
is not the exact canonical runtime of the invoked product home, is rejected.

Target app commands:

```bash
./r project attach /path/to/app
./r project switch <registered-project>
./r project detach
./r project register <name> /path/to/app
./r project clone <remote-url> /path/to/app
```

Workflow and Goal commands:

```bash
./r goal create "Describe the product goal"
./r goal list
./r goal show <goal-id>
./r workflow pause
./r workflow resume
```

Distributed/node commands:

```bash
./r node list
./r node settings <node-id>
./r fleet list
./r fleet maintenance
./r fleet distribute [--to <node-id>] [--converge] [--dry-run]
```

Refine publishes durable state automatically on the dedicated `refine/state`
branch without touching application branches. Its live projection and isolated
state worktree live under the target repository's Git common directory as
`refine-live-state/` and `refine-state-worktree/`; do not assume that directory
is `<app>/.git` when the app is a linked worktree. `<app>/.refine` never exists
in the primary target-app worktree. Goal logs under
`refine-live-state/runtime/goals/` are node-local and are not published. The
Target App **Git remote** setting controls both state and Goal-branch
publication and defaults to `origin`. If that remote is unavailable, Refine
still initializes and commits local state; it simply cannot publish it. Use
`project sync` or the Node screen's **Sync state now** action when a state
handoff must happen immediately; `fleet sync` invokes the same shared
capability for the current node. Manual sync is queued in a supervised runner
process, and the UI reports its progress and any terminal error without
blocking the daemon.

### Refine-owned durable state

Beyond `refine-live-state/` and `refine-state-worktree/`, Refine keeps these
artifacts under the target repository's Git common directory and runtime root.
Each is normal to encounter; only the first is safe to delete by hand.

- `refine-integration/target` (in the Git common directory): a permanent
  detached worktree where integration porcelain runs, so the shared human
  checkout never has to be clean for an integration. It is invisible to
  `git status`, appears in `git worktree list` locked with reason
  "refine integration workspace", and persists between integrations because
  recreating a large checkout is expensive. Safe to purge when Refine is not
  integrating: it self-recreates on the next integration.
- `refine-checkout-sync-pending.json` (in the Git common directory): records a
  checkout sync that a working-tree collision (or other sync failure) skipped.
  While it exists, the target branch ref already points at the integrated
  commit but the checkout's index and files still hold the pre-integration
  content, so the checkout shows the integration delta as staged-reverse —
  this file is the explanation. Do not delete it: it encodes the unapplied ref
  delta, and Refine retries the sync and clears the record once the colliding
  files are committed, stashed, or restored.
- `refine-integrated-target-transaction.json` (in the Git common directory):
  marks an integrated-target transaction in progress so an interrupted one is
  recovered on the next pass. Do not delete it: it encodes which Goal owns the
  interruption. Recovery recreates the Refine-owned integration worktree
  instead of quarantining its residue, replays any pending checkout sync, and
  appends what it did to `refine-integrated-target-recoveries.jsonl` alongside
  it; only legacy shared-checkout markers still quarantine residue to a stash.
- `refine-live-state/runtime/scheduler-holds.jsonl` (node-local, never
  published): one JSONL line per change in why a Goal was excluded from a
  scheduling pass (and when the hold cleared). Read it to answer "why isn't
  this Goal scheduling".

Worker machine creation is agent-operated rather than part of the Refine
binary. Follow `docs/runbooks/manage-fleet.md` when a fleet needs another
worker.

## Operating Refine after install

If you are an agent operating Refine for a user (not just installing it),
three entry points make the surface self-navigating — prefer them over
reading source code:

- `./r next` — recommends the next operations from current project and fleet
  state, each with the exact command. Call it whenever you are deciding what
  to do next.
- `./r commands` — machine-readable JSON catalog of supported user-facing
  production-binary commands with descriptions. Load once instead of
  exploring `--help` per subcommand.
- `docs/runbooks/` — task-oriented guides (manage the fleet, distribute and
  converge work) with preconditions, user questions, verification, and undo
  steps.

When a command fails, report the exact command, exit code, stdout/stderr summary, and any relevant log path. Prefer CLI evidence over guessing from browser state.

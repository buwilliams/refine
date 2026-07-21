# Install Refine

Refine is an agentic software delivery system that runs locally against a user's application repository. It coordinates agents and humans through Goals, workflow state, provider CLIs, local processes, and a browser UI so software changes can move from request to implementation to human review.

Use this document when an agent is responsible for installing Refine. Follow the steps in order, ask the user only the questions needed for the chosen install path, confirm where Refine should be installed when you cannot infer it, and do not claim installation succeeded until the CLI reports a healthy running system or you have reported the exact blocker.

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
2. Check for required tools, identify reachable dependency sources, and install missing dependencies only from a source the user approves. Do not use `scripts/install.sh`; the agent should make dependency choices explicitly.

```bash
curl --version
git --version
cc --version
cargo --version
```

3. If Refine is already installed, update the checkout through the CLI and skip the fresh clone:

```bash
cd <refine-checkout>
./r system update --yes
```

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

5. Always compile the host-local release binary and mark the checkout as deployed. Do this after either `./r system update` or a fresh clone so `./r` uses `bin/refine` instead of running through Cargo:

```bash
cd <refine-checkout>
cargo build --release --locked
mkdir -p bin
install -m 755 target/release/refine bin/refine
printf 'mode=deployed\nrelease_bin=bin/refine\n' > .refine-deployed
```

6. Configure the selected provider:

```bash
cd <refine-checkout>
./r agent configure --provider <provider>
./r agent detect
```

7. If the selected provider CLI is missing, install or authenticate it only after the user approves. Treat Refine installation and provider readiness separately.
8. Use the matching provider auth command when the user approves auth now:

```bash
claude
codex login
gemini auth login
copilot login
```

Do not offer `smoke-ai` during installation. It is reserved for deterministic tests.

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

Use `./r --help` and `./r <group> --help` as the source of truth for management commands. There is no generic `./r status`; use the specific command group.

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
./r system update --yes
```

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
./r node settings
./r cluster list
./r cluster maintenance
./r cluster distribute [--to <node-id>] [--converge] [--dry-run]
```

Refine publishes durable state automatically on the dedicated `refine/state`
branch without touching application branches. Its live local projection and
isolated branch worktree live under the target repository's `.git/` directory,
so `<app>/.refine` never exists in the primary target-app worktree. The Target App **Git remote**
setting controls both state and Goal-branch publication and defaults to
`origin`. If that remote is unavailable, Refine still initializes and commits
local state; it simply cannot publish it. Use `project sync` or the Node screen's
**Sync state now** action when a state handoff must happen immediately;
`cluster sync` invokes the same shared capability for the current node. Manual
sync is queued in a supervised runner process, and the UI reports its progress
and any terminal error without blocking the daemon.

Cloud worker creation is provider-operated rather than part of the Refine
binary. Follow `docs/runbooks/provision.md` when a fleet needs another worker.

## Operating Refine after install

If you are an agent operating Refine for a user (not just installing it),
three entry points make the surface self-navigating — prefer them over
reading source code:

- `./r next` — recommends the next operations from current project and fleet
  state, each with the exact command. Call it whenever you are deciding what
  to do next.
- `./r commands` — machine-readable JSON catalog of every CLI command with
  descriptions. Load once instead of exploring `--help` per subcommand.
- `docs/runbooks/` — task-oriented guides (provision a fleet worker,
  distribute and converge work) with preconditions, user questions,
  verification, and undo steps.

When a command fails, report the exact command, exit code, stdout/stderr summary, and any relevant log path. Prefer CLI evidence over guessing from browser state.

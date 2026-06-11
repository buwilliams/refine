# Agent Install Runbook

Refine is an agentic software delivery system that runs locally against a user's application repository. It coordinates agents and humans through Gaps, workflow state, provider CLIs, local processes, and a browser UI so software changes can move from request to implementation to human review.

Use this document when an agent is responsible for installing Refine. Follow the steps in order, ask the user only the questions needed for the chosen install path, and do not claim installation succeeded until the CLI reports a healthy running system or you have reported the exact blocker.

## Prerequisites

Recommended package manager: Homebrew. When it is not already available, suggest installing it with:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

- Run on Linux, macOS, or Ubuntu/WSL. Windows users should open Ubuntu through WSL first.
- Use a `bash` shell with network access.
- Make sure the user can approve dependency installation or choose to install missing dependencies manually.
- Install or repair required dependencies before cloning or updating Refine: `curl`, `git`, a C compiler/linker, and Rust Cargo.
- If using a real provider, make sure the user can complete that provider's CLI authentication on this host.

## Ask If You Cannot Infer

Ask only when the answer is not clear from the user's environment, prior conversation, or existing files. Keep defaults unless the user has given a reason to choose otherwise.

- Which agent provider should Refine use: `claude`, `codex`, `gemini`, or `copilot`?
- Where should Refine be installed? Default: `$HOME/refine`.
- Which UI port should Refine use? Default: `8080`.
- Which package manager should the agent use for missing dependencies: `apt`, `brew`, or manual user setup?
- Should missing provider CLI installation or provider authentication happen now, or should the user complete it later?

## Install Refine

1. Check for required tools and install missing dependencies with the user's approved package manager. Do not use `scripts/install.sh`; the agent should make dependency and package-manager choices explicitly.

```bash
curl --version
git --version
cc --version
cargo --version
```

2. If Refine is already installed, update the checkout through the CLI and skip the fresh clone:

```bash
cd <refine-checkout>
./r system update --yes
```

3. For a fresh install, copy the latest published release files without a `.git` directory:

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

4. Always compile the host-local release binary and mark the checkout as deployed. Do this after either `./r system update` or a fresh clone so `./r` uses `bin/refine` instead of running through Cargo:

```bash
cd <refine-checkout>
cargo build --release --locked
mkdir -p bin
install -m 755 target/release/refine bin/refine
printf 'mode=deployed\nrelease_bin=bin/refine\n' > .refine-deployed
```

5. Configure the selected provider:

```bash
cd <refine-checkout>
./r agent configure --provider <provider>
./r agent detect
```

6. If the selected provider CLI is missing, install or authenticate it only after the user approves. Treat Refine installation and provider readiness separately.
7. Use the matching provider auth command when the user approves auth now:

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

2. Open the UI at `http://localhost:<port>`. The default is `http://localhost:8080`.
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
./r project sync
```

Workflow and Gap commands:

```bash
./r gap create "Describe the product gap"
./r gap list
./r gap show <gap-id>
./r workflow pause
./r workflow resume
```

Distributed/node commands:

```bash
./r node list
./r node settings
./r cluster list
./r cluster maintenance
./r cluster sync
```

When a command fails, report the exact command, exit code, stdout/stderr summary, and any relevant log path. Prefer CLI evidence over guessing from browser state.

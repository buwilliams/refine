# Agent Install Runbook

Refine is an agentic software delivery system that runs locally against a user's application repository. It coordinates agents and humans through Gaps, workflow state, provider CLIs, local processes, and a browser UI so software changes can move from request to implementation to human review.

Use this document when an agent is responsible for installing Refine. Follow the steps in order, ask the user only the questions needed for the chosen install path, and do not claim installation succeeded until the CLI reports a healthy running system or you have reported the exact blocker.

## Prerequisites

- Run on Linux, macOS, or Ubuntu/WSL. Windows users should open Ubuntu through WSL first.
- Use a `bash` shell with network access.
- Make sure `curl` is available to download the installer.
- Make sure the user can approve dependency installation or choose to install missing dependencies manually. The installer checks and guides the rest.
- If using a real provider, make sure the user can complete that provider's CLI authentication on this host.

## User Questions

Ask only the questions needed for the install path. Useful questions:

- Which agent provider should Refine use: `claude`, `codex`, `gemini`, `copilot`, or `smoke-ai`?
- Should provider CLI installation and auth happen now, or should provider auth be completed later?
- Where should Refine be installed? Default: `$HOME/refine`.
- Which UI port should Refine use? Default: `8080`.
- Should missing host dependencies be installed with Homebrew when the installer offers that path, or should the user install dependencies manually?
- Should Playwright Chromium be installed or repaired for regression screenshots?
- Does the target app need Docker or container support?

## Install Refine

1. Run the installer:

```bash
curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash
```

2. Answer installer prompts using the user's choices. If the installer fails, preserve the install log path it printed, report the missing dependency or failing command, and stop.
3. If the installer asks for a manual build, run it from the selected checkout:

```bash
cd <refine-checkout>
cargo build --release --locked
install -m 755 target/release/refine bin/refine
```

4. Configure the selected provider:

```bash
cd <refine-checkout>
./r agent configure --provider <provider>
./r agent detect
```

5. If the selected provider CLI is missing, install or authenticate it only after the user approves. Treat Refine installation and provider readiness separately.
6. Use the matching provider auth command when the user approves auth now:

```bash
claude
codex login
gemini auth login
copilot login
```

For Smoke AI, make the `smoke-ai` executable available on `PATH` before running provider checks.

## After Install

1. Start and check Refine:

```bash
cd <refine-checkout>
./r system start --port <port>
./r system status --port <port>
./r system doctor --repo-root .
```

2. Open the UI at `http://localhost:<port>`. The default is `http://localhost:8080`.
3. Ask the user what app Refine should target: an existing local repo, a git remote to clone, a new app to create, or no target app yet.
4. Attach or create the target app with the matching command:

```bash
./r project attach /path/to/app
./r project clone <remote-url> /path/to/app --make-current
```

5. If creating a new app, run the user-approved starter command in the new app directory, make the initial git commit, then attach the app.
6. Finish with the Refine checkout path, UI URL, selected provider, target app status, install log path, and summaries from `./r system status` and `./r system doctor`.

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
./r system update
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
./r workflow allowed <gap-id>
./r workflow schedule
./r workflow pause
./r workflow resume
./r workflow restore
./r workflow enforce
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

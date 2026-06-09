# Agent Install Runbook

Use this document when an agent is responsible for installing Refine. Follow the steps in order, ask the user only the questions needed for the chosen install path, and do not claim installation succeeded until verification passes or you have reported the exact blocker.

## Prerequisites

- Operating system: Linux, macOS, or Ubuntu/WSL. Windows users should open Ubuntu through WSL first.
- Network access: required for package installs, release lookup, clone, and provider setup.
- Shell: `bash`.
- Download tools: `curl` and `git`.
- Build tools: a C compiler/linker (`cc` and `ld`) and Rust Cargo.
- Optional JavaScript tools: Node.js 18+ and npm, needed when installing npm-based provider CLIs or Playwright.
- Optional provider CLI: one of `claude`, `codex`, `gemini`, `copilot`, or `smoke-ai`.
- Optional browser automation: Playwright Chromium, needed for managed regression screenshots.
- Optional containers: Docker, needed only for target apps that use containers.
- Debian/Ubuntu install option: `sudo apt-get update && sudo apt-get install -y curl git build-essential`.
- Debian/Ubuntu Rust option: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`.
- Debian/Ubuntu Node option: install Node.js 18+ from your preferred Node distribution or package source.
- macOS Homebrew install option: `/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"`.
- macOS Homebrew deps option: `brew install git rust llvm node`.
- macOS Apple tools option: `xcode-select --install` for compiler/linker support.

## User Questions

Ask only the questions needed for the install path. Useful questions:

- Which agent provider should Refine use: `claude`, `codex`, `gemini`, `copilot`, or `smoke-ai`?
- Should provider CLI installation and auth happen now, or should provider auth be completed later?
- Where should Refine be installed? Default: `$HOME/refine`.
- Which UI port should Refine use? Default: `8080`.
- Should missing host dependencies be installed with Homebrew when the installer offers that path, or should the user install dependencies manually?
- Should Playwright Chromium be installed or repaired for regression screenshots?
- Does the target app need Docker or container support?

## Download

Set the install inputs first:

```bash
export REFINE_INSTALL_PROVIDER=codex
export REFINE_INSTALL_CHECKOUT_DEFAULT="$HOME/refine"
export REFINE_INSTALL_PORT=8080
export REFINE_DEFAULT_PORT=8080
export REFINE_INSTALL_LOG=/tmp/refine-install.log
```

Download and run the installer in build/repair mode first:

```bash
curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh \
  | REFINE_INSTALL_UPDATE_ONLY=1 bash -s -- --yes
```

If this step fails, preserve `REFINE_INSTALL_LOG`, report the missing dependency or failing command, and stop.

## Build

Verify that the checkout and installed binary exist:

```bash
REFINE_CHECKOUT="${REFINE_INSTALL_CHECKOUT_DEFAULT:-$HOME/refine}"
cd "$REFINE_CHECKOUT"
test -x bin/refine
test -f .refine-deployed
```

Run the build again manually only if the installer asked for it:

```bash
cd "$REFINE_CHECKOUT"
cargo build --release --locked
install -m 755 target/release/refine bin/refine
```

## Configure

Configure the selected provider:

```bash
cd "$REFINE_CHECKOUT"
./r agent configure --provider "$REFINE_INSTALL_PROVIDER"
./r agent detect
```

If the selected provider CLI is missing, install or authenticate it only after the user approves. Treat Refine installation and provider readiness separately.

Provider auth commands:

```bash
claude
codex login
gemini auth login
copilot login
```

For Smoke AI, set `REFINE_SMOKE_AI_PATH` to the executable path before running provider checks.

## CLI Management

Use `./r --help` and `./r <group> --help` as the source of truth for management commands. There is no generic `./r status`; use the specific command group.

Core management commands:

```bash
cd "$REFINE_CHECKOUT"
./r system status --port "$REFINE_PORT"
./r system doctor --repo-root .
./r project status
./r project doctor
./r agent detect
./r agent diagnose --provider "$REFINE_INSTALL_PROVIDER"
```

Runtime lifecycle commands:

```bash
./r system start --port "$REFINE_PORT"
./r system stop --port "$REFINE_PORT"
./r system restart --port "$REFINE_PORT"
./r system repair --port "$REFINE_PORT"
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

## Start

Start Refine:

```bash
REFINE_PORT="${REFINE_INSTALL_PORT:-${REFINE_DEFAULT_PORT:-8080}}"
cd "$REFINE_CHECKOUT"
./r system start --port "$REFINE_PORT"
./r system status --port "$REFINE_PORT"
./r system doctor --repo-root .
```

Open the UI after `system status` succeeds:

```text
http://localhost:<REFINE_PORT>
```

For the default install, use:

```text
http://localhost:8080
```

## Target App

Ask the user which app Refine should target. Possible questions:

- Do you want to attach an existing local git repo?
- Do you want Refine to clone a git remote and attach it?
- Do you want to create a new app first, then attach it?
- Do you want to continue without a target app and use the browser Guide later?
- If attaching an existing repo, what is the local path?
- If cloning a remote, what is the remote URL and destination path?
- If creating a new app, what path should be used?
- If creating a new app, what starter command or framework should initialize it?
- Does the target app need Docker or other host services before Refine starts working on it?

Use the matching path:

- Attach an existing local git repo.
- Clone and attach a git remote.
- Create a new app, then attach it.
- Continue without a target app and let the browser Guide handle setup later.

For an existing local git repo:

```bash
export REFINE_INSTALL_TARGET_APP=/path/to/app
cd "$REFINE_CHECKOUT"
./r project attach "$REFINE_INSTALL_TARGET_APP"
./r project status
```

For a git remote:

```bash
git clone <remote-url> /path/to/app
export REFINE_INSTALL_TARGET_APP=/path/to/app
cd "$REFINE_CHECKOUT"
./r project attach "$REFINE_INSTALL_TARGET_APP"
./r project status
```

For a new app:

```bash
mkdir -p /path/to/app
cd /path/to/app
git init
# Run the user-approved starter command here.
git add .
git commit -m "Initial app"

cd "$REFINE_CHECKOUT"
./r project attach /path/to/app
./r project status
```

Finish with a concise report:

- Refine checkout path.
- UI URL.
- Selected provider token.
- Target app path, git remote, new app path, or `not attached`.
- Install log path.
- Output summary from `./r system status`.
- Output summary from `./r system doctor`.
- Any follow-up items printed by the installer.

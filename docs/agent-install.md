# Agent Install Runbook

Use this document when an agent is responsible for installing Refine. Follow the commands as written, report evidence, and do not claim installation succeeded until the verification commands pass or you have reported the exact blocker.

## Preconditions

- Run on Linux, macOS, or Ubuntu/WSL.
- Use a shell with network access.
- Make sure `curl` and `git` are available.
- Make sure you are allowed to install or repair host dependencies when prompted.
- If using a real provider, make sure the provider CLI and auth can be completed on this host.

Windows agents should open Ubuntu through WSL first, then follow the Linux path.

## Choose Inputs

Set these values before running the installer:

- `REFINE_INSTALL_PROVIDER`: one of `claude`, `codex`, `gemini`, `copilot`, or `smoke-ai`.
- `REFINE_INSTALL_TARGET_APP`: a local git repo path, a git remote URL, or an empty value to install Refine without attaching an app.
- `REFINE_INSTALL_CHECKOUT_DEFAULT`: where Refine should be installed. Defaults to `$HOME/refine`.
- `REFINE_INSTALL_PORT`: UI port. Defaults to `8080`.
- `REFINE_INSTALL_LOG`: install log path. Defaults to `/tmp/refine-install-<pid>.log`.

OpenClaw can follow this runbook as the installing agent, but the current Refine installer does not accept `REFINE_INSTALL_PROVIDER=openclaw`. Choose a supported provider token or leave provider setup for the user.

## Install

Interactive install:

```bash
export REFINE_INSTALL_PROVIDER=codex
export REFINE_INSTALL_TARGET_APP=/path/to/app
export REFINE_INSTALL_LOG=/tmp/refine-install.log

curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash
```

Unattended install with defaults:

```bash
export REFINE_INSTALL_PROVIDER=codex
export REFINE_INSTALL_TARGET_APP=/path/to/app
export REFINE_INSTALL_LOG=/tmp/refine-install.log

curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash -s -- --yes
```

Install without attaching a target app:

```bash
export REFINE_INSTALL_PROVIDER=codex
export REFINE_INSTALL_TARGET_APP=
export REFINE_INSTALL_LOG=/tmp/refine-install.log

curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash -s -- --yes
```

Install on a non-default port:

```bash
export REFINE_INSTALL_PROVIDER=codex
export REFINE_INSTALL_TARGET_APP=/path/to/app
export REFINE_INSTALL_PORT=19080
export REFINE_DEFAULT_PORT=19080
export REFINE_INSTALL_LOG=/tmp/refine-install.log

curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash -s -- --yes
```

## Verify

Run verification from the Refine checkout:

```bash
REFINE_CHECKOUT="${REFINE_INSTALL_CHECKOUT_DEFAULT:-$HOME/refine}"
REFINE_PORT="${REFINE_INSTALL_PORT:-${REFINE_DEFAULT_PORT:-8080}}"

cd "$REFINE_CHECKOUT"
./r system status --port "$REFINE_PORT" --runtime-root run
./r system doctor --runtime-root run --repo-root .
./r agent detect
```

Open the UI after `system status` succeeds:

```text
http://localhost:<REFINE_PORT>
```

For the default install, use:

```text
http://localhost:8080
```

## Report

Finish with a concise report containing:

- Refine checkout path.
- UI URL.
- Selected provider token.
- Target app path, git remote, or `not attached`.
- Install log path.
- Output summary from `./r system status`.
- Output summary from `./r system doctor`.
- Any follow-up items printed by the installer.

## Failure Handling

If the installer fails:

- Do not retry blindly.
- Preserve the install log and the first failing command.
- Report the dependency, permission, network, or provider-auth error shown by the installer.
- After fixing the specific issue, rerun the same installer command.

If provider auth fails:

- Treat Refine installation and provider readiness separately.
- Report that Refine is installed only if `system status` and `system doctor` pass.
- Report that agent workflows still need the relevant provider login or auth command before execution.

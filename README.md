# Refine – Your Team's Agentic Software Delivery System

<img src="src/surfaces/web/static/images/refine_logo_transparent.png" alt="refine" style="width: 100%; height: auto;">

refine is an agentic software delivery system that coordinates people and agents across distributed machines. Product, support, and customers — anyone who can articulate what the app does today vs what it should do instead - can make meaningful contributions to your software. Build new apps, implement features, and fix bugs while keeping feedback cheap, local, and repeatable.

- **Empower your team** - extend product feedback and app edits to your whole team. Keep planning, features, Gaps, chat, and human verification in one workflow so work moves without losing context.
- **Agent Fleet** - coordinate agents across decentralized repositories, branches, processes, and node clusters. Cheap feedback loops keep agent work moving while git keeps everyone in sync.
- **Governance** - keep agent work aligned with your product intent, local rules, and requirements. Review stays grounded before changes merge.
- **Personal AI** - install refine and manage your node cluster with your favorite agent.

## Intent

The full design intent of Refine is documented at [Intent](docs/intent/README.md). You can learn the most by reading these documents.

## Install with your agent

Use OpenClaw, Codex, Claude Code, Gemini, Copilot, or another coding agent to install Refine:

```bash
Follow instructions found at: https://raw.githubusercontent.com/buwilliams/refine/refs/heads/main/docs/agent-install.md
```

Or have your agent ensure system dependencies are installed:

```bash
Install or repair required system dependencies: curl, git, a C compiler/linker, and Rust Cargo.
```

## Install yourself

Linux, macOS, or Ubuntu/WSL:

```bash
curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash
```

The installer checks the host, installs or repairs missing tools when you approve, and starts Refine. Attach a target application from the browser Guide after startup, or pass `REFINE_INSTALL_TARGET_APP=/path/to/app` to attach one during install.

refine has a robust CLI:

```bash
./r --help
```

### Windows Users

Open PowerShell as Administrator:

```powershell
wsl --install
```

After Ubuntu opens, use the Install yourself one-liner above.

### Ubuntu Dependencies in Corporate Firewall

```bash
sudo apt-get update && sudo apt-get install -y build-essential rustc cargo
```

## Tests

Use `./r test` as the authoritative test entrypoint. The default command runs only the in-crate Rust unit tests; integration suites are opt-in.

```bash
./r test [suite]
```

Suites:

- `unit` - in-crate Rust unit tests. This is the default when no suite is provided.
- `integration` - opt-in CLI, daemon, Docker, and cluster suites.
- `full` - all test suites and repository checks.
- `rust` - Rust unit, integration, and doc tests.
- `smoke-ai` - Smoke AI fixture contract.
- `cli` - daemon-backed CLI integration tests. These are the authoritative surface tests and exercise the same daemon-backed product services used by the web UI.
- `cluster-ssh` - Docker/SSH-backed cluster CLI tests.
- `install-uninstall` - Docker-backed install/uninstall tests.
- `full-workflow` - daemon-backed full workflow test.
- `multi-instance-sync` - multi-instance sync tests.

## License

[MIT](LICENSE) — use it however you like, modify it, ship it, sell it. No warranty, no support obligations on my end. If you build something useful on top, a heads-up is appreciated but not required.

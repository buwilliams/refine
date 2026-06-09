# Refine – Your Team's Agentic Software Delivery System

<img src="src/surfaces/web/static/images/refine_logo_transparent.png" alt="refine" style="width: 100%; height: auto;">

refine is an agentic software delivery system that coordinates people and agents across distributed machines. Product, support, and customers — anyone who can articulate what the app does today vs what it should do instead - can make meaningful contributions to your software. Build new apps, implement features, and fix bugs while keeping feedback cheap, local, and repeatable.

- **Decentralized** - refine runs locally against existing repositories, branches, processes, and infrastructure, while git keeps people in sync across machines.
- **Cheap feedback loops** - Gaps move from report to agent work to human review, so the system improves through fast correction instead of perfect upfront specification.
- **Planning and chat** - people can think with agents before execution, ask questions, and steer Gap-specific follow-up.
- **Quality automation** - Guidance, Governance, and QA shape agent work from planning through merge, keeping automation aligned with product intent, local rules, and requirements.
- **Human verification** - people review the result before merge, preserving ordinary human judgment where it matters.

## Install with your agent

Use OpenClaw, Codex, Claude Code, Gemini, Copilot, or another coding agent to install Refine:

```bash
https://raw.githubusercontent.com/buwilliams/refine/refs/heads/main/docs/agent-install.md
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

## Tests

Use `./r test` as the authoritative test entrypoint. The default command runs only the in-crate Rust unit tests; integration suites are opt-in:

```bash
./r test
./r test --integration
./r test --full
```

The release gate is `./r test`, the unit-only default. Use `./r test --full` when you explicitly want all suites and repository checks: unit tests, Rust doc tests, xtask checks, Rust integration tests, smoke AI contract, daemon-backed CLI surface, Docker/SSH-backed cluster CLI tests, Docker-backed install/uninstall tests, full workflow, multi-instance sync, Playwright UI tests, and `git diff --check`.

Focused suites:

```bash
./r test --rust
./r test --smoke-ai
./r test --cli
./r test --ui
./r test --surface
./r test --cluster-ssh
./r test --install-uninstall
./r test --full-workflow
./r test --multi-instance-sync
```

## License

[MIT](LICENSE) — use it however you like, modify it, ship it, sell it. No warranty, no support obligations on my end. If you build something useful on top, a heads-up is appreciated but not required.

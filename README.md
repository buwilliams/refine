# refine

refine is an agentic software delivery system that coordinates people and agents across distributed machines. Product, support, and customers — anyone who can articulate what the app does today vs what it should do instead - can make meaningful contributions to your software. Build new apps, implement features, and fix bugs while keeping feedback cheap, local, and repeatable.

- **Decentralized** - refine runs locally against existing repositories, branches, processes, and infrastructure, while git keeps people in sync across machines.
- **Cheap feedback loops** - Gaps move from report to agent work to human review, so the system improves through fast correction instead of perfect upfront specification.
- **Planning and chat** - people can think with agents before execution, ask questions, and steer Gap-specific follow-up.
- **Quality automation** - Guidance, Governance, and QA shape agent work from planning through merge, keeping automation aligned with product intent, local rules, and requirements.
- **Human verification** - people review the result before merge, preserving ordinary human judgment where it matters.

## Quick Start

Linux, macOS, or Ubuntu/WSL:

```bash
curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash
```

The installer checks the host, installs or repairs missing tools when you approve, asks which AI provider to use, and starts Refine. Attach a target application from the browser Guide after startup, or set `REFINE_INSTALL_TARGET_APP` for scripted installs.

refine has a robust CLI:

```bash
./r --help
```
(./r is a shell script that executes 'uv run refine [args]' for convenience)

For development of Refine, there is an independent app used to test Refine surfaces (UI and CLI) at [buwilliams/refine-test](https://github.com/buwilliams/refine-test).

### Windows Users

Open PowerShell as Administrator:

```powershell
wsl --install
```

After Ubuntu opens, use the Quick Start one-liner above.

## License

[MIT](LICENSE) — use it however you like, modify it, ship it, sell it. No warranty, no support obligations on my end. If you build something useful on top, a heads-up is appreciated but not required.

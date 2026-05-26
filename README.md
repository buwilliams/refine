# refine

refine is an agentic software delivery system that builds new apps, implements features, and fixes bugs. It coordinates people and agents across distributed machines while keeping feedback cheap, local, and repeatable. Product, support, and customers — anyone who can articulate what the app does today vs what it should do instead - can make meaningful contributions to your software.

- **Decentralized** - each instance owns its queue and data locally, while git keeps people in sync across machines.
- **Cheap feedback loops** - Gaps move from report to agent work to human review, so the system improves through fast correction instead of perfect upfront specification.
- **Planning and chat** - people can think with agents before execution, ask questions, and steer Gap-specific follow-up.
- **Quality automation** - Guidance, Governance, and QA shape agent work from planning through merge, keeping automation aligned with product intent, local rules, and requirements.
- **Human verification** - people review the result before merge, preserving ordinary human judgment where it matters.
- **Works with you** - refine fits existing repositories, branches, processes, and infrastructure, so teams can use it alongside current development practices.

## Quick Start

Linux, macOS, or Ubuntu/WSL:

```bash
curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash
```

The installer checks the host, installs or repairs missing tools when you approve, asks which AI provider to use, optionally clones or attaches the target application, and starts Refine.

refine has a robust CLI:

```bash
uv run refine --help
```

### Windows Users

Open PowerShell as Administrator:

```powershell
wsl --install
```

After Ubuntu opens, use the Quick Start one-liner above.

## License

[MIT](LICENSE) — use it however you like, modify it, ship it, sell it. No warranty, no support obligations on my end. If you build something useful on top, a heads-up is appreciated but not required.

# refine

Turn software gaps: new apps, features, and bugs into verified software written by
orchestrated AI agents and verified by ordinary people. QA, Product, support,
customers — anyone who can articulate *what the app does today* vs
*what it should do instead* — submits a Gap.

## Why?

refine is a decentralized agentic system that runs on your existing development machines, infrastructure, and processes. It creates economic advantage by shrinking the time and coordination cost between identifying a problem and shipping a verified change.

Full-control engineering is expensive: reading every line, modeling every dependency, and organizing code as if the system can be made "perfect" still does not prevent outages or security breaches. Reliability often increases the cost of change, and many engineering cultures optimize for correctness at the expense of the businesses they serve.

refine puts automation at the forefront, reduces feedback loops by orders of magnitude, and lets companies onboard without new infrastructure or new processes. The architecture is economics first and technical accuracy second: the same principles of high-quality software and rapid development, reordered so businesses can stay competitive. Instead of trying to make every decision correct up front, refine makes feedback cheap enough that software can become accurate over time as the business learns and changes.

## Features

- **Decentralized** - each instance owns its work locally and syncs through git.
- **Existing Infrastructure** - works inside existing applications, repositories, branches, and development practices.
- **Dashboard** - consolidated space for run status, reporter stats, and work needing attention.
- **Gaps** - searchable, filterable backlog with sorting and bulk updates.
- **Changes** - searchable history of merged Gap work with undo controls.
- **Instances** - separate work contexts with their own Gap queues.
- **Chat** - persistent dock for standalone questions and Gap-specific follow-up.
- **Workflow** - move Gaps from report to agent work to human review.
- **Guidance** - reusable instructions classified against each Gap before work starts.
- **Governance** - Product, Constitution, and Rules checks before work starts.
- **Merge** - integrate completed Gap branches through a single serialized agent.
- **Logs** - filtered audit trail for agent output, git events, and system activity.

## Quick Start

```bash
git clone https://github.com/buwilliams/refine.git <refine-checkout> && cd <refine-checkout> && uv run refine start
```

### Prerequisites

- Git
- Python
- uv, a replacement for pip
- OS: Linux or Windows/WSL2 (systemd and systemd-run for process management), or macOS (launchctl for process management)

Use `uv run refine install [port]` for a persistent system service that runs as the installing user and may prompt for sudo; `start [port]` runs a non-installed background process.

## Workflow

1. A person adds a Gap; it starts in the Backlog.
2. A person or automation moves the Gap to the Todo list when it is ready for work.
3. Guidance adds matched instructions; Governance validates the request.
4. AI agents work Todo Gaps in parallel.
5. People review the result; if it misses the target, they submit another Round.
6. Approval closes the Gap.

## Mental Model

- refine is a development tool installed on a dev or QA machine.
- Use it as a solo contributor or open it up to a group.
- Each machine has its own instance, configured manually.
- refine supports multiple layers: processes on one host at different ports, targeted applications as data boundaries, and device instances on the same targeted application.
- All data is owned by an instance and synced through git.
- Governance and Guidance are global across all instances.

![refine architecture](refine_ui/static/images/refine-architecture.svg)

## License

[MIT](LICENSE) — use it however you like, modify it, ship it, sell it. No
warranty, no support obligations on my end. If you build something useful
on top, a heads-up is appreciated but not required.

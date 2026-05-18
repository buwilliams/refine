# refine

Turn software gaps (features and bugs) into verified software
through ordinary people enhanced by agents. QA, Product, support,
customers — anyone who can articulate *what the app does today* vs
*what it should do instead* — submits a Gap.

- **Dashboard** - consolidated space for run status, reporter stats, and work needing attention.
- **Gaps** - searchable, filterable backlog with sorting and bulk updates.
- **Chat** - persistent dock for standalone questions and Gap-specific follow-up.
- **Workflow** - move Gaps from report to agent work to human review.
- **Governance** - Product, Constitution, and Rules checks before work starts.
- **Merge** - integrate completed Gap branches through a single serialized agent.
- **Logs** - filtered audit trail for agent output, git events, and system activity.

## Quick Start

```bash
git clone https://github.com/buwilliams/refine.git <refine-checkout>
cd <refine-checkout>
uv run refine start
```

## Layout

```
refine/
├── refine_cli/           # the `refine` CLI: init, start, stop, status, server, ui, doctor
├── refine_server/        # server logic, storage, config, subprocesses, git, gap.json owner
├── refine_ui/            # host-native UI backend + static HTML/JS
├── pyproject.toml        # makes `refine` a real console script
└── spec.md               # the design document
```

## Operational assumptions

- The host running refine is dedicated to refine — no human edits the client
  repo's working copy directly; Refine owns local commits on that checkout.
- The client's developers push from their own machines; refine sees those
  commits via `fetch` and folds them in through the Merge agent while Gaps are
  in `ready-merge`.

## License

[MIT](LICENSE) — use it however you like, modify it, ship it, sell it. No
warranty, no support obligations on my end. If you build something useful
on top, a heads-up is appreciated but not required.

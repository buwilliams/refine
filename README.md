# refine

Turn software gaps (features and bugs) into verified software
through ordinary people enhanced by agents. QA, Product, support,
customers — anyone who can articulate *what the app does today* vs
*what it should do instead* — submits a Gap. Refine runs the configured host
agent CLI in git worktrees, then keeps the result in review until a human
verifies it.

Refine handles the git plumbing — worktrees, fetch, merge, push,
auto-committing its own state — and inherits the selected agent CLI's host
auth, so operators rarely need to think about either.

- Dashboard for run status, reporter stats, and work needing attention.
- Gaps list with search, filters, sorting, and bulk updates.
- Logs view with filters for agent output, git events, and system activity.
- Persistent Chat dock for standalone questions and Gap-specific follow-up.
- Import-from-text flow that extracts Gap drafts from free-form notes.
- Gap Governance for Product, Constitution, and Rules checks before work starts.
- Multi-app setup and switching from System → Application.
- Host-native operation that reuses local agent auth, SSH keys, git config, and PATH.

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

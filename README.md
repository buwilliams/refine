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
- Multi-app setup and switching from Settings → Project.
- Host-native operation that reuses local agent auth, SSH keys, git config, and PATH.

## Quick Start

```bash
git clone https://github.com/buwilliams/refine.git <refine-checkout>
claude login                       # or: codex login / gemini auth login
cd <refine-checkout>
uv run refine init <target-app>
cd <target-app>
git add .refine/refine.toml .refine/.gitignore
git commit -m "add refine"
cd <refine-checkout>
uv run refine start
uv run refine status               # exact unit name and log command
# open http://localhost:8080
# optional: loginctl enable-linger $USER
# switch apps: uv run refine init <other-target-app> --force
# reset binding: uv run refine reset
# purge active app refine data: uv run refine reset --purge -y
```

If you skip `refine init` and run `uv run refine start` in a fresh checkout,
refine serves a setup UI where you can create or attach the first target app.

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
  repo's working copy directly; all local commits come from refine agents.
- The client's developers push from their own machines; refine sees those
  commits via `fetch` and folds them in during `verify`.

## License

[MIT](LICENSE) — use it however you like, modify it, ship it, sell it. No
warranty, no support obligations on my end. If you build something useful
on top, a heads-up is appreciated but not required.

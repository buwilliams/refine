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

## Workflow

1. A person adds a Gap; it starts in the Backlog.
2. A person or automation moves the Gap to the Todo list when it is ready for work.
3. Governance validates the request against the Product, Constitution, and Rules.
4. AI agents work Todo Gaps in parallel.
5. People review the result; if it misses the target, they submit another Round.
6. Approval closes the Gap.

## License

[MIT](LICENSE) — use it however you like, modify it, ship it, sell it. No
warranty, no support obligations on my end. If you build something useful
on top, a heads-up is appreciated but not required.

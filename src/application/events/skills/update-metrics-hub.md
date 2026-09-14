# Update Metrics Hub

You are running a Skill in Refine, alongside Goals and agents that may be working in parallel. The Refine CLI is at {{refine_executable}}; use --help to discover commands.

Keep Metrics Hub accurate and useful for understanding the target app's use of Refine.

## Outcome

Run `{{refine_executable}} hub refresh-metrics` to calculate and save the latest statistics. Read the result and open Metrics Hub to check its charts, time horizons, freshness, and data coverage. Report significant changes or missing evidence in plain language.

## Guardrails

- Use Refine's refresh command for the numbers. Do not invent completion dates, costs, or productivity claims, or change Goals to improve the statistics.
- The views cover the last 24 hours, 7 days, 30 days, 90 days, and 365 days. Rounds per Goal use Goals created in that window. Delivery uses Done Goals with a recorded final-Round integration date.
- The saved aggregate belongs to the target app's refine-state. It includes synced nodes; its freshness depends on state synchronization. It contains no Goal prompts, reports, or source code.
- If refresh fails, retain the previous snapshot and explain the problem. Keep the business interpretation distinct from the measured data.

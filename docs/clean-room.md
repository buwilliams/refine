# Clean-Room Agent Baseline

## Purpose

Refine's primary operator is an AI agent, but the people building Refine can
never experience it the way users do: a builder's agent carries context from
the source and its own development history. The clean-room baseline measures
what a **context-free agent** — the agent a user actually has — can accomplish
with Refine, and what it costs them.

It exists to answer one question with evidence instead of intuition: *is
Refine agent-first friendly yet?* Every stall the clean-room agent hits is a
discoverability goal to file; the baseline re-run is the acceptance test for
fixing them.

## What the clean room removes — and what it deliberately keeps

The clean room removes exactly one thing: **conversation context**. The test
agent is a fresh session that has never seen this repository discussed,
designed, or debugged.

It keeps the **full, faithful install**, source included. Refine installs as
a source checkout, so a user's agent can and will read `src/` when the
product fails to explain itself. Deleting the source would test an
aspiration, not the experience — and would report failures users never hit
(they'd have dug through the source) while hiding the failure mode they do
hit (getting there slowly and expensively). Source access is not forbidden;
it is **measured**.

## Setup

1. Produce a realistic install in a throwaway directory: clone/copy the
   checkout, `cargo build --release`, install to `bin/refine`, write the
   `.refine-deployed` marker — what `scripts/install.sh` produces.
2. Create a small toy target project (a real git repo) alongside it.
3. Put a **fake `fly` shim** first on `PATH`: a script that logs every
   invocation to a file and returns plausible canned JSON. The agent can
   drive cloud scenarios end to end with zero spend, and the log records
   what it *would have done* to the user's account.
4. Launch one **fresh agent per scenario** — no shared context between
   scenarios. The agent prompt contains a persona, a goal, the constraint
   budget, and stall-logging rules; nothing else.

## Scenario design

Scenarios are worded in **user vocabulary, not product vocabulary**. If a
scenario says "provision a cluster node," the command names have leaked and
the test is void. Describe outcomes the user wants; let the agent find the
vocabulary.

Baseline scenario set:

1. **Orientation** — "Your user installed this tool at `<path>`. Figure out
   what it does, start it, and give them the URL."
2. **Work capture** — "Here is a feature description for their project. Get
   it tracked in Refine as separate pieces of work."
3. **Cloud worker** — "Your user wants some of this work done on a cloud
   machine on their Fly.io account instead of their laptop. Make that
   happen."
4. **Convergence** — "Work finished on the cloud machine. Get it back in
   front of your user for review."
5. **Diagnosis** — "The user says the cloud machine 'seems broken.' Find out
   what's wrong and explain it."

Each agent gets a tool-call budget (default 40) so wandering shows up as a
number, and instructions to narrate: log `STALL: <what I expected to exist>`
when a route dead-ends, `BLOCKED` if giving up. The stall text is the most
valuable output — it is users' agents telling us, in their own words, which
affordances they assumed.

## Scoring

Grading is mechanical, never the grader's product knowledge.

**Completion** is verified against artifacts, not the agent's claims:
state files (`nodes.json`, goal JSON, health), daemon responses, and the fly
shim log (did it run `apps create` + `deploy` with sane flags?).

**Cost and route** come from the transcript:

- tool calls used, bytes read, failed commands (hallucinated flags or
  subcommands, non-zero exits) before the goal
- `--help` invocations (the cheap path) vs reads/greps into `src/` (the
  expensive path)
- STALL count, with each stall's expected-affordance text

**Two-tier pass bar:**

| Tier | Meaning | Pass condition |
|---|---|---|
| Realistic UX | what users experience today | all scenarios complete within budget, zero BLOCKED |
| Design bar | what `docs/intent/01-design.md` promises | scenarios complete with **zero source reads** |

The per-scenario goal between tiers quantifies where discoverability is
currently subsidized by source-readability. The design bar is a target
state, not an enforced condition — source reads trend to zero because the
product explains itself, never because reading was banned.

## Protocol notes

- Run each scenario 2–3 times; a single agent run is an anecdote, not a
  measurement.
- The baseline agent shares the builder's underlying model, so this measures
  *context* deprivation, not harness diversity. Runs under other harnesses
  (Codex, Gemini) are separate columns of the same scorecard, not the same
  test.
- The cloud scenario with a shim validates *discoverability*, not cloud
  behavior; the real-provider procedure lives in
  `docs/runbooks/provision.md`.
- An ablated no-source variant may be run once as a diagnostic — to learn
  which scenarios are passable *only* via source — but the product is never
  graded on it.

## Outputs

Each run produces a findings report: per-scenario outcome, cost metrics,
route (docs vs source), and a ranked stall list where every stall maps to a
concrete Goal ("agent looked for `refine deploy`; never found the provisioning
runbook" → naming/help-text Goal). Fixes are then implemented and
the same scenarios re-run; the loop closes when the realistic tier passes
cheaply and the design tier passes at all.

The long-term home for this is an opt-in test suite (like the Docker-gated
integration tests): scenario definitions plus a runner that spawns the
agent, applies the budget, and emits the scorecard — usability regression
testing for agent operators.

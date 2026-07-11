# Refine Hype Video

## Goal

Create a value-led hype video for Refine that feels impressive immediately. Do
not lead with the problem. Open with a sharp question, introduce Refine, then
move through real product capabilities with hype music, clean voiceover, and fast
UI proof. Keep captions short. Make the product feel obvious.

Target length: 75-95 seconds. Hard cap: 110 seconds.

Primary audience: Product and QA buyers who should understand the value by
watching the workflow, not by being told which pain points they have.

Value proposition:

- **Decentralized:** Local repo, git, provider, machine, existing processes, and
  infrastructure.
- **Cheap feedback loops:** Goals turn feedback into reviewable agent work.
- **Planning and chat:** People can think, steer, and follow up in context.
- **Quality automation:** Guidance, Governance, and Quality align the work.
- **Human verification:** People review before anything is done.

## Opening

Use this opening structure:

1. Fast dashboard montage.
2. On-screen question:

   > Agentic software delivery?

3. Cut to Refine UI.
4. On-screen title:

   > Introducing Refine

Then move directly into capabilities.

## Recording Checklist

Record short OBS clips with clean cursor movement, no waiting, and no private
data visible.

- **Intro:** Refine dashboard loaded with real-looking Goals, activity, statuses,
  and agent state.
- **Dashboard:** needs attention, running agents, review queue, and recent
  activity.
- **New Goal:** `+ New Goal`, reporter/context selector, actionable prompt
  behavior, and submit.
- **Import:** paste a bug report, customer note, meeting transcript, or feature spec;
  show extracted draft Goals and the review/confirm flow.
- **Plan / Chat:** create-menu Plan flow and attached `Open Chat` on a Goal.
- **Agent workflow:** status movement through `todo`, `in-progress`,
  `ready-merge`, `review`, and `done`.
- **Quality:** Quality gate and test evidence, preferably including a
  screenshot result if available.
- **Guidance / Governance:** configured rules and instructions shaping work
  before execution.
- **Processes:** System -> Processes with UI, runner, workers, agents,
  pause/resume, cancel, and target app controls.
- **Files / Changes:** file browser or changed files proving Refine is operating
  against a real repo.
- **Install close:** README quick-start command or terminal showing:

  ```bash
  curl -fsSL https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh | bash
  ```

## Voiceover Script

Use this as the base narration, paced around 80 seconds:

> Agentic software delivery?
>
> Introducing Refine.
>
> Refine helps teams build, fix, and verify software with agents, without giving
> up local ownership or human control.
>
> Start with a Goal: what the app does now, what it should do instead, and who
> reported it.
>
> Create one manually, or paste a bug report, customer note, or meeting
> transcript. Refine turns real feedback into reviewable work.
>
> Each Goal gets its own agent run, its own branch, and its own worktree, so
> multiple changes can move at once while git keeps the work isolated and
> traceable.
>
> The dashboard shows what is running, what needs review, what failed, and what
> is ready for attention.
>
> Guidance and Governance shape agent work before it starts. Quality checks and
> test evidence keep it aligned with product intent and requirements.
>
> When an agent finishes, Refine moves the change through logs, review, merge,
> and explicit human verification.
>
> Need judgment? Open Chat, inspect files, resolve issues, or send the agent
> another round.
>
> The runtime is visible too: processes, workers, active agents, target app
> controls, pause, and resume. It runs locally against your repo, your git, your
> provider, and your machine.
>
> Refine turns agent work into a local, repeatable delivery workflow people can
> operate inside the practices they already use.
>
> Install Refine, connect it to an app, and start shipping verified changes.

## Scene Edit Plan

Use hype pacing, real UI proof, short captions, and tight zooms. Captions should
state user value, not just name UI features.

| Time | Scene | Footage | Callout |
| --- | --- | --- | --- |
| 0-4s | Opening question | Fast dashboard montage | Agentic software delivery? |
| 4-8s | Reveal | Refine title, dashboard, top nav | Introducing Refine |
| 8-18s | Capture work | `+ New Goal`, prompt field, reporter | Product intent becomes work |
| 18-28s | Import | Paste messy feedback, extracted draft Goals | Feedback becomes Goals |
| 28-42s | Agents running | Status changes, activity feed, running indicator | Parallel work, clean git |
| 42-58s | Review and steer | Goal detail, logs, Chat, Files/Changes, `Verify` | Humans approve what ships |
| 58-70s | Quality/control | Quality, Guidance, Governance | Automation follows intent |
| 70-84s | Operations | Processes, workers, pause/resume, target app controls | Runtime you can operate |
| 84-94s | CTA | Install command and polished dashboard final frame | Start shipping verified changes |

## CapCut Edit Notes

- Keep UI clips between 3 and 7 seconds.
- Trim all waits, loading time, and repeated cursor movement.
- Use quick zooms on meaningful states: created Goal, extracted drafts, running
  agent, review, verify, Quality result, and process table.
- Keep hype music energetic but low enough for narration to stay clear.
- Use captions as short value statements tied to the visible capability.
- Do not use generated b-roll unless it supports a transition; real product
  footage should carry the proof.
- Blur or crop private paths, repo URLs, customer names, tokens, secrets, and
  unreleased customer data.
- Export a 16:9 version first. Make a vertical crop only after the main cut
  works.

## Value Emphasis

Prioritize these ideas in the final edit:

- More than prompting: a delivery workflow people can operate.
- Feedback becomes executable work.
- Parallel agent runs stay clean in git.
- Humans review, steer, and verify.
- Quality controls keep automation aligned.
- Local runtime, visible processes, existing practices.

## Avoid

- A problem-led opening.
- Generic AI magic visuals.
- Stock team footage.
- Claims that are not shown by real UI footage.
- Overexplaining the product in captions.
- Recording secrets, private paths, tokens, or customer identifiers.

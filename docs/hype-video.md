# Refine Hype Video

## Goal

Create a value-led hype video for Refine that feels impressive immediately. Do
not lead with the problem. Open with a sharp question, introduce Refine, then
move through real product capabilities with hype music, clean voiceover, and fast
UI proof. The footage should show capability; the voiceover and captions should
translate that capability into value for the user.

Target length: 90-110 seconds. Hard cap: 120 seconds.

Primary audience: Product and QA buyers who should understand the value by
watching the workflow, not by being told which pain points they have.

Value proposition:

- **Local ownership:** Refine runs against the user's repo, git, provider, and
  machine.
- **Cheap feedback loops:** Gaps move from report to agent work to human review.
- **Planning and chat:** People can think with agents before execution and steer
  follow-up work.
- **Quality automation:** Guidance, Governance, and Quality keep automation
  aligned with product intent and requirements.
- **Human verification:** People explicitly review the result before it becomes
  done.
- **Operational continuity:** Refine fits existing repositories, branches,
  processes, and development practices.

## Opening

Use this opening structure:

1. Fast dashboard montage.
2. On-screen question:

   > What should your software delivery process look like when agents are part
   > of the team?

3. Cut to Refine UI.
4. On-screen title:

   > Introducing Refine

Then move directly into capabilities.

## Recording Checklist

Record short OBS clips with clean cursor movement, no waiting, and no private
data visible.

- **Intro:** Refine dashboard loaded with real-looking Gaps, activity, statuses,
  and agent state.
- **Dashboard:** needs attention, running agents, review queue, and recent
  activity.
- **New Gap:** `+ New Gap`, reporter/context selector, actual behavior, target
  behavior, and submit.
- **Import gaps:** paste a bug report, customer note, or meeting transcript;
  show extracted draft Gaps and the review/confirm flow.
- **Plan / Chat:** create-menu Plan flow and attached `Open Chat` on a Gap.
- **Agent workflow:** status movement through `todo`, `in-progress`,
  `ready-merge`, `review`, and `done`.
- **Quality:** Quality gate and regression evidence, preferably including a
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

Use this as the base narration, paced around 100 seconds:

> What should your software delivery process look like when agents are part of
> the team?
>
> Introducing Refine.
>
> Refine is an agentic software delivery system that helps teams build, fix, and
> verify software without giving up local ownership or human control.
>
> Start with a Gap: what the app does now, what it should do instead, and who
> reported it. Product, QA, support, and customers can turn real observations
> into useful software work.
>
> Create one manually, or paste a bug report, customer note, or meeting
> transcript. Refine extracts the work into reviewable Gaps, so feedback becomes
> clear, actionable, and ready to run.
>
> Each Gap gets its own agent run, its own branch, and its own worktree, so
> multiple changes can move at once while git keeps the work isolated and
> traceable.
>
> The dashboard keeps the feedback loop visible: what is running, what needs
> review, what failed, and what is ready for human attention.
>
> Guidance and Governance shape agent work before it starts. Quality checks and
> regression evidence keep automation aligned with product intent, local rules,
> and requirements.
>
> When an agent finishes, Refine does not just call it done. It moves the change
> through merge flow, logs, review, and explicit human verification, preserving
> human judgment where it matters.
>
> Need judgment? Open Chat on the Gap, inspect files, resolve issues, or send
> the agent another round. People can steer the system instead of starting over.
>
> The runtime is visible too: processes, workers, active agents, target app
> controls, pause and resume, all running locally against your repo, your git,
> your provider, and your machine. Refine works inside the development practices
> you already use.
>
> Refine turns agent work into a local, repeatable delivery workflow people can
> actually operate.
>
> Install Refine, connect it to an app, and start shipping verified changes.

## Scene Edit Plan

Use hype pacing, real UI proof, short captions, and tight zooms. Captions should
state user value, not just name UI features.

| Time | Scene | Footage | Callout |
| --- | --- | --- | --- |
| 0-5s | Opening question | Fast dashboard montage | What should software delivery look like with agents? |
| 5-10s | Reveal | Refine title, dashboard, top nav | Introducing Refine |
| 10-22s | Capture work | `+ New Gap`, actual/target fields, reporter | Product intent becomes executable work |
| 22-34s | Import | Paste messy feedback, extracted draft Gaps | Turn real feedback into reviewable Gaps |
| 34-48s | Agents running | Status changes, activity feed, running indicator | Move fast without branch collisions |
| 48-62s | Review | Gap detail, logs, merge/review state, `Verify` | Humans approve what ships |
| 62-76s | Quality/control | Quality, Guidance, Governance | Automation follows product rules |
| 76-90s | Operations | Processes, workers, pause/resume, target app controls | Keep the runtime visible and controllable |
| 90-102s | Chat/files | Attached Chat and Files/Changes | Inspect, steer, and recover in context |
| 102-112s | CTA | Install command and polished dashboard final frame | Start shipping verified changes |

## CapCut Edit Notes

- Keep UI clips between 3 and 7 seconds.
- Trim all waits, loading time, and repeated cursor movement.
- Use quick zooms on meaningful states: created Gap, extracted drafts, running
  agent, review, verify, Quality result, and process table.
- Keep hype music energetic but low enough for narration to stay clear.
- Use captions as punchy value propositions tied to the visible capability.
- Do not use generated b-roll unless it supports a transition; real product
  footage should carry the proof.
- Blur or crop private paths, repo URLs, customer names, tokens, secrets, and
  unreleased customer data.
- Export a 16:9 version first. Make a vertical crop only after the main cut
  works.

## Value Emphasis

Prioritize these ideas in the final edit:

- Refine is a full delivery workflow that people can operate, not just a code
  generation prompt.
- Gaps make product intent executable while keeping feedback cheap to submit and
  review.
- Isolated branches and worktrees let teams move multiple changes without
  losing git clarity.
- Review, Chat, Files, and explicit Verify keep humans in control.
- Guidance, Governance, and Quality make agent work better aligned with product
  intent and requirements.
- Processes and runtime controls make the system visible, recoverable, and
  operationally trustworthy.
- Local execution preserves ownership of the repo, credentials, provider, and
  development workflow.

## Avoid

- A problem-led opening.
- Generic AI magic visuals.
- Stock team footage.
- Claims that are not shown by real UI footage.
- Overexplaining the product in captions.
- Recording secrets, private paths, tokens, or customer identifiers.

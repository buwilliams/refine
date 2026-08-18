# Table of Contents

- [Design](01-design.md)
- Model
  - [Node](02-model/01-node.md)
  - [Models](02-model/02-models.md)
  - [Target App](02-model/03-target-app.md)
  - [Fleet](02-model/04-fleet.md)
- Application
  - Agents
    - [Overview](03-application/01-agents/00-overview.md)
    - Agent Operations
      - [Overview](03-application/01-agents/01-agent-operations/00-overview.md)
      - [Import](03-application/01-agents/01-agent-operations/01-import.md)
    - [Guidance](03-application/01-agents/02-guidance.md)
    - [Quality](03-application/01-agents/03-quality.md)
    - [Governance](03-application/01-agents/04-governance.md)
    - [Merge, Review, And Git Worktrees](03-application/01-agents/05-merge-review-git-worktrees.md)
    - [Activity And Evidence](03-application/01-agents/06-activity-evidence.md)
    - [Planning](03-application/01-agents/07-planning.md)
    - [Agent-First Orchestration](03-application/01-agents/09-agent-first-orchestration.md)
    - [Goal Agents](03-application/01-agents/10-goal-agents.md)
  - Workflow
    - [Overview](03-application/02-workflow/00-overview.md)
    - [Backlog](03-application/02-workflow/01-backlog.md)
    - [Todo](03-application/02-workflow/02-todo.md)
    - [Plan](03-application/02-workflow/03-plan.md)
    - [Implement](03-application/02-workflow/04-implement.md)
    - [Quality](03-application/02-workflow/05-quality.md)
    - [Governance](03-application/02-workflow/06-governance.md)
    - [Review](03-application/02-workflow/07-review.md)
    - [Done](03-application/02-workflow/08-done.md)
    - [Failed](03-application/02-workflow/09-failed.md)
    - [Cancelled](03-application/02-workflow/10-cancelled.md)
    - [Shared Workflow Consistency Contract](03-application/02-workflow/11-consistency-contract.md)
  - [Execution Ownership](03-application/03-execution-ownership.md)
  - [Persistence Sync](03-application/04-persistence-sync.md)
- Infrastructure
  - [Process](04-infrastructure/01-process.md)
- Surfaces
  - [Surface Principles](05-surfaces/01-surface-principles.md)
  - [CLI](05-surfaces/02-cli.md)
  - Browser
    - [Overview](05-surfaces/03-browser/00-overview.md)
    - Shared Components
      - [Overview](05-surfaces/03-browser/01-shared-components/00-overview.md)
      - [Table](05-surfaces/03-browser/01-shared-components/01-table.md)
      - [Pagination](05-surfaces/03-browser/01-shared-components/02-pagination.md)
    - [Nav](05-surfaces/03-browser/02-nav.md)
    - [Command Palette](05-surfaces/03-browser/03-command-palette.md)
    - [Main](05-surfaces/03-browser/04-main.md)
    - [Dashboard](05-surfaces/03-browser/05-dashboard.md)
    - [Workflow](05-surfaces/03-browser/06-workflow.md)
    - [Feature](05-surfaces/03-browser/07-feature.md)
    - [Goal](05-surfaces/03-browser/08-goal.md)
    - [Import](05-surfaces/03-browser/09-import.md)
    - [Changes Visualizations](05-surfaces/03-browser/10-changes-visualizations.md)
    - [Log](05-surfaces/03-browser/11-log.md)
    - [Settings](05-surfaces/03-browser/12-settings.md)
    - [Guide](05-surfaces/03-browser/13-guide.md)
    - [Target App](05-surfaces/03-browser/14-target-app.md)
    - [Toolbar](05-surfaces/03-browser/15-toolbar.md)
    - [System](05-surfaces/03-browser/16-system.md)
    - [Processes](05-surfaces/03-browser/17-processes.md)
    - [Files](05-surfaces/03-browser/18-files.md)
    - [Terminal](05-surfaces/03-browser/19-terminal.md)
    - [Chat](05-surfaces/03-browser/20-chat.md)
    - [Standalone](05-surfaces/03-browser/21-standalone.md)
    - [Footer](05-surfaces/03-browser/22-footer.md)
  - [API](05-surfaces/04-api.md)
  - [Agent](05-surfaces/05-agent.md)
  - [MCP](05-surfaces/06-mcp.md)

## Key Ideas

- **Intent Over Implementation**: these documents explain why Refine exists, why each part exists, and what outcomes each part should preserve.
- **Table of Contents As Design**: the file layout should make the system understandable before any file is opened.
- **Consistent Vocabulary**: use the same words for the same concepts across every document.
- **Purpose First**: describe each feature by its purpose, expected role, and future direction before naming implementation details.
- **Implementation As Evidence**: include technical details only when they explain or protect intent.
- **Future AI Readers**: write so stronger future agents can preserve the design even when they change the code.

## Purpose

The intent folder is the durable explanation of Refine's design. It is not a changelog, implementation manual, or product marketing site. It is the place where the system states what it is trying to become and what must remain true as the implementation changes.

These documents should help people and agents understand the product from the inside out:

- what Refine believes about work,
- what each system area is responsible for,
- why each feature exists,
- what outcomes the feature should create,
- what future versions should preserve or improve.

## Document Shape

Each feature document should generally use this shape:

- **Key Ideas**: the small set of principles that define the feature.
- **Purpose**: why the feature exists.
- **Expected Role**: how the feature contributes to the whole system.
- **Future Direction**: how the feature should evolve as Refine and AI agents improve.

The sections may be adapted when a topic needs a different shape, but the document should still answer the same questions.

## Organization

The root document, `01-design.md`, explains the whole-system design.

The remaining documents are organized by semantic responsibility:

- **Model**: domain concepts, invariants, states, and policies.
- **Application**: Refine behavior and orchestration.
- **Infrastructure**: Git, process, host, storage, provider, and telemetry mechanisms.
- **Surfaces**: adapters for people and machines.

Each section should be discrete enough to read on its own and connected enough to make the whole system easier to understand.

## Writing Rules

- Lead with key ideas.
- Use consistent vocabulary.
- Explain features by purpose, expected role, and future direction.
- Avoid implementation detail unless it matters to the intent.
- Prefer plain language over framework language.
- Keep the writing compact enough that the structure stays visible.
- Preserve the product philosophy even when describing technical tradeoffs.

## Implementation Detail

Implementation details belong in these documents when they explain a product decision. For example, Rust, flat files, and Git matter because they serve the intent of performance, local ownership, infrastructure simplicity, and agent-friendly operation.

Implementation details do not belong here when they only describe how the current code happens to be arranged. Those details should live closer to the code unless they protect an intentional design choice.

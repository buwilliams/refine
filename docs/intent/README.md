# Organizing Principles

## Key Ideas

- **Intent Over Implementation**: these documents explain why Refine exists, why each part exists, and what outcomes each part should preserve.
- **Table Of Contents As Design**: the file layout should make the system understandable before any file is opened.
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

## Table Of Contents

- [Design](01-design.md)
- Foundation
  - [Models](02-foundation/01-models.md)
  - [Node](02-foundation/02-node.md)
  - [State](02-foundation/03-state.md)
  - [Storage](02-foundation/04-storage.md)
  - [Target App](02-foundation/05-target-app.md)
- Capabilities
  - [Workflow](03-capabilities/01-workflow.md)
  - [Process](03-capabilities/02-process.md)
  - [Tools](03-capabilities/03-tools.md)
  - [Guidance](03-capabilities/04-guidance.md)
  - [Import](03-capabilities/05-import.md)
  - [Quality](03-capabilities/06-quality.md)
  - [Governance](03-capabilities/07-governance.md)
  - [Merge, Review, And Git Worktrees](03-capabilities/08-merge-review-git-worktrees.md)
  - [Activity And Evidence](03-capabilities/09-activity-evidence.md)
- Surfaces
  - [Surface Principles](04-surfaces/01-surface-principles.md)
  - [CLI](04-surfaces/02-cli.md)
  - Browser Desktop
    - [Overview](04-surfaces/03-browser-desktop/00-overview.md)
    - Shared Components
      - [Overview](04-surfaces/03-browser-desktop/01-shared-components/00-overview.md)
      - [Table](04-surfaces/03-browser-desktop/01-shared-components/01-table.md)
      - [Pagination](04-surfaces/03-browser-desktop/01-shared-components/02-pagination.md)
    - [Nav](04-surfaces/03-browser-desktop/02-nav.md)
    - [Command Palette](04-surfaces/03-browser-desktop/03-command-palette.md)
    - [Main](04-surfaces/03-browser-desktop/04-main.md)
    - [Dashboard](04-surfaces/03-browser-desktop/05-dashboard.md)
    - [Workflow](04-surfaces/03-browser-desktop/06-workflow.md)
    - [Feature](04-surfaces/03-browser-desktop/07-feature.md)
    - [Gap](04-surfaces/03-browser-desktop/08-gap.md)
    - [Import](04-surfaces/03-browser-desktop/09-import.md)
    - [Changes Visualizations](04-surfaces/03-browser-desktop/10-changes-visualizations.md)
    - [Log](04-surfaces/03-browser-desktop/11-log.md)
    - [Settings](04-surfaces/03-browser-desktop/12-settings.md)
    - [Guide](04-surfaces/03-browser-desktop/13-guide.md)
    - [Target App](04-surfaces/03-browser-desktop/14-target-app.md)
    - [Toolbar](04-surfaces/03-browser-desktop/15-toolbar.md)
    - [System](04-surfaces/03-browser-desktop/16-system.md)
    - [Processes](04-surfaces/03-browser-desktop/17-processes.md)
    - [Files](04-surfaces/03-browser-desktop/18-files.md)
    - [Terminal](04-surfaces/03-browser-desktop/19-terminal.md)
    - [Chat](04-surfaces/03-browser-desktop/20-chat.md)
    - [Standalone](04-surfaces/03-browser-desktop/21-standalone.md)
    - [Footer](04-surfaces/03-browser-desktop/22-footer.md)
  - [API](04-surfaces/04-api.md)
  - [Agent](04-surfaces/05-agent.md)

## Document Shape

Each feature document should generally use this shape:

- **Key Ideas**: the small set of principles that define the feature.
- **Purpose**: why the feature exists.
- **Expected Role**: how the feature contributes to the whole system.
- **Future Direction**: how the feature should evolve as Refine and AI agents improve.

The sections may be adapted when a topic needs a different shape, but the document should still answer the same questions.

## Organization

The root document, `01-design.md`, explains the whole-system design.

The remaining documents are organized by system level:

- **Foundation**: the concepts Refine depends on.
- **Capabilities**: the active powers Refine provides.
- **Surfaces**: the ways people and agents interact with Refine.

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

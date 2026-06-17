# Tools

## Key Ideas

- **Shared Capability Layer**: tools are reusable system powers, not surface-specific helpers.
- **Host Tools**: Refine should use the host's existing infrastructure: Git, shells, target-app commands, provider CLIs, and local files.
- **Product Tools**: work item, import, chat, project state, node, merge, and registry services should preserve product semantics.
- **Observability Tools**: logs, activity, metrics, diagnostics, support bundles, and process views should make work inspectable.
- **Agent Usability**: tools should be callable by humans, surfaces, and agents through the same core behavior.

## Purpose

Tools are how Refine acts on the world. They connect the model and workflow to the user's actual development environment: Git worktrees, target app commands, provider CLIs, quality checks, imports, diagnostics, logs, and project files.

Tools should not be confused with UI widgets. A button may expose a tool, but the tool itself should live in shared backend capability so other surfaces and future agents can use it.

This section is the parent for tool-backed agent capabilities. Import gets its own child document because it is a major tool flow: turning external plans, transcripts, files, and issue lists into structured Gaps and Features.

## Expected Role

The tools capability should make Refine useful without requiring users to adopt new infrastructure. It should wrap existing local systems in product-aware behavior.

Current implementation details that matter to intent:

- host tools cover agent providers, clusters, deployed updates, Git worktrees, installation, quality, and target apps.
- product tools cover chat, imports, merging, nodes, project migration, project registry, project state, and work items.
- observability tools cover activity, diagnostics, logs, metrics, processes, and support bundles.
- the work item service centralizes Gap and Feature behavior so surfaces share the same rules.
- chat and standalone worktree behavior are product tools, not browser-only behavior.

Tools should be powerful. Refine's safety posture is mitigation greater than prevention: use Git, logs, governance, quality checks, review, and observability to make powerful actions recoverable and accountable.

## Future Direction

As AI improves, tools should become the interface through which agents compose software. Refine should make tools discoverable, structured, auditable, and reusable across surfaces.

The long-term direction is not a fixed list of tools. It is a tool layer that future agents can reason about, extend, and orchestrate while preserving product semantics: what work is being done, against which project, under whose guidance, with what evidence, and with what recoverability.

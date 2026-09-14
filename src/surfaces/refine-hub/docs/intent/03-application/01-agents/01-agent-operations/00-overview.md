# Agent Operations

## Key Ideas

- **Application Owns Intent**: agent operations should preserve Refine's product rules, authority, and orchestration.
- **Infrastructure Owns Mechanism**: Git, shells, provider CLIs, processes, local files, and telemetry should remain host mechanisms rather than a generic product layer.
- **Model In, Model Out**: operations should consume and produce ordinary Refine concepts and evidence.
- **Surface Independent**: browser controls, CLI commands, APIs, MCP, and agents should reach the same Application behavior.
- **Agent Usability**: operations should be discoverable, composable, and accountable for both people and agents.

## Purpose

Agent operations are how Refine turns agent intent into product behavior and host effects. They connect Model and Application concerns to the user's development environment: Git worktrees, target-app commands, provider CLIs, quality checks, imports, diagnostics, logs, and project files.

A button, CLI command, or MCP tool may expose an operation, but none of those Surfaces should own it. Application owns the use case and Infrastructure supplies the mechanism. There is no generic Tools layer between them.

This section describes operations commonly initiated by agents. Import gets its own child document because it is a major Application flow: turning external plans, transcripts, files, and issue lists into structured Goals and Features.

## Expected Role

Agent operations should make Refine useful without requiring users to adopt new infrastructure. Application should compose existing local mechanisms into product-aware behavior.

Current implementation details that matter to intent:

- Application owns chat, imports, fleet orchestration, project migration and registry behavior, work items, Quality, Governance integration, diagnostics, installation, and target-app lifecycle.
- Infrastructure owns provider discovery and invocation, Git and worktree mechanisms, subprocess supervision, storage layout, runtime discovery, logs, activity, and metrics.
- the work item service centralizes Goal and Feature behavior so Surfaces share the same rules.
- chat and standalone worktree behavior are Application behavior, not browser-only behavior.

Agent operations should be powerful. Refine's safety posture is mitigation greater than prevention: use Git, logs, Governance, Quality, review, and observability to make powerful actions recoverable and accountable.

## Future Direction

As AI improves, agent operations should become a primary way software is composed. Refine should make Application behavior discoverable, structured, auditable, and reusable across Surfaces.

The long-term direction is not a fixed list of tools or a generic tool layer. It is an Application that future agents can reason about, extend, and orchestrate while Infrastructure remains replaceable and product semantics stay explicit: what work is being done, against which project, under whose guidance, with what evidence, and with what recoverability.

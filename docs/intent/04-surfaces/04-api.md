# API

## Key Ideas

- **Local Daemon Contract**: the API is primarily the contract between surfaces and the local Refine daemon.
- **Capability Groups**: routes should map to real system capabilities, not arbitrary page needs.
- **Surface Alignment**: browser, CLI, desktop, and agent integrations should share API behavior where appropriate.
- **Not A SaaS Boundary By Default**: the API should not imply that Refine must become a centralized hosted service.
- **Recoverable Mutations**: API writes should flow through shared services with idempotency, logging, and state repair where needed.

## Purpose

The API exists so surfaces can talk to the local daemon consistently. It gives browser JavaScript, CLI daemon routing, desktop wrappers, and future agent integrations a shared way to access project status, work items, workflow, processes, chat, settings, files, terminal sessions, diagnostics, and more.

The API should be treated as local capability plumbing. It is important, but it is not the product center.

## Expected Role

The API should expose system capability groups that match Refine's product design. Current route groups include system, apps, project, target app, work, workflow, activity, import, dashboard, agents, operations, runner workers, processes, events, quality, chat, settings, governance, guidance, reporters, nodes, cluster, changes, cache, performance, files, terminal, diagnostics, and upgrade.

Those groups are useful because they map surfaces onto shared behavior. They should not drift into page-specific endpoints when a shared service would express the capability better.

The API should remain local-first. It should be secure by context, constrained by local daemon ownership, and careful about which operations mutate target state.

## Future Direction

Future agent-native surfaces may use the API directly or through a higher-level protocol. The API should be structured enough for automated discovery and stable enough that agents can rely on it.

If Refine later supports distributed or hosted operation, the API may become a stronger remote contract. That should be an intentional scale step, not an accidental consequence of browser implementation.

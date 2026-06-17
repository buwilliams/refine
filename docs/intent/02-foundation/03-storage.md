# Storage

## Key Ideas

- **Flat Files First**: storage should be easy to inspect, copy, version, and repair.
- **Git Backed**: history, isolation, rollback, and review should come from existing source-control infrastructure.
- **Cache For Speed**: derived indexes and projections should improve performance without becoming authoritative.
- **No Required Database**: Refine should not force users to adopt new infrastructure before they need it.
- **Atomic Enough For Local Work**: file writes should protect normal local operation without pretending to be a global transaction system.

## Purpose

Storage exists to preserve user ownership and agent readability. Refine should store its durable product concepts in files that live with the software project, not in an opaque remote service by default.

Flat files matter because they make Refine legible. A person or agent can inspect a Gap, Feature, setting, guidance file, governance rule, or log without first negotiating a database schema, hosted service, or proprietary API.

## Expected Role

Storage should serve three outcomes:

- make product state durable,
- make state easy for agents and people to inspect,
- make local operation fast enough to feel native.

The current implementation uses flat JSON-like product records, runtime files, projection snapshots, and Git worktrees. This lets Refine keep durable state simple while still supporting fast list screens, workflow automation, background processes, standalone worktrees, and recoverable daemon operation.

The important boundary is source of truth. Caches, indexes, and projections are allowed and necessary for performance, but they should be rebuildable from durable state. If a cache is wrong, the repair path should be refresh or rebuild, not manual database surgery.

## Future Direction

At larger scale, Refine may need stronger replication, distributed coordination, richer indexing, or optional server-backed storage. Those additions should be treated as scale adaptations, not as a replacement for the core intent.

The future storage design should still preserve local ownership, inspectability, Git compatibility, and agent readability. If superintelligent systems can coordinate software composition across many repositories, they will still need durable artifacts that explain what happened and why.

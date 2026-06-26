# MCP

## Key Ideas

- **Always Available**: the MCP surface starts with the local daemon and shares its lifetime. Whenever Refine is running, MCP clients can connect.
- **Capability Reuse, Not Reimplementation**: every MCP tool dispatches back through the shared daemon API. MCP adds a protocol adapter, not a parallel implementation of work.
- **Standard Protocol For Agents**: MCP gives external AI clients a discoverable, stable way to read and act on Refine without learning its internal HTTP routes.
- **Local Daemon Contract**: MCP is served by the same local daemon as every other surface. It is local capability access, not a hosted integration boundary.
- **Discoverable Tools**: tools list themselves with names and input schemas so agents can find capabilities instead of guessing endpoints.

## Purpose

The MCP surface exists so MCP-speaking AI clients can use Refine directly through the Model Context Protocol. It complements the CLI and HTTP API: where the CLI is the explicit, scriptable agent interface and the API is the contract between surfaces and the daemon, MCP is the standard protocol an external assistant already knows how to speak.

It exists to lower the cost of agent integration. An agent should be able to attach to a running Refine daemon, list the available tools, and start reading status, Gaps, and Features or driving capability routes — without bespoke wiring for Refine's internals.

MCP is treated as a thin, always-on adapter. It is important because it widens who can operate Refine, but it is not the product center and it does not own any capability.

## Expected Role

The MCP surface should be mounted by the daemon web server and reachable as soon as the daemon is up. It speaks JSON-RPC 2.0 and answers the core MCP methods: capability negotiation on `initialize`, tool discovery on `tools/list`, and tool execution on `tools/call`.

Its tools should map onto real system capabilities the same way the API route groups do. Reads (system status, dashboard, Gaps, Features) should be first-class and safe. A general request tool should remain available as an escape hatch so an agent can reach any daemon route — including writes — without the catalog having to enumerate every capability up front.

Because MCP delegates to the shared daemon API, it inherits the system's local-first security, idempotency, logging, and state-repair behavior rather than re-deriving them.

## Future Direction

As agent-native interaction grows, MCP may become a primary way external assistants drive Refine. The tool catalog should grow toward the most valuable capabilities — planning, import, workflow advancement, review — while keeping each tool aligned to a shared capability rather than a one-off behavior.

Future versions may add streaming, richer tool schemas, or capability scoping. Those should be intentional steps that preserve the core intent: an always-available, standard, local protocol over the same durable capabilities every other surface uses.

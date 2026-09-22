# How Refine works today

Baseline: Refine 4.3.2, source reviewed 22 September 2026. [Assessment scope](00-overview.md).

## From a Goal to a change

1. A person or integration uses the browser, CLI, or MCP surface to describe work and invoke capabilities.
2. The daemon coordinates Goals, workflow state, agent sessions, and local processes for an attached application repository.
3. Configured provider CLIs execute agent work on a node. Their tools can read or change what their effective OS identity and tool configuration allow.
4. Application changes go through Refine's workflow and review mechanisms. A workflow approval is distinct from a production deployment authorization.
5. Shared coordination state synchronizes through Git on `refine/state`. Other nodes use their own runtimes, credentials, and configured agents.

The model provider is a separate service from Refine. A local coordinator does not imply that prompts, code context, tool output, or attachments remain local: provider behavior, account terms, selected tools, and egress policy determine the actual boundary.

## Authority and storage

| Surface | Source-established behavior | Enterprise implication |
| --- | --- | --- |
| HTTP daemon | `system start` defaults to all IPv4 interfaces, port 8082; the address is configurable | Explicitly bind and firewall; a local installation is not proof of loopback-only exposure |
| API and MCP | Shared capability dispatch, including work and process controls | Treat reachable control endpoints as privileged; inventory API, SSE, terminal and Hub routes |
| Agent execution | Provider command configuration controls invocation | Review effective arguments and tools; bundled Claude/Codex defaults bypass approval controls, and Codex's flag also bypasses its sandbox |
| Shared state | Git-backed coordination state | Classify Goals, prompts, output, Hub content, and repository access; verify actual synchronized paths |
| Secrets | OS-specific backends with a file fallback | Verify the selected backend, file permissions, encryption, rotation and backup behavior on each OS |
| Activity | Mutation events and runtime records exist | Verify identity attribution, completeness, retention, export and tamper protection before calling them audit evidence |

A process cannot normally exceed an enforced OS or network restriction simply because an agent requested it. However, broad user privileges, accessible credentials, elevation tools, and trusted browser sessions may already confer substantial authority. Agent instructions are not an enforcement boundary.

## Source evidence

Paths are relative to the Refine repository at the baseline commit:

- `src/surfaces/cli/actions/system.rs` — daemon listen defaults.
- `src/surfaces/web_server/http/runtime.rs` and `http/transport.rs` — bind, HTTP dispatch and mutation records.
- `src/surfaces/web_server/server.rs` — shared API/MCP dispatch.
- `src/model/providers_defaults.json` — bundled provider invocation flags; installed settings may differ.
- `src/infrastructure/process/supervisor/security/host_commands.rs` and `security/secrets.rs` — backend selection and file fallback. Unix file creation requests mode 0600; that alone does not prove existing-file permissions or Windows ACLs.
- [State synchronization recovery](../docs/runbooks/state-sync-recovery.md) and [installation](../docs/runbooks/install.md).

This review did not exercise customer production systems, test native desktop automation, or inspect private provider credentials.

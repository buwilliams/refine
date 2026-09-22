# Enterprise operating plan

This plan is for the people who use Refine, administer machines, manage identity, review contracts, support applications and oversee assurance. Start with [how Refine works](01-how-it-works.md). The controls below follow its actual execution path: a request reaches the daemon, a provider starts tools under a host identity, and those tools read data or act in other systems.

**Status:** proposed deployment plan grounded in the source baseline on the [overview](00-overview.md). No enterprise installation has been assessed by this guide. Record the deployment's own results alongside these instructions. Completing the plan supports the organization's SOC 2 controls; the assurance team determines audit scope and sufficiency.

## Who does what

| Role | Responsibility |
| --- | --- |
| Business owner | Defines the useful task, permitted data and acceptable consequences; approves the intended use |
| Refine administrator | Records version, provider settings, enabled tools and runtime configuration; controls changes |
| Infrastructure and identity teams | Restrict host permissions, network access, accounts, sessions and credential storage |
| Security operations | Collects activity records, monitors alerts and can stop access independently of Refine |
| Engineering and application owners | Review changes, dependencies, releases and permissions in target systems |
| Legal, procurement and data owners | Approve rights to inputs, provider agreements, license obligations and handling of confidential material |
| Assurance owner | Connects evidence to existing SOC 2 controls and tracks gaps, exceptions and review dates |

One person may hold several roles, but a worker must not approve its own privilege increase, package exception or release. Use existing enterprise approval systems where possible.

## Gate 1 — Define a bounded use

The business owner describes one task, such as changing code in a test repository. List the input data, expected output, systems the tools may touch and actions requiring human approval. Begin with synthetic or explicitly approved non-production data. Record prohibited actions, including production changes, external publication and sending messages where not authorized.

The administrator inventories the installed Refine commit, OS, daemon service definition, provider CLI versions and arguments, tools and integrations. Identify every credential available through files, environment variables, command-line login caches and browser profiles. Refine's child processes may inherit these even when no credential was entered in Refine itself.

**Gate evidence:** approved use description, data inventory, named owners, current configuration and [network map](02-network.md). No customer data or production credentials until their owners approve the specific use and controls.

## Gate 2 — Restrict the execution environment

Use a dedicated non-administrator account and a worker environment containing only the required repositories and data. Apply enterprise patching, disk encryption, endpoint monitoring and restricted file access. Remove standing elevation rights, unrelated browser profiles, cloud credentials and host-management sockets. An isolated VM can provide a stronger boundary than a directory alone; validate any mounted paths and management access.

The daemon's source default is all IPv4 interfaces on port 8082. Configure the installed service explicitly for loopback access, or allow only a tested identity gateway to reach it. Inspect the actual running listener after restarting the service; a CLI flag is not proof that an existing service picked up the change. Block unapproved inbound connections. For remote use, require encrypted transport, enterprise login and MFA, and deny direct backend access.

Limit outbound connections to approved Git, model, artifact, identity and target-system endpoints. Include DNS, proxies, telemetry, updates and backups. Exercise provider tools under these restrictions: disabling incoming HTTP alone does not restrict their outbound access or local file authority.

Inspect the effective provider configuration. Bundled Claude and Codex profiles contain permission-bypass flags, and Codex's bypass includes its sandbox. Determine whether the provider can run with enterprise-approved restrictions; test compatibility. Where bypass remains necessary, enforce the boundary outside the agent through host, identity and network controls and record that decision.

**Gate evidence:** denied network and file-access tests, effective process arguments, host baseline and working approved task. Infrastructure owns these restrictions; an instruction in a prompt cannot substitute for them.

## Gate 3 — Protect identity, data and intellectual property

Complete the [identity and emulation](03-identity-and-emulation.md) checks. Prefer short-lived, narrowly scoped target credentials. Keep browser sessions separate for each authorized principal. Verify both session revocation and running-tool termination; signing out of the entry page may not stop an existing downstream session.

Check the actual secret storage path using a harmless test value. Refine can fall back to an ordinary serialized file if a native-store write fails. Verify permissions and backup handling, and alert on an unapproved fallback. Do not infer successful secret retrieval from the preferred backend label. Windows native reads have a documented implementation limitation in the reviewed code; validate a complete store/read/delete cycle on the deployment OS before relying on that backend.

Approve each provider account and data route using the [IP guide](07-intellectual-property.md). Configure [artifact controls](08-artifacts-and-providers.md) before allowing package installation. Review prompts, tool output, screenshots, Git state, telemetry and backups as possible copies of confidential information. Keep secrets out of them. External documents, repository text and web pages can contain instructions intended to mislead an agent; treat this content as untrusted and limit the damage its tools can do.

**Gate evidence:** tested account scopes and revocation, approved provider register, package rejection results, secret backend checks and data retention rules.

## Gate 4 — Prove the complete workflow is supportable

Run the [verification workbook](09-verification.md) in the restricted environment. Demonstrate an allowed task, a denied task, a reviewed change and a controlled release. Keep production deployment credentials outside the development worker unless that exact operational use has separately passed review. Existing application release controls still apply to agent-written changes.

Send security records to a store the worker cannot alter. Connect gateway identity, Refine Goal/run, host process, Git change and target-system event. Refine's local API mutation records contain method, path, status and time; they do not alone identify the human or prove every tool action. Gateway access logs alone also cannot explain all child-process activity. If correlation cannot be established, reduce the workflow's scope or resolve the gap before approval.

Back up required configuration, repositories and coordination state with controlled access. Restore to an isolated environment, verify the recovered state and keep actions disabled until credentials and approvals are rechecked. Measure recovery against business-defined objectives. Rehearse stopping the daemon and its children, blocking egress and revoking downstream credentials.

**Gate evidence:** linked test results, log samples, restore and shutdown results, known gaps and support contacts. The business, security and assurance owners record approval for a defined scope and expiration/review date; approval is not transferable to arbitrary tools or data.

## Keep the controls operating

The following is a starting schedule to adapt to enterprise policy and risk, not a SOC 2 mandated timetable.

| When | Operator action | Retained evidence |
| --- | --- | --- |
| Each run | Use approved workspace and identity; capture run, output and required approval references | Run-to-change record without secrets |
| Each change or release | Review source, scan dependencies and secrets, verify artifact identity and authorization | Review, scan, build and release records |
| Continuously where supported; otherwise each operating day | Monitor unexpected destinations, account misuse, disabled logging, secret fallback and package-policy bypass | Alerts, triage and resolution |
| Monthly starting point | Review access, stale sessions, exception expiry, patches and unresolved findings | Owner sign-off and remediation tickets |
| Quarterly starting point | Exercise recovery and emergency shutdown; sample end-to-end attribution | Drill results, measured recovery, corrective actions |
| On provider, model, tool, identity, data or network change | Revisit affected tests, contracts and approval scope before expanded use | Updated inventory and acceptance record |

## Incident and retirement procedure

1. Security operations contains the affected worker: stop its processes, deny network access and revoke target and provider credentials. Do not rely on the agent to cooperate.
2. Preserve restricted copies of relevant logs, configuration, process details, Git revisions and artifact digests under the incident policy. Avoid spreading leaked material into tickets or chat.
3. Determine which data left, which systems changed and which identities acted. Involve data owners and legal for confidential information or suspected IP infringement; follow contractual notification and claim procedures.
4. Remove or replace affected dependencies and outputs, rotate exposed credentials and restore an approved state. Fix the control failure and repeat the relevant tests before resuming.
5. When retiring the deployment, disable entry and scheduled work, revoke all sessions, remove credentials and provider access, and delete or retain data according to policy, legal holds and provider deletion commitments. Record completion, including backups that expire later.

[Next: Intellectual property risks](07-intellectual-property.md) · [Verification workbook](09-verification.md)

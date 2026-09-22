# Operationalizing Refine

Refine gives agents a path from intent to software changes. An enterprise deployment must define who can reach that path, what authority it carries, and how its use is recorded.

**Assessment baseline:** Refine 4.3.2, source commit `d7e2baee6a81106554c10afc7e39d49daa3159c6`, reviewed 22 September 2026. This is a source-based deployment guide, not a SOC 2 opinion or a test of an enterprise installation. Revalidate against the installed version and effective configuration.

<div class="hub-cards">
<a class="hub-card" href="01-how-it-works.md"><span class="card-index">01 / SYSTEM</span><strong>How Refine works today</strong><p>Processes, shared state, providers, and the authority of a node.</p></a>
<a class="hub-card" href="02-network.md"><span class="card-index">02 / NETWORK</span><strong>Connections &amp; trust boundaries</strong><p>Current logical architecture and a proposed enterprise deployment.</p></a>
<a class="hub-card" href="03-identity-and-emulation.md"><span class="card-index">03 / IDENTITY</span><strong>Authentication &amp; user emulation</strong><p>Separate access to Refine from access by its agents.</p></a>
<a class="hub-card" href="04-assurance.md"><span class="card-index">04 / ASSURANCE</span><strong>SOC 2 &amp; acceptance evidence</strong><p>Control responsibilities, evidence, and unresolved deployment questions.</p></a>
<a class="hub-card" href="05-deployment-options.md"><span class="card-index">05 / DECISION</span><strong>Deployment options</strong><p>Restrict network access, introduce identity controls, or disable access pending review.</p></a>
<a class="hub-card" href="06-operating-plan.md"><span class="card-index">06 / OPERATIONS</span><strong>Enterprise operating plan</strong><p>Owners, rollout gates, ongoing operation and incidents.</p></a>
<a class="hub-card" href="07-intellectual-property.md"><span class="card-index">07 / IP</span><strong>Protect intellectual property</strong><p>Input rights, confidential data, generated code and contracts.</p></a>
<a class="hub-card" href="08-artifacts-and-providers.md"><span class="card-index">08 / SOFTWARE SUPPLY</span><strong>Dependencies and artifacts</strong><p>Artifactory and alternatives, package policies and releases.</p></a>
<a class="hub-card" href="09-verification.md"><span class="card-index">09 / EVIDENCE</span><strong>Verification workbook</strong><p>Practical checks and records for enterprise acceptance.</p></a>
</div>

## The operating principle

Refine and its child processes are constrained by host permissions and enforced network controls. That is a useful starting point. It does not mean they automatically satisfy every organizational policy or inherit a SOC 2 conclusion. New listeners, external model calls, delegated sessions, and automated actions change how existing authority can be exercised.

Document the controls required for the intended use of Refine, assign an owner to each control, and verify the effective configuration before granting access to enterprise systems or data.

## What this review establishes

The source shows a configurable HTTP listener, API/MCP access to shared capabilities, provider CLI execution, Git state synchronization, and host secret backends. The daemon start default is `0.0.0.0:8082`. The reviewed HTTP path does not establish authenticated end-user identity before capability dispatch. Bundled Claude and Codex profiles include approval/sandbox bypass flags. These findings require deployment decisions; they do not establish how any particular enterprise host is configured.

Before approval, record the installed commit, actual listen address and firewall exposure, provider arguments, enabled tools, data classification, identity model, and operational owner. See the [evidence checklist](04-assurance.md).

## Where to start

Anyone evaluating Refine can begin with [how it works](01-how-it-works.md) and the [operating plan](06-operating-plan.md). Infrastructure and identity teams can follow the network and authentication pages. Engineering, procurement and legal can use the IP and artifact pages together. Assurance teams can connect the verification workbook to existing SOC 2 controls.

This guide distinguishes current Refine behavior, proposed enterprise controls and deployment evidence still to collect. Source references explain why the controls are needed. No enterprise-specific test results, executed contracts or production configuration are asserted here.

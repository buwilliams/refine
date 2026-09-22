# Deployment verification workbook

Use this workbook with the [operating plan](06-operating-plan.md). These are procedures for the enterprise to execute, not reported test passes. Use synthetic data and harmless fixtures in an approved test environment. Record failures and coverage limits as carefully as successes.

## Acceptance checks

| Check | Procedure and expected result | Responsible team |
| --- | --- | --- |
| Entry boundary | Inspect running listener and service config. Try gateway, direct port, alternate hostname and denied network. Only authorized paths succeed | Infrastructure |
| Authentication | Try absent, expired and revoked identities against browser, API, MCP, event and terminal routes. Check active streams and reject spoofed identity headers | Identity and security |
| Browser request protection | Submit harmless cross-origin changes and repeat through the real proxy. Verify rejection and correct host/origin handling; confirm authentication still applies when Origin/Referer headers are absent | Security |
| Worker isolation | Attempt a harmless write outside the allowed workspace, denied command/elevation and access to an unrelated account's fixture. All prohibited actions fail | Infrastructure |
| Tool and session authority | Run an allowed target action, denied action and revocation during work. Confirm no session crossing between unrelated users and stop access within the documented time limit | Application and identity owners |
| Data destinations | Run a representative task with synthetic markers. Capture permitted egress and inspect controlled logs/state. Attempt an unapproved endpoint. Reconcile destinations and prevent the denied transfer | Security and data owner |
| Secret storage | Store/read/delete a dummy credential on the actual OS; inspect selected storage, permissions and fallback behavior without exposing real secrets | Platform |
| Dependency control | Admit an approved fixture, deny a policy-blocked fixture, test alternate registry/URL, cache, expired exception and scanner outage | Repository administrator |
| Release control | Trace a reviewed change to source commit, build, scans, SBOM, artifact digest and authorized deployment. Deny worker self-promotion | Engineering |
| Attribution and preservation | Trace human request through Goal/run, worker process, Git and target event. Verify off-host retention and denied worker modification of retained records | Security operations |
| Recovery and shutdown | Restore an isolated backup; verify state and disabled actions. Stop running work and revoke downstream sessions independently of the agent | Operations |
| Contract and rights | Match actual account/endpoint/feature to approved agreement, data rights and required safeguards; confirm renewal and claim contacts | Procurement and legal |

## Evidence record to copy into the enterprise system

```text
Deployment / environment:
Intended use and approved data:
Refine version and source commit:
Host / OS / worker identity:
Provider CLI versions, tools and configuration reference:
Network and service configuration reference / hash:
Enterprise control identifier:
Check and test environment:
Expected result:
Observed result and coverage limitations:
Timestamp / operator / independent reviewer:
Restricted evidence links:
Finding / remediation owner / due date:
Exception approver / compensating control / expiry, if applicable:
Acceptance scope / approvers / next review:
```

Keep credentials and confidential payloads out of this template. Use restricted links to evidence rather than copying it into public Hub pages. Do not store executed contracts, production diagrams or customer records in the bundled documentation.

## Connect evidence to SOC 2

The assurance owner maps these records to the organization's existing controls, system description and applicable Trust Services Criteria. Include relevant suppliers, customer responsibilities and dependencies. Review supplier assurance reports for service scope, period, exceptions and responsibilities the enterprise must perform; a supplier report does not cover this entire deployment.

Preserve evidence of ongoing operation across the relevant review period, including access reviews, changes, alerts, exceptions, incidents and recovery exercises. A one-time configuration screenshot cannot show sustained operation. Confirm retention and sampling needs with the assurance team. [AICPA SOC 2 resources](https://www.aicpa-cima.com/topic/audit-assurance/audit-and-assurance-greater-than-soc-2).

## Decision record

Approve a named use only when its required checks have evidence and unresolved issues have an authorized disposition. Possible outcomes are: approve the stated scope; approve a narrower scope with expiring conditions; or hold access pending remediation. Record who decides and when the decision must be revisited. Neither this workbook nor installing an artifact repository grants a SOC 2 audit opinion.

[Back to overview](00-overview.md)

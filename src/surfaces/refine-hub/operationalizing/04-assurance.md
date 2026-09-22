# SOC 2 & operational acceptance

SOC 2 examines a service organization's described system and controls against applicable Trust Services Criteria. It is not a property that a tool acquires by running on an audited laptop. The system boundary and selected criteria determine the relevant controls and evidence. [AICPA SOC 2 overview](https://www.aicpa.com/cpe-learning/publication/soc-2-reporting-on-an-examination-of-controls-at-a-service-organization-relevant-to-security-availability-processing-integrity-confidentiality-or-privacy).

This is a readiness mapping, not a certification, audit opinion, or exhaustive mapping to individual criteria. The assurance owner and auditor determine scope.

## Responsibility and evidence

| Control area | Product/development responsibility | Enterprise/operator responsibility | Acceptance evidence |
| --- | --- | --- | --- |
| System scope & risk | Publish architecture, flows and capabilities | Classify data, dependencies and deployment use | Approved diagram, inventory, risk decisions and named owners |
| Identity & access | Enable entry controls, scoped actions and attribution | Provision users/workers, review access, revoke sessions | Denial/revocation tests; least-privilege access review |
| Host & network | Document listeners and egress dependencies | Harden, patch, isolate and monitor hosts | Listener scan, firewall rules, endpoint posture, egress tests |
| Secure development & change | Review code, manage dependencies, test and package releases | Accept versions and authorize production deployment | Source/build provenance, SBOM, test and vulnerability results, approval |
| Data & secrets | Minimize context, support protected storage and redaction | Approve providers, retention, keys and data movement | Data-flow inventory; secret-backend verification; deletion/restore tests |
| Logging & incident response | Emit useful events and support investigation | Preserve evidence, alert, respond and coordinate suppliers | Attributed event sample, retention policy, alert exercise and incident contacts |
| Availability & recovery | Provide backup/restore/rollback procedures | Set service objectives and run recovery exercises | Measured restore and rollback results; accepted recovery objectives |

NIST's SSDF provides secure software development practices that can inform review of dependencies, changes, and release evidence. [NIST SSDF](https://www.nist.gov/publications/secure-software-development-framework-ssdf-version-11-recommendations-mitigating-risk).

## Evidence pack for one deployment

Record the installed version/commit, effective configuration, host and owner, data classes, supported workflows, network diagram, tool inventory, provider agreements, and credentials model. Attach results for the tests on [identity](03-identity-and-emulation.md) and [network flows](02-network.md), plus backup/restore and incident drills. Each item needs an owner, test date, result and link to retained evidence.

Explicitly track gaps: native multi-user authentication and authorization, attribution of shared-worker actions, audit completeness/immutability, provider data handling, secret fallback, actual browser/desktop capabilities, and controls for autonomous actions. Absence of a verified control is a gap, not evidence that it exists.

## Approval boundary

Start with synthetic or approved non-production data and no production authority. Expand access only against named use cases and recorded evidence. The deployment owner records approval, remaining control gaps and accepted residual risk.

## Put the controls into operation

Follow the [enterprise operating plan](06-operating-plan.md) for owners, rollout gates and ongoing activities. Use the [verification workbook](09-verification.md) to retain evidence. Include [IP and contractual protections](07-intellectual-property.md) and [artifact controls](08-artifacts-and-providers.md) in supplier, data and change-management reviews.

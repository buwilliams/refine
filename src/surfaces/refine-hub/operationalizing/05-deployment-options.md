# Deployment options

These are proposed choices. This documentation does not change a running daemon, firewall, identity provider or organizational policy.

## 1. Restrict HTTP exposure

**Useful first containment:** explicitly bind Refine to loopback on a dedicated managed development machine and block remote inbound access to the daemon. Check the service's effective configuration: `system start` defaults to all interfaces in the reviewed version. Revalidate after restart and upgrade.

Blocking all HTTP would also block Refine's browser/API interactions. Blocking plaintext internet egress alone leaves HTTPS egress and local control available. Blocking inbound network access does not restrict what agent processes can do with local files or outbound credentials. Specify direction, interfaces, ports, clients and protocols rather than saying only “block HTTP.”

For remote users, use a tested TLS/identity gateway and prevent direct access to its backend. Local loopback access is still available to permitted local processes; use a dedicated worker boundary when that matters.

## 2. Third-party identity and controlled sessions

**Useful for managed shared access:** put enterprise identity controls in front of Refine, with tested session expiry, revocation and complete route coverage. Design downstream delegation separately. Reusing an authenticated target session can enable automation, but it is not a substitute for scope, intent, attribution or action authorization.

This option needs integration and acceptance testing. It does not turn the current product into a verified multi-tenant service merely by adding an SSO login page.

## 3. Disable Refine access pending review

If the required controls cannot be verified, stop Refine workloads and remove their access to enterprise systems and data. Revoke associated credentials and sessions, preserve required records, and follow the organization's software removal process where applicable.

Re-enable access only after the deployment owner has approved the configuration and verified the required controls.

## Recommended staged configuration

1. Define a restricted development environment, permitted data and allowed provider/tool set. Assign an operational owner and capture the baseline.
2. Use local-only access for an initial bounded pilot, or validate a gateway before shared remote access. Verify the controls; do not label a proposed configuration as deployed.
3. Verify the installed version, dependencies, security results, identity controls, logging, configuration and recovery procedures.
4. Record approval for the intended use, monitor the deployment, and reassess controls when access, tools or configuration change.

Choose based on the required workflow and evidence. NIST's zero-trust architecture rejects implicit trust based only on network location or device ownership; host controls remain necessary within the overall design. [NIST SP 800-207](https://www.nist.gov/publications/zero-trust-architecture).

Implement the chosen option through the [operating plan](06-operating-plan.md), including identity, IP, dependency, monitoring and recovery controls.

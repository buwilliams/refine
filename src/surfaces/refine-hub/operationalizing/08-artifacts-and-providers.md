# Controlled dependencies and release artifacts

Refine starts provider CLIs whose tools can run package managers and build commands with the worker's authority. This is where Artifactory or an equivalent repository service fits: it controls the software that the worker can obtain and the artifacts the enterprise retains. It does not automatically control model prompts, browser sessions or arbitrary internet downloads.

## Intended flow

```text
Public package sources
        | approved repository service fetches packages
        v
Policy evaluation and quarantine ----> rejected or unknown: review queue
        | permitted package versions and digests
        v
Internal package repository <---- restricted Refine worker and CI downloads
                                         |
                              reviewed source and controlled build
                                         |
                                         v
                      private release repository + evidence
                                         |
                              authorized deployment process
```

CI means the automated build and test system. A digest is a content fingerprint used to identify exact bytes. The arrows describe a proposed enterprise control, not a built-in Refine integration. Model-service traffic follows a separate approved route under the [network plan](02-network.md).

## Configure and operate the control

1. **Inventory the downloads.** Include language packages, container base images, OS packages, provider CLI updates, browser binaries, plugins, direct Git dependencies and install scripts. List each package manager and any URL-based download. An uncovered format needs a separate control or must be disallowed.
2. **Establish private repositories.** Separate third-party intake, private packages, build candidates and approved releases. Disable anonymous access. Give workers read access only to needed packages; give build/promotion identities narrowly scoped write access. Keep credentials outside source and prompts.
3. **Enable policy enforcement.** Define denied licenses, severity and exploitability handling, known malicious components, approved sources and treatment of unknown or unscanned components. Validate that enforcement blocks rather than merely reports. Require review for unknown results and scan outages before admitting new components; retain explicitly approved existing artifacts for continuity under a documented policy.
4. **Route all workers and builds through it.** Configure each package manager, enforce approved outbound destinations and block direct public registry/download paths. Test an explicit alternate registry, direct URL, Git source and install script. Configuration files alone can be bypassed by a tool with network access.
5. **Control version selection.** Commit lockfiles where supported, verify hashes/signatures where available, reserve internal package names and define registry precedence. Prevent a public package with a matching internal name from winning resolution. Apply controls to cached and transitive packages, not only new direct dependencies.
6. **Handle exceptions outside the agent.** Record component, version/digest, reason, affected applications, compensating control, approver and expiry. Have the security or license owner approve; prevent the worker from altering policy. Revoke expired exceptions and verify they are actually denied.
7. **Build and promote exact artifacts.** Review changes, scan code and dependencies, produce an SBOM, retain required notices and record source commit, build identity and artifact digest. Sign or otherwise protect provenance as required by enterprise policy. Promote the tested artifact without rebuilding different bytes for deployment.
8. **Monitor after release.** Reevaluate retained components when new vulnerability or license information arrives. Use the SBOM to locate affected releases, assign remediation deadlines and preserve a known-good rollback artifact. Test repository backup/restore and administrative audit retention.

JFrog Curation documents package intake policies, while Xray provides component security and license analysis. Feature coverage depends on format and entitlement. Sonatype Repository Firewall is another intake-control option; its setup guidance notes that initial audit does not quarantine already-cached components. Explicitly review existing inventory. [JFrog Curation](https://jfrog.com/curation/), [JFrog Xray](https://jfrog.com/xray/), [Sonatype setup](https://help.sonatype.com/en/repository-firewall-getting-started.html).

## What to prove before relying on the repository

Using harmless fixtures in a test repository, demonstrate an approved package succeeds and a policy-denied package fails. Repeat through the worker's actual package manager and CI. Attempt direct-download bypass, an unapproved cached version and an unapproved publishing operation. Simulate unavailable policy evaluation and confirm the chosen fail-closed behavior for new admissions. Verify an expired exception is rejected. Retain policy version, package digest, caller identity, result and evidence link.

The repository administrator owns access and availability; security owns threat policy; legal or the open-source owner owns license decisions; engineering owns component inventory and release obligations. Repository scanning cannot establish all rights in generated source. Review the separate [IP and contract controls](07-intellectual-property.md).

## Protect private artifacts at the provider

For hosted repositories, verify the agreement covers private source and binaries, tenant isolation, authorized support access, confidentiality, subprocessors, incident notice, export/return and deletion. Confirm retention and recovery behavior and what happens when the subscription ends. For self-hosted repositories, the enterprise operates storage, keys, backups and access controls, and must still examine telemetry and support-upload routes.

Do not equate a vendor's protection against claims about its own product with indemnification for everything customers store or build using it. Record the actual scope and limitations in the provider register before making that assurance.

[Next: Verification workbook](09-verification.md)

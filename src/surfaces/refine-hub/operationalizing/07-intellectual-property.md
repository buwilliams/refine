# Intellectual property: risks and protection

Intellectual property (IP) includes source code, business rules, documentation and other protected work. Refine's tools can read this material, send selected context to a model service, download third-party components and generate changes. Protection must cover each of those routes. Running on an enterprise machine does not by itself establish permission to send material to a provider or redistribute the resulting software.

## Four questions for every workflow

1. **May we use this input?** Confirm the right to access, copy and send customer, vendor and employee-created material for this purpose.
2. **Who receives it?** Identify the actual account, model endpoint, tool service, artifact host, log store and backup destination.
3. **May we ship the output?** Review dependency licenses, notices, provenance and potentially copied material before release.
4. **Who is contractually responsible if something goes wrong?** Identify the signed terms, covered service, exclusions, notification duties and remedy.

## Risk-to-operation map

| Risk and connection to Refine | Required operation | Owner and evidence |
| --- | --- | --- |
| Proprietary code leaves through provider context or a tool call | Permit only approved data classes and destinations; minimize context; use approved business accounts; test with synthetic markers | Data owner and security: approved flow and observed destinations |
| Customer or vendor material has use restrictions | Check the agreement before placing it in an accessible workspace or prompt; use synthetic substitutes when rights are unclear | Legal and business owner: purpose-specific rights decision |
| A package imposes license obligations or contains malicious code | Route downloads through controlled repositories; inspect transitive dependencies; enforce license and security policy; review unknowns | Engineering: dependency inventory, decisions, notices and scan results |
| Generated code resembles third-party work | Review contributions and available provenance or matching results; investigate suspicious passages and preserve origin information | Reviewer and legal when needed: disposition linked to change |
| Credentials, prompts or confidential output persist in Git, logs, screenshots or backups | Minimize and redact collection, restrict readers, set retention and verify deletion paths; rotate exposed credentials immediately | Platform and security: access reviews and retention/deletion evidence |
| Unclear ownership of contributed work | Check employee/contractor agreements and licenses for imported code; record human review and contribution | Legal and engineering: agreements and change history |
| A provider's protection does not cover the actual route | Verify product, account, purchase channel, model, feature and required settings against executed terms | Procurement: service-to-contract register and re-review date |

An SBOM (software bill of materials) lists software components. It helps locate dependencies and their obligations; it does not prove that all generated source is original, that every license was detected or that confidential information was never disclosed. Security and license scanners provide evidence within their coverage, not universal legal clearance.

## Ownership, confidentiality and indemnity are different

**Ownership terms** describe rights between the contracting parties. They do not establish that output is original or enforceably copyrighted. In the United States, the Copyright Office explains that sufficient human authorship matters; prompts alone do not necessarily establish it. Keep records of substantive human contributions and have legal evaluate the relevant jurisdiction and use. [U.S. Copyright Office report announcement](https://www.copyright.gov/newsnet/2025/1060.html).

**Confidentiality and data-use terms** constrain how a provider may handle submitted material. Check training, retention, support access, subprocessors, deletion and incident notification separately. A no-training term does not mean no storage or no access.

**IP indemnity or defense obligations** allocate responsibility for specified third-party claims. They are conditional contractual remedies, not a promise that infringement cannot occur. Coverage may change with inputs, modifications, combinations, product tier and required safeguards. The contract owner should document whether the intended software workflow meets those conditions.

## Examples to evaluate against the actual purchase

Public sources reviewed 22 September 2026. These are examples for procurement review, not confirmation of Insurity's entitlements or a review of its signed agreements.

| Provider or service | What the public source establishes | Deployment decision |
| --- | --- | --- |
| JFrog Artifactory with appropriate Xray/Curation services | Repository, component analysis and package-admission capabilities address software supply-chain risk; coverage and features depend on the products/configuration | Confirm subscriptions and enforcement. Separately review confidentiality, handling of private artifacts and any IP remedy in executed terms; do not assume hosting or scanning indemnifies customer code |
| Sonatype Repository Firewall | Policies can evaluate and quarantine components at repository intake | Confirm the supported repository integration and license; inventory existing cached packages too. Review the hosting and commercial agreement separately |
| Anthropic services covered by Commercial Terms | Sections B and E address customer content rights, no model training on that content and confidentiality. Section K provides defense/indemnity for specified claims from authorized paid use, with exclusions including inputs, modifications, combinations, known infringement, certain patent and trademark claims | Verify the actual Refine provider login uses a covered service/account and applicable service-specific terms. Record exclusions and prompt claim-notice duties; consumer access is not interchangeable |
| GitHub Generative AI Services under direct volume licensing | The terms address ownership, training use and extend defense to outputs **if the main agreement provides it**. Microsoft purchases and personal customers follow different agreements; previews have separate terms | Verify purchase route, underlying defense clause, Required Mitigations and actual feature. A GitHub agreement does not cover an unrelated model endpoint simply because the source repository is on GitHub |

Sources: [JFrog Curation](https://jfrog.com/curation/), [JFrog Xray](https://jfrog.com/xray/), [JFrog terms index](https://jfrog.com/terms-and-conditions/), [Sonatype Repository Firewall](https://help.sonatype.com/en/repository-firewall.html), [Sonatype getting started](https://help.sonatype.com/en/repository-firewall-getting-started.html), [Anthropic Commercial Terms](https://www.anthropic.com/legal/commercial-terms), [GitHub Generative AI Services Terms](https://github.com/customer-terms/github-generative-ai-services-terms).

## Make contractual protection operational

Before confidential material is used, procurement and legal retain the executed agreement, order, data/security addenda and incorporated terms in the restricted contract store. Record the legal entity, covered services and accounts, approved data, region, retention, training restrictions, support/subprocessor access, deletion terms and renewal date. Record confidentiality and IP clauses separately, including liability limits, exclusions and defense/notification procedure. A general marketing statement is not the contract record.

The administrator then maps every provider configuration and tool endpoint to an approved register entry. Test the route with synthetic content and verify the account in the provider's administrative records. Disable personal or unapproved accounts and fallback routes. Restrict who may change endpoint URLs, tools and credentials; re-review before enabling previews or new services.

At renewal or material change, procurement checks terms and entitlement; engineering checks required safeguards remain enabled. If protection lapses or the route cannot be established, suspend confidential inputs for that route until resolved. Maintain an incident contact and promptly refer potential claims to legal so contractual notice and cooperation duties can be met.

[Next: Artifact controls](08-artifacts-and-providers.md) · [Operating plan](06-operating-plan.md)

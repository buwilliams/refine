# Source architecture

Refine's Rust crate has four semantic roots:

- `model` contains domain types, invariants, status policies, and pure derivations. It does not depend on runtime, filesystem, process, or surface code.
- `application` contains Refine capabilities and their orchestration.
- `infrastructure` contains Git, process, host, storage, provider-execution, and telemetry mechanisms.
- `surfaces` contains CLI, MCP, HTTP/browser, and website adapters. Code outside this root does not import surface modules.

`error.rs` is the neutral shared error boundary. `lib.rs` exports only these roots and that error boundary. Rust callers move atomically with module reorganizations; compatibility aliases are not retained.

## Deferred coupling work

This organization is semantic housekeeping, not a crate-wide ports-and-adapters rewrite. Some application capabilities still construct concrete filesystem, process-supervision, Git-worktree, and installed-provider services. Prompt transport also consumes the provider capability and effective launch-environment contracts while owning prompt materialization policy. Governed workflow and persistence synchronization similarly call concrete Git and process mechanisms behind repository locks.

Those dependencies are explicit follow-up candidates for ports where substitution or independent deployment creates value. They should not be inverted mechanically: workflow authority, exact-candidate proof, state-sync fencing, durable evidence, and repository-lock ordering must remain the controlling constraints of any later extraction.

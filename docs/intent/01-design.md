# Design

## Key Ideas

- **AI Agent First**: keep surfaces replaceable, application behavior shared, and the model legible so agents can operate Refine directly. Prefer installed AI agents through CLIs over direct provider APIs.
- **Decentralized**: use flat files + Git + system caching instead of leveraging databases.
- **Simple Nomenclature**: anyone can describe "a goal", what something does today and what it should do next. AIs do well when they are given outcomes with enough environmental considerations (context).
- **Performant**: use low-level programming language (Rust) for maximum performance.
- **Bounded Concerns**: do not implement authorization, authentication, or features AIs are likely to quickly subsume. Push those concerns upstream (authn/authx) or downstream (frontier models).
- **Open To Everyone**: everyone, regardless of skill, can use Refine.
- **Leverage Existing Infrastructure**: instead of a centralized application, use all the same tools systems are already using: Rust, Git, flat files, AI agents, browser, and whatever else makes sense. Do not force people to adopt new infrastructure until a tipping point arrives if it does.
- **Mitigation Greater Than Prevention**: instead of restricting capability, put in guardrails and safety checks to ensure systems that Refine works on maintain their purpose and functionality, but do not prevent bad actors because those same rules prevent novelty and breakthroughs. Provide unrestricted power tools to get work done, rely on mitigations for safety: Git-backed flat files, governance, quality checks.
- **Agent Guidance**: since everything that can be automated by AI will be, we should provide "guidance" to the agents so that they have the necessary context of concerns to do their work effectively.
- **Capability Overhang**: model intelligence is spiky, and new capabilities often arrive before products learn to use them. Keep agent prompts concise, test what current models can do, and remove scaffolding that limits stronger models without improving outcomes.
- **Ambitious Outcomes**: do not preserve familiar trade-offs by default. Use agent speed and capability to pursue work that is good, fast, and cheap, then rely on evidence to show whether the result holds.

## System

Most software systems are commonly explained in terms of presentation, business logic, and persistence. That framing helps explain the kinds of concerns Refine must handle: interaction, action, and durable state.

Refine separates those concerns by responsibility:

- Model defines domain concepts, invariants, status policies, and pure derivations.
- Application turns intent into behavior by orchestrating Refine's workflows and use cases.
- Infrastructure provides Git, process, host, storage, provider, and telemetry mechanisms.
- Surfaces adapt Refine for people and machines through CLI, MCP, HTTP, browser, website, and future interfaces.

Durable state crosses these responsibilities without blurring them. Model defines what the state means, Application decides how it may change, Infrastructure stores and transports it, and Surfaces expose it. Flat files, Git, runtime state, and caches protect local ownership, inspectability, performance, recoverability, and agent readability without becoming one undifferentiated architectural layer.

### Surfaces

A system can have many types of interfaces: CLI, API, browser, MCP, voice, and agent, and the list is growing by the day. Often several of these are used at once. Therefore, Refine should not be dependent on any one of them with the base being the CLI.

#### API and UI

This is currently the most user-friendly version of Refine, but I expect it to be subsumed by personal agents who will work directly with Refine.

#### CLI

The CLI is the most reliable (because of limited UI statefulness) surface.

## Model, Application, Infrastructure, And Surfaces

Refine should be understood through four semantic areas:

- Model is the stable language of Goals, Features, Nodes, projects, workflow states, evidence, and policy.
- Application owns Refine behavior: orchestration, workflow, synchronization, agents, projects, diagnostics, maintenance, and system operations.
- Infrastructure owns mechanisms supplied by the host environment: Git, subprocesses, runtime discovery, storage layout, provider invocation, and observability.
- Surfaces translate human or machine interaction into Application behavior without owning product meaning.

The Model should remain small and independent of runtime, filesystem, process, and surface concerns. Its concepts should be simple enough for people to explain and structured enough for agents to operate on without guessing.

Application behavior should be shared. Agents, Workflow, synchronization, projects, diagnostics, and system operations should not belong to one UI, command, or integration. Application may use concrete Infrastructure where current substitution does not justify another abstraction, but product decisions, authority, and orchestration remain in Application.

Infrastructure should provide mechanisms without deciding what Refine work means. Git operations, process supervision, provider invocation, storage, runtime discovery, and telemetry should preserve the authority and evidence requirements supplied by Application rather than inventing parallel product policy.

The Surfaces should be replaceable. Browser, CLI, API, voice, and agent-native interfaces will evolve quickly. Refine should treat them as adapters over the same Application and Model so a new surface can appear without changing what work means.

## Intended Outcome

Refine should become the local operating layer for agentic software work. It should let people and AI systems express work as direct Goal prompts, organize those Goals into larger Features, preserve the context needed to act on them, and move the work through implementation, quality, review, and merge.

The long-term direction is software composition at scale: workflow, persistence, and orchestration for fleets of agents. If future AI systems find better internal designs, they should still preserve the product intent:

- work is represented as understandable goals and features,
- agents receive enough context and guidance to act well,
- state is durable, inspectable, and owned by the user,
- surfaces are conveniences over shared Application behavior,
- process execution is observable and recoverable,
- safety comes from mitigation, auditability, Git, governance, and quality checks rather than capability denial,
- prompts and workflow leave room for current agents to discover stronger solutions than the initial specification anticipated.

## Design Pressure

Refine should resist becoming a centralized SaaS-shaped system by default. Centralization may become useful at some scale, but the first design pressure is local ownership: the user's code, files, Git history, settings, runtime state, and agent outputs should remain close to the work.

Refine should also resist becoming a UI-shaped system. The browser matters because it makes the product accessible, but the core system should be understandable and operable by agents directly. As AI gets better, the highest-value surface may be an agent reading the intent docs, inspecting the flat files, and using the shared Application without needing a human-style screen.

## Architecture Direction

The current implementation uses Rust, flat files, Git, a local daemon, shared services, and static browser assets because those choices serve the intent:

- Rust supports fast local operation and long-lived background processes.
- Flat files keep state inspectable, portable, and easy for agents to read.
- Git provides history, isolation, rollback, and merge discipline.
- The daemon gives surfaces a single local authority for runtime state and process control.
- Shared services keep CLI, browser, API, and agent surfaces aligned.
- Static browser assets keep the user interface deployable without a separate frontend infrastructure stack.

The Rust crate mirrors the philosophy through `model`, `application`, `infrastructure`, and `surfaces`, with `error.rs` as a neutral shared boundary. Model does not depend on the other areas, and code outside Surfaces does not depend on surface adapters. These boundaries are semantic guidance, not a demand for a mechanical ports-and-adapters rewrite: later abstractions should earn their place while preserving workflow authority, exact-candidate evidence, synchronization fencing, durable records, and repository-lock ordering.

These choices are not sacred on their own. They are important because they protect performance, ownership, infrastructure simplicity, surface independence, and agent readability.

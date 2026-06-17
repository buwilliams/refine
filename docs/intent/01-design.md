# Design

## Key Ideas

- **AI Agent First**: separate surfaces from the shared model and capabilities to make Refine agent first. Prefer installed AI agents through CLIs over direct provider APIs.
- **Decentralized**: use flat files + Git + system caching instead of leveraging databases.
- **Simple Nomenclature**: anyone can describe "a gap", what something does today and what it should do next. AIs do well when they are given outcomes with enough environmental considerations (context).
- **Performant**: use low-level programming language (Rust) for maximum performance.
- **Bounded Concerns**: do not implement authorization, authentication, or features AIs are likely to quickly subsume. Push those concerns upstream (authn/authx) or downstream (frontier models).
- **Open To Everyone**: everyone, regardless of skill, can use Refine.
- **Leverage Existing Infrastructure**: instead of a centralized application, use all the same tools systems are already using: Rust, Git, flat files, AI agents, browser, and whatever else makes sense. Do not force people to adopt new infrastructure until a tipping point arrives if it does.
- **Mitigation Greater Than Prevention**: instead of restricting capability, put in guardrails and safety checks to ensure systems that Refine works on maintain their purpose and functionality, but do not prevent bad actors because those same rules prevent novelty and breakthroughs. Provide unrestricted power tools to get work done, rely on mitigations for safety: Git-backed flat files, governance, quality checks.
- **Agent Guidance**: since everything that can be automated by AI will be, we should provide "guidance" to the agents so that they have the necessary context of concerns to do their work effectively.

## System

Most software systems are commonly explained in terms of presentation, business logic, and persistence. That framing helps explain the kinds of concerns Refine must handle: interaction, action, and durable state.

Refine names those concerns by product intent:

- Surfaces are the interaction points for people and agents.
- Capabilities are the active powers that move work, run processes, and use tools.
- Foundation is the durable conceptual base: model, decentralized scale, state, storage, guidance, and project context.

Storage is part of the foundation because flat files, Git, and caches are not just implementation details. They protect local ownership, inspectability, performance, and agent readability.

### Surfaces

A system can have many types of interfaces: mobile, desktop, cli, api, browser, mcp, voice, agent, and the list is growing by the day. Often several of these are used at once. Therefore, Refine should not be dependent on any one of them with the base being the CLI.

#### API and UI

This is currently the most user-friendly version of Refine, but I expect it to be subsumed by personal agents who will work directly with Refine.

#### CLI

The CLI is the most reliable (because of limited UI statefulness) surface.

#### Desktop

Since Refine is intended for any user, the Desktop and Browser are the easiest current surfaces for Refine with Agent-first as a fast follow via voice and text.

## Foundation, Capabilities, And Surfaces

Refine should be understood through three system levels:

- Foundation: the durable model, decentralized scale, state, storage, guidance, and project-context concepts.
- Capabilities: the active powers that move work, run processes, and use tools.
- Surfaces: the ways people and agents interact with those capabilities.

The foundation should remain small. It defines the concepts future agents must preserve: Gap, Feature, decentralized scale, node, workflow state, project state, logs, guidance, governance, settings, and runtime state. These concepts should be simple enough for people to explain and structured enough for agents to operate on without guessing.

The capabilities should be shared. Workflow, process management, and tools should not belong to one UI, command, or agent integration. They are the system's durable powers. Every surface should call into them rather than reimplementing them.

The surfaces should be replaceable. Browser, desktop, CLI, API, voice, and agent-native interfaces will evolve quickly. Refine should treat them as adapters over the same model and capabilities so a new surface can appear without changing what work means.

## Intended Outcome

Refine should become the local operating layer for agentic software work. It should let people and AI systems describe gaps between actual and desired software behavior, organize those gaps into larger features, preserve the context needed to act on them, and move the work through implementation, quality, review, and merge.

The long-term direction is software composition at scale: workflow, persistence, and orchestration for fleets of agents. If future AI systems find better internal designs, they should still preserve the product intent:

- work is represented as understandable gaps and features,
- agents receive enough context and guidance to act well,
- state is durable, inspectable, and owned by the user,
- surfaces are conveniences over shared capability,
- process execution is observable and recoverable,
- safety comes from mitigation, auditability, Git, governance, and quality checks rather than capability denial.

## Design Pressure

Refine should resist becoming a centralized SaaS-shaped system by default. Centralization may become useful at some scale, but the first design pressure is local ownership: the user's code, files, Git history, settings, runtime state, and agent outputs should remain close to the work.

Refine should also resist becoming a UI-shaped system. The browser and desktop surfaces matter because they make the product accessible, but the core system should be understandable and operable by agents directly. As AI gets better, the highest-value surface may be an agent reading the intent docs, inspecting the flat files, and using the shared capabilities without needing a human-style screen.

## Architecture Direction

The current implementation uses Rust, flat files, Git, a local daemon, shared services, and static browser assets because those choices serve the intent:

- Rust supports fast local operation and long-lived background processes.
- Flat files keep state inspectable, portable, and easy for agents to read.
- Git provides history, isolation, rollback, and merge discipline.
- The daemon gives surfaces a single local authority for runtime state and process control.
- Shared services keep CLI, browser, API, and agent surfaces aligned.
- Static browser assets keep the user interface deployable without a separate frontend infrastructure stack.

These choices are not sacred on their own. They are important because they protect performance, ownership, infrastructure simplicity, surface independence, and agent readability.

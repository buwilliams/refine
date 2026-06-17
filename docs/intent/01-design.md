# Design

## Key Ideas

- **AI Agent First**: separate surfaces from core logic to make Refine agent first. Also, do not use APIs, instead rely on install AI agents via CLI.
- **Decentralized**: use flat files + Git + system caching instead of leveraging databases.
- **Simple Nomenclature**: anyone can describe "a gap", what something does today and what it should do next. AI do well when they are given outcomes with enough environmental considerations (context).
- **Performant**: use low-level programming language (Rust) for maximum performance.
- **Bounded Concerns**: do not implement authoritzation, authentication, or features AIs are likely to quickly subsume. Push those concerns upstream (authn/authx) or downstream (frontier models).
- **Open To Everyone**: everyone, regardless of skill, can use Refine.
- **Leverage Existing Infrastructure**: instead of a centralized application, use all the same tools system are already using: Rust, Git, flat files, AI agents, Browser, and whatever else makes sense. Do not force people to adopt new infrastructure until a tipping point arrives if it does.
- **Mitigation Greater Than Prevention**: instead of retricting capability, put in guardrails and safety checks to ensure systems that Refine works on maintain their purpose and functionality, but do not prevent bad actors because those same rules prevent novelty and breakthroughs. Provide unrestricted power tools to get work done, rely on mitigations for safety: Git-backed flat files, governance, quality checks.
- **Agent Guidance**: since everything that can be automated by AI will be, we should provide "guidance" to the agents so that they have the necessary context of concerns to do their work effectively.

## System

A typical system can be viewed as layers: presentation, business logic, and persistence. For, the Refine system this translates to:
- Surfaces (Presentation)
- Core (Business Logic)
- Flat files (Persistence)

### Surfaces

A system can have many types of interfaces: mobile, desktop, cli, api, browser, mcp, voice, agent, and the list of growing by the day. Often several of these are used at once. Therefore, Refine should not be dependent on any one of them with the base being the CLI.

#### API and UI

This is currently the most user-friendly version of Refine, but I expect it to be subsumed by personal agents who will work directly with Refine.

#### CLI

The CLI is the most reliable (because of limited UI statefulness) surface.

#### Desktop

Since Refine is intended for any user, the Desktop and Browser are the easiest current surfaces for Refine with Agent-first as a fast follow via voice and text.

# AI Chat App

Scaffold a provider-neutral AI chat or workbench app with streaming responses, durable local state, and clear safety boundaries.

## Stack

- Node.js 24 LTS.
- Next.js latest stable release with the App Router, React Server Components, route handlers, and TypeScript.
- Tailwind CSS v4 for styling and accessible component primitives such as shadcn/ui when useful.
- A provider adapter layer for chat completion, tool calls, and streaming output. Do not hard-code a single paid vendor.
- SQLite by default for local chat history, prompt runs, and audit logs, with a documented path to PostgreSQL or pgvector when the app needs semantic search.
- Zod or equivalent schema validation for request bodies, tool inputs, and environment variables.
- Vitest for unit tests and Playwright for a browser smoke test.

## Requirements

- Generate the app in the repository root without nesting a second project directory.
- The app must run without an API key by showing a local mock provider or setup-required state.
- Implement a streaming chat route, a basic conversation list, and a single conversation view.
- Store prompt text, model/provider id, request metadata, response text, errors, and timestamps.
- Keep tool-call execution opt-in, auditable, and disabled by default unless the product Gap explicitly requires it.
- Provide `.env.example` with documented provider variables and no real secrets.
- Use package scripts named `start`, `dev`, `build`, `test`, `lint`, and `format`.
- `npm run start` must return promptly when managed by Refine. If a long-running server is needed, use a checked-in wrapper script that writes a pid file and logs under `.refine/`.
- Add a concise README with setup, provider configuration, development, testing, and production build commands.

## Acceptance

- Dependencies install cleanly.
- `npm run build`, `npm run test`, `npm run lint`, and the browser smoke test pass.
- The app boots without paid services and clearly reports when no real provider is configured.
- Refine can start, stop, and inspect the app without hanging.

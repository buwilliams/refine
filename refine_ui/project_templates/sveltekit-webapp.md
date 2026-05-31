# SvelteKit WebApp

Scaffold a lean TypeScript SvelteKit application using Svelte 5 patterns and full-stack routes.

## Stack

- Node.js 24 LTS.
- SvelteKit latest stable release with Svelte 5, runes, and TypeScript.
- Vite for development and production builds.
- Tailwind CSS v4 through the first-party Vite integration.
- SvelteKit server routes, load functions, and form actions for full-stack behavior.
- Vitest plus Testing Library for unit and component tests.
- Playwright for one browser smoke test.
- ESLint and Prettier using project-local scripts.

## Requirements

- Generate the app in the repository root without nesting a second project directory.
- Build a useful first screen with at least one interactive flow backed by a server route or form action.
- Use Svelte 5 runes for local component state and keep server-only code out of the browser bundle.
- Keep runtime configuration in `.env.example` and validate required environment variables at startup.
- Use package scripts named `start`, `dev`, `build`, `test`, `lint`, and `format`.
- `npm run start` must return promptly when managed by Refine. If a long-running server is needed, use a checked-in wrapper script that writes a pid file and logs under `.refine/`.
- Add a concise README with setup, development, testing, and production build commands.
- Do not add authentication, a database, or paid services unless the repository already requires them.

## Acceptance

- Dependencies install cleanly.
- `npm run build`, `npm run test`, `npm run lint`, and the browser smoke test pass.
- Refine can start, stop, and inspect the app without hanging.

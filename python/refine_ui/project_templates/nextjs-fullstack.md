# Next.js Full-Stack App

Scaffold a TypeScript Next.js product app with server-rendered routes, typed mutations, and a local-first data layer.

## Stack

- Node.js 24 LTS.
- Next.js latest stable release with the App Router, React Server Components, Server Actions or route handlers, and TypeScript.
- Tailwind CSS v4 for styling and accessible component primitives such as shadcn/ui when useful.
- Drizzle ORM or another typed SQL layer with SQLite for local development and a documented PostgreSQL migration path.
- Zod or equivalent schema validation for forms, server actions, API input, and environment variables.
- Vitest for unit tests and Playwright for browser smoke coverage.
- ESLint and Prettier using project-local scripts.

## Requirements

- Generate the app in the repository root without nesting a second project directory.
- Build a useful first screen with real product structure: navigation, a dashboard or list view, and one create/edit flow.
- Keep data fetching and mutations on the server by default; use Client Components only for interactive UI.
- Include a local database schema, migration command, seed command, and `.env.example`.
- Use package scripts named `start`, `dev`, `build`, `test`, `lint`, `format`, `db:migrate`, and `db:seed`.
- `npm run start` must return promptly when managed by Refine. If a long-running server is needed, use a checked-in wrapper script that writes a pid file and logs under `.refine/`.
- Add a concise README with setup, development, database, testing, and production build commands.
- Do not add authentication or paid services unless the repository already requires them.

## Acceptance

- Dependencies install cleanly.
- The local database can be migrated and seeded from a clean checkout.
- `npm run build`, `npm run test`, `npm run lint`, and the browser smoke test pass.
- Refine can start, stop, and inspect the app without hanging.

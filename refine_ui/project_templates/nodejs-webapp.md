# Node.js WebApp

Scaffold a production-ready TypeScript React single-page application for the attached empty project.

## Stack

- Node.js 24 LTS.
- Vite with the React TypeScript template.
- React with TypeScript, strict compiler settings, functional components, and React Router when the app needs more than one route.
- Tailwind CSS v4 through the first-party `@tailwindcss/vite` plugin.
- TanStack Query or a small typed API client when the app talks to HTTP services.
- Vitest plus Testing Library for unit and component tests.
- Playwright for one smoke test covering the first screen.
- ESLint and Prettier using project-local scripts.

## Requirements

- Generate the app in the repository root without nesting a second project directory.
- Use package scripts named `start`, `dev`, `build`, `test`, `lint`, and `format`.
- `npm run start` must return promptly when managed by Refine. If a long-running server is needed, use a checked-in wrapper script that writes a pid file and logs under `.refine/`.
- Include a health page or route that renders immediately without external services.
- Add a concise README with setup, development, testing, and production build commands.
- Keep the first screen useful and app-like, not a marketing landing page.
- Keep runtime configuration in `.env.example` and validate required environment variables at startup.
- Do not add authentication, a database, or paid services unless the repository already requires them.

## Acceptance

- Dependencies install cleanly.
- `npm run build`, `npm run test`, `npm run lint`, and the browser smoke test pass.
- Refine can start, stop, and inspect the app without hanging.

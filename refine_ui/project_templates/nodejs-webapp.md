# Node.js WebApp

Scaffold a production-ready TypeScript React web application for the attached empty project.

## Stack

- Node.js 24 LTS.
- Vite with the React TypeScript template.
- React with TypeScript, strict compiler settings, and functional components.
- Tailwind CSS v4 through the first-party `@tailwindcss/vite` plugin.
- Vitest plus Testing Library for unit and component tests.
- ESLint and Prettier using project-local scripts.

## Requirements

- Generate the app in the repository root without nesting a second project directory.
- Use package scripts named `start`, `dev`, `build`, `test`, `lint`, and `format`.
- `npm run start` must return promptly when managed by Refine. If a long-running server is needed, use a checked-in wrapper script that writes a pid file and logs under `.refine/`.
- Include a health page or route that renders immediately without external services.
- Add a concise README with setup, development, testing, and production build commands.
- Keep the first screen useful and app-like, not a marketing landing page.
- Do not add authentication, a database, or paid services unless the repository already requires them.

## Acceptance

- Dependencies install cleanly.
- `npm run build`, `npm run test`, and `npm run lint` pass.
- Refine can start, stop, and inspect the app without hanging.

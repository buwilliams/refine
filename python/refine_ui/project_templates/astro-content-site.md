# Astro Content Site

Scaffold a fast content-focused site for docs, blogs, marketing pages, or launch pages that need excellent static output.

## Stack

- Node.js 24 LTS.
- Astro latest stable release with TypeScript.
- Astro Content Collections and MDX for typed content.
- Islands architecture for small interactive regions only where the page needs them.
- Tailwind CSS v4 through the first-party Vite integration.
- RSS, sitemap, metadata, and image optimization configured for production.
- Vitest for content utilities and Playwright for one smoke test covering a rendered page.

## Requirements

- Generate the site in the repository root without nesting a second project directory.
- Include a useful first page, a content collection with at least two example entries, and a reusable layout.
- Keep JavaScript minimal by default; hydrate islands only for real interaction.
- Add content schema validation so broken frontmatter fails the build.
- Include package scripts named `start`, `dev`, `build`, `test`, `lint`, and `format`.
- `npm run start` must return promptly when managed by Refine. If a long-running server is needed, use a checked-in wrapper script that writes a pid file and logs under `.refine/`.
- Add a concise README with writing, local preview, testing, and deployment notes.
- Do not add authentication, a database, or paid services unless the repository already requires them.

## Acceptance

- Dependencies install cleanly.
- `npm run build`, `npm run test`, `npm run lint`, and the browser smoke test pass.
- The production build emits static pages for the example content.
- Refine can start, stop, and inspect the site without hanging.

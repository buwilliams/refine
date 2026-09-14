# Contribute to Refine Hub

Refine Hub's source lives in `src/surfaces/refine-hub/`. The build bundles its Markdown, images, and other assets into the executable. Both the application and website serve this bundle. User-created hubs continue to live in project state.

## Write for the reader

Product guides explain how to accomplish a task. Design intent records the system's purpose, architecture, and rules. Runbooks describe operational procedures. Keep each document in its relevant section and link to it instead of copying it.

## Add a release

Create `releases/<version>.md` with the title “Refine <version> — What you need to know.” State the release coverage period and review the changes across that whole period. Describe the final shipped behavior, combining related fixes and omitting reverted or superseded designs. Add one subsection for each major change. Explain what changed, why the reader cares, where to find it, and whether they need to do anything. Add screenshots of the actual interface and links to the relevant product guides.

Store screenshots beside release notes in a version-specific image directory. Keep historical screenshots unchanged. Update the home page to link to the newest release.

## Check your change

Use relative links between hub pages. Build Refine and open `/hub/sites/refine/` to check the page, its images, links, and narrow-screen layout. The website's `/docs` page uses the same hub content. Existing `/read/docs/...` and raw `/docs/...` URLs remain supported.

Keep the root README as the repository entry point. Markdown used as an agent prompt, package metadata, or a third-party license is not product documentation and stays with its owning code.

use super::*;

pub(super) fn render_markdown_document(static_root: &Path, path: &str) -> WireResponse {
    let requested = path
        .trim_start_matches("/read")
        .trim_start_matches('/')
        .trim();
    let document = if requested.is_empty() {
        "docs/intent/README.md"
    } else {
        requested
    };
    if !website_markdown_path_allowed(document) {
        return WireResponse::json(ApiResponse::json(
            400,
            json!({
                "error": {
                    "code": "invalid_markdown_path",
                    "message": "markdown reader only serves README.md and docs/**/*.md"
                }
            }),
        ));
    }
    let full_path = static_root.join(document);
    if !is_within(static_root, &full_path) || !full_path.is_file() {
        return WireResponse::json(ApiResponse::json(
            404,
            json!({
                "error": {
                    "code": "markdown_not_found",
                    "message": "markdown document was not found"
                }
            }),
        ));
    }
    let markdown = match fs::read_to_string(&full_path) {
        Ok(markdown) => markdown,
        Err(error) => {
            return WireResponse::json(ApiResponse::json(
                500,
                json!({
                    "error": {
                        "code": "markdown_read_failed",
                        "message": format!("failed to read markdown document: {error}")
                    }
                }),
            ));
        }
    };
    WireResponse::bytes(
        200,
        "text/html; charset=utf-8",
        render_markdown_html(document, &markdown).into_bytes(),
    )
}

fn website_markdown_path_allowed(path: &str) -> bool {
    !path.contains("..")
        && (path == "README.md" || (path.starts_with("docs/") && path.ends_with(".md")))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DocsNavItem {
    title: String,
    path: String,
}

struct DocsNavGroup {
    title: &'static str,
    items: &'static [DocsNavEntry],
}

struct DocsNavEntry {
    title: &'static str,
    path: &'static str,
    summary: &'static str,
}

const DOCS_NAV_GROUPS: &[DocsNavGroup] = &[
    DocsNavGroup {
        title: "Start Here",
        items: &[
            DocsNavEntry {
                title: "Design Intent",
                path: "docs/intent/README.md",
                summary: "The durable product principles behind Refine.",
            },
            DocsNavEntry {
                title: "System Design",
                path: "docs/intent/01-design.md",
                summary: "The whole-system model for local agentic delivery.",
            },
        ],
    },
    DocsNavGroup {
        title: "Foundation",
        items: &[
            DocsNavEntry {
                title: "Node",
                path: "docs/intent/02-foundation/01-node.md",
                summary: "How Refine understands local and distributed execution.",
            },
            DocsNavEntry {
                title: "Models",
                path: "docs/intent/02-foundation/02-models.md",
                summary: "The durable records agents and people work through.",
            },
            DocsNavEntry {
                title: "Target App",
                path: "docs/intent/02-foundation/03-target-app.md",
                summary: "The software project Refine is helping improve.",
            },
        ],
    },
    DocsNavGroup {
        title: "Capabilities",
        items: &[
            DocsNavEntry {
                title: "Process",
                path: "docs/intent/03-capabilities/01-process.md",
                summary: "Observable local processes, checks, and runtime work.",
            },
            DocsNavEntry {
                title: "Agents",
                path: "docs/intent/03-capabilities/02-agents/00-overview.md",
                summary: "How agents receive context, act, and leave evidence.",
            },
            DocsNavEntry {
                title: "Agent Tools",
                path: "docs/intent/03-capabilities/02-agents/01-tools/00-overview.md",
                summary: "Shared operations that let agents work consistently.",
            },
            DocsNavEntry {
                title: "Import",
                path: "docs/intent/03-capabilities/02-agents/01-tools/01-import.md",
                summary: "Turning external input into structured Refine work.",
            },
            DocsNavEntry {
                title: "Guidance",
                path: "docs/intent/03-capabilities/02-agents/02-guidance.md",
                summary: "The context agents need before they change code.",
            },
            DocsNavEntry {
                title: "Quality",
                path: "docs/intent/03-capabilities/02-agents/03-quality.md",
                summary: "How Refine keeps checks close to the work.",
            },
            DocsNavEntry {
                title: "Governance",
                path: "docs/intent/03-capabilities/02-agents/04-governance.md",
                summary: "Product rules and review constraints for agent work.",
            },
            DocsNavEntry {
                title: "Merge, Review, And Git Worktrees",
                path: "docs/intent/03-capabilities/02-agents/05-merge-review-git-worktrees.md",
                summary: "How agent work becomes reviewable Git history.",
            },
            DocsNavEntry {
                title: "Activity And Evidence",
                path: "docs/intent/03-capabilities/02-agents/06-activity-evidence.md",
                summary: "The audit trail that makes automation inspectable.",
            },
        ],
    },
    DocsNavGroup {
        title: "Workflow",
        items: &[
            DocsNavEntry {
                title: "Workflow Overview",
                path: "docs/intent/03-capabilities/03-workflow/00-overview.md",
                summary: "The lifecycle that carries Goals from idea to done.",
            },
            DocsNavEntry {
                title: "Backlog",
                path: "docs/intent/03-capabilities/03-workflow/01-backlog.md",
                summary: "Captured work that is not ready to consume capacity.",
            },
            DocsNavEntry {
                title: "Todo",
                path: "docs/intent/03-capabilities/03-workflow/02-todo.md",
                summary: "Ready work that agents or people can pick up.",
            },
            DocsNavEntry {
                title: "Plan",
                path: "docs/intent/03-capabilities/03-workflow/03-plan.md",
                summary: "Governed proposal, critique, and final implementation plan.",
            },
            DocsNavEntry {
                title: "Implement",
                path: "docs/intent/03-capabilities/03-workflow/04-implement.md",
                summary: "Plan-guided work in an isolated candidate.",
            },
            DocsNavEntry {
                title: "Quality",
                path: "docs/intent/03-capabilities/03-workflow/05-quality.md",
                summary: "Independent correction and passing test evidence.",
            },
            DocsNavEntry {
                title: "Governance",
                path: "docs/intent/03-capabilities/03-workflow/06-governance.md",
                summary: "Governed integration or bounded recovery Rounds.",
            },
            DocsNavEntry {
                title: "Review",
                path: "docs/intent/03-capabilities/03-workflow/07-review.md",
                summary: "Human and automated inspection of changes.",
            },
            DocsNavEntry {
                title: "Done",
                path: "docs/intent/03-capabilities/03-workflow/08-done.md",
                summary: "Completed work with preserved evidence.",
            },
            DocsNavEntry {
                title: "Failed",
                path: "docs/intent/03-capabilities/03-workflow/09-failed.md",
                summary: "Failures that remain inspectable and recoverable.",
            },
            DocsNavEntry {
                title: "Cancelled",
                path: "docs/intent/03-capabilities/03-workflow/10-cancelled.md",
                summary: "Work intentionally stopped without losing context.",
            },
            DocsNavEntry {
                title: "Execution Ownership",
                path: "docs/intent/03-capabilities/04-execution-ownership.md",
                summary: "Synchronized Goal authority with replaceable node-local workers.",
            },
        ],
    },
    DocsNavGroup {
        title: "Surfaces",
        items: &[
            DocsNavEntry {
                title: "Surface Principles",
                path: "docs/intent/04-surfaces/01-surface-principles.md",
                summary: "Why interfaces are adapters over shared capability.",
            },
            DocsNavEntry {
                title: "CLI",
                path: "docs/intent/04-surfaces/02-cli.md",
                summary: "The most reliable surface for people and agents.",
            },
            DocsNavEntry {
                title: "Browser",
                path: "docs/intent/04-surfaces/03-browser/00-overview.md",
                summary: "The visual product surface for local software work.",
            },
            DocsNavEntry {
                title: "API",
                path: "docs/intent/04-surfaces/04-api.md",
                summary: "Shared local access for product operations.",
            },
            DocsNavEntry {
                title: "Agent",
                path: "docs/intent/04-surfaces/05-agent.md",
                summary: "How agents understand Refine through the CLI.",
            },
        ],
    },
    DocsNavGroup {
        title: "Browser Details",
        items: &[
            DocsNavEntry {
                title: "Shared Components",
                path: "docs/intent/04-surfaces/03-browser/01-shared-components/00-overview.md",
                summary: "Reusable interaction patterns for the visual UI.",
            },
            DocsNavEntry {
                title: "Table",
                path: "docs/intent/04-surfaces/03-browser/01-shared-components/01-table.md",
                summary: "Dense, scannable data views for repeated work.",
            },
            DocsNavEntry {
                title: "Pagination",
                path: "docs/intent/04-surfaces/03-browser/01-shared-components/02-pagination.md",
                summary: "Navigation for long local records without losing context.",
            },
            DocsNavEntry {
                title: "Nav",
                path: "docs/intent/04-surfaces/03-browser/02-nav.md",
                summary: "Orientation and movement through the browser product.",
            },
            DocsNavEntry {
                title: "Command Palette",
                path: "docs/intent/04-surfaces/03-browser/03-command-palette.md",
                summary: "Fast command access without hunting through screens.",
            },
            DocsNavEntry {
                title: "Main",
                path: "docs/intent/04-surfaces/03-browser/04-main.md",
                summary: "The primary browser workspace.",
            },
            DocsNavEntry {
                title: "Dashboard",
                path: "docs/intent/04-surfaces/03-browser/05-dashboard.md",
                summary: "A high-level view of work, state, and attention.",
            },
            DocsNavEntry {
                title: "Workflow",
                path: "docs/intent/04-surfaces/03-browser/06-workflow.md",
                summary: "The browser view of work movement.",
            },
            DocsNavEntry {
                title: "Feature",
                path: "docs/intent/04-surfaces/03-browser/07-feature.md",
                summary: "Larger product goals composed from Goals.",
            },
            DocsNavEntry {
                title: "Goal",
                path: "docs/intent/04-surfaces/03-browser/08-goal.md",
                summary: "The core unit of product difference and repair.",
            },
            DocsNavEntry {
                title: "Import",
                path: "docs/intent/04-surfaces/03-browser/09-import.md",
                summary: "Bringing outside work into the browser surface.",
            },
            DocsNavEntry {
                title: "Changes Visualizations",
                path: "docs/intent/04-surfaces/03-browser/10-changes-visualizations.md",
                summary: "Readable change views for review and understanding.",
            },
            DocsNavEntry {
                title: "Log",
                path: "docs/intent/04-surfaces/03-browser/11-log.md",
                summary: "Evidence and event history in the browser.",
            },
            DocsNavEntry {
                title: "Settings",
                path: "docs/intent/04-surfaces/03-browser/12-settings.md",
                summary: "Project configuration, governance, and local controls.",
            },
            DocsNavEntry {
                title: "Guide",
                path: "docs/intent/04-surfaces/03-browser/13-guide.md",
                summary: "Contextual help close to the current task.",
            },
            DocsNavEntry {
                title: "Target App",
                path: "docs/intent/04-surfaces/03-browser/14-target-app.md",
                summary: "The attached software project inside the UI.",
            },
            DocsNavEntry {
                title: "Toolbar",
                path: "docs/intent/04-surfaces/03-browser/15-toolbar.md",
                summary: "Persistent tools, chat, files, and terminal access.",
            },
            DocsNavEntry {
                title: "System",
                path: "docs/intent/04-surfaces/03-browser/16-system.md",
                summary: "Runtime, install, and operational state.",
            },
            DocsNavEntry {
                title: "Processes",
                path: "docs/intent/04-surfaces/03-browser/17-processes.md",
                summary: "Process visibility and control in the browser.",
            },
            DocsNavEntry {
                title: "Files",
                path: "docs/intent/04-surfaces/03-browser/18-files.md",
                summary: "File inspection close to agent work.",
            },
            DocsNavEntry {
                title: "Terminal",
                path: "docs/intent/04-surfaces/03-browser/19-terminal.md",
                summary: "Command execution without leaving the product context.",
            },
            DocsNavEntry {
                title: "Chat",
                path: "docs/intent/04-surfaces/03-browser/20-chat.md",
                summary: "Conversation tied to work and evidence.",
            },
            DocsNavEntry {
                title: "Standalone",
                path: "docs/intent/04-surfaces/03-browser/21-standalone.md",
                summary: "Focused browser use outside the primary workspace.",
            },
            DocsNavEntry {
                title: "Footer",
                path: "docs/intent/04-surfaces/03-browser/22-footer.md",
                summary: "Low-priority navigation and product affordances.",
            },
        ],
    },
    DocsNavGroup {
        title: "Install",
        items: &[DocsNavEntry {
            title: "Agent Install Runbook",
            path: "docs/runbooks/install.md",
            summary: "A direct install path for coding agents.",
        }],
    },
];

fn docs_nav_entries() -> impl Iterator<Item = &'static DocsNavEntry> {
    DOCS_NAV_GROUPS.iter().flat_map(|group| group.items.iter())
}

fn render_markdown_html(path: &str, markdown: &str) -> String {
    let markdown = markdown_for_website(path, markdown);
    let rendered = render_markdown_fragment(path, &markdown);
    let nav = render_docs_nav(path);
    let pager = render_docs_pager(path, &docs_nav_items());
    let escaped_path = escape_html(path);
    let escaped_title = escape_html(&docs_title_for_path(path));
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{escaped_title} - Refine Docs</title>
    <link rel="icon" href="/src/surfaces/web/static/images/favicon.svg" type="image/svg+xml">
    <link rel="stylesheet" href="/src/surfaces/website/assets/site.css">
    <script src="/src/surfaces/website/assets/site.js" defer></script>
  </head>
  <body class="reader-body">
    <header class="site-header">
      <a class="brand" href="/" aria-label="Refine home">
        <img src="/src/surfaces/web/static/images/refine_logo_transparent.png" alt="">
      </a>
      <div class="site-menu-shell">
        <button
          class="menu-button"
          type="button"
          data-menu-toggle
          aria-controls="site-menu"
          aria-expanded="false"
          aria-label="Open navigation menu"
        >
          <svg aria-hidden="true" viewBox="0 0 24 24">
            <path d="M4 7h16"></path>
            <path d="M4 12h16"></path>
            <path d="M4 17h16"></path>
          </svg>
        </button>
        <div class="site-menu" id="site-menu" data-menu-panel hidden>
          <nav aria-label="Primary">
            <a href="/">Home</a>
            <a href="/#product">Product</a>
            <a href="/#start">Get Started</a>
            <a href="/docs">Docs</a>
            <a href="/#agents">Agents</a>
            <a href="/read/docs/runbooks/install.md">Install</a>
            <a href="/{escaped_path}">Raw Markdown</a>
            <a href="https://github.com/buwilliams/refine">GitHub</a>
          </nav>
          <div class="menu-docs" aria-label="Documentation sections">
            {nav}
          </div>
        </div>
      </div>
    </header>
    <main class="reader-shell">
      <article class="markdown-card">
        <div class="doc-meta">{escaped_path}</div>
        {pager}
        <div class="markdown-content">{rendered}</div>
        {pager}
      </article>
    </main>
  </body>
</html>"#
    )
}

pub(super) fn render_docs_landing_page() -> WireResponse {
    let nav = render_docs_nav("docs");
    let groups = render_docs_landing_groups();
    WireResponse::bytes(
        200,
        "text/html; charset=utf-8",
        format!(
            r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Refine Product Guide</title>
    <meta
      name="description"
      content="A web-first guide to Refine's product model, agent workflow, governance, quality, and local software delivery surfaces."
    >
    <link rel="icon" href="/src/surfaces/web/static/images/favicon.svg" type="image/svg+xml">
    <link rel="stylesheet" href="/src/surfaces/website/assets/site.css">
    <script src="/src/surfaces/website/assets/site.js" defer></script>
  </head>
  <body class="reader-body docs-home-body">
    <header class="site-header">
      <a class="brand" href="/" aria-label="Refine home">
        <img src="/src/surfaces/web/static/images/refine_logo_transparent.png" alt="">
      </a>
      <div class="site-menu-shell">
        <button
          class="menu-button"
          type="button"
          data-menu-toggle
          aria-controls="site-menu"
          aria-expanded="false"
          aria-label="Open navigation menu"
        >
          <svg aria-hidden="true" viewBox="0 0 24 24">
            <path d="M4 7h16"></path>
            <path d="M4 12h16"></path>
            <path d="M4 17h16"></path>
          </svg>
        </button>
        <div class="site-menu" id="site-menu" data-menu-panel hidden>
          <nav aria-label="Primary">
            <a href="/">Home</a>
            <a href="/#product">Product</a>
            <a href="/#start">Get Started</a>
            <a href="/docs" aria-current="page">Docs</a>
            <a href="/read/docs/runbooks/install.md">Install</a>
            <a href="https://github.com/buwilliams/refine">GitHub</a>
          </nav>
          <div class="menu-docs" aria-label="Documentation sections">
            {nav}
          </div>
        </div>
      </div>
    </header>
    <main class="docs-home-shell">
      <section class="docs-hero" aria-labelledby="docs-home-title">
        <p class="eyebrow">Product Guide</p>
        <h1 id="docs-home-title">How Refine works.</h1>
        <p>
          Refine is a local operating layer for agentic software delivery. It turns product goals
          into durable work, gives agents enough context to act, keeps processes observable, and
          uses Git, review, governance, and quality checks to make automation inspectable.
        </p>
        <div class="hero-actions" aria-label="Docs starting points">
          <a class="button primary" href="/read/docs/intent/01-design.md">Read the system design</a>
          <a class="button secondary" href="/read/docs/runbooks/install.md">Install with an agent</a>
        </div>
      </section>

      <section class="docs-orientation" aria-labelledby="docs-orientation-title">
        <div>
          <p class="eyebrow">The Model</p>
          <h2 id="docs-orientation-title">One product, three layers.</h2>
        </div>
        <div class="docs-orientation-grid">
          <article>
            <span>01</span>
            <h3>Foundation</h3>
            <p>Node, model, and target-app concepts keep state durable, local, and readable.</p>
          </article>
          <article>
            <span>02</span>
            <h3>Capabilities</h3>
            <p>Process, agents, and workflow are shared powers that every surface can use.</p>
          </article>
          <article>
            <span>03</span>
            <h3>Surfaces</h3>
            <p>CLI, browser, API, MCP, and agent interfaces adapt the same underlying system.</p>
          </article>
        </div>
      </section>

      <section class="docs-map" aria-labelledby="docs-map-title">
        <div class="section-heading">
          <p class="eyebrow">Browse</p>
          <h2 id="docs-map-title">Choose the path that matches what you are trying to understand.</h2>
        </div>
        {groups}
      </section>
    </main>
  </body>
</html>"#
        )
        .into_bytes(),
    )
}

fn render_docs_nav(current_path: &str) -> String {
    let docs_home_current = if current_path == "docs" {
        r#" aria-current="page""#
    } else {
        ""
    };
    let mut html = format!(
        r#"<h2>Docs</h2>
        <a class="docs-home-link" href="/docs"{docs_home_current}>Product Guide</a>"#,
    );
    for group in DOCS_NAV_GROUPS {
        html.push_str(&format!(
            r#"<section class="docs-nav-group"><h3>{}</h3><ul>"#,
            escape_html(group.title)
        ));
        for item in group.items {
            let current = if item.path == current_path {
                r#" aria-current="page""#
            } else {
                ""
            };
            html.push_str(&format!(
                r#"<li><a href="/read/{}"{}>{}</a></li>"#,
                escape_html(item.path),
                current,
                escape_html(item.title)
            ));
        }
        html.push_str("</ul></section>");
    }
    html
}

fn render_docs_landing_groups() -> String {
    let mut html = String::from(r#"<div class="docs-group-list">"#);
    for group in DOCS_NAV_GROUPS {
        html.push_str(&format!(
            r#"<section class="docs-group-card" aria-labelledby="docs-group-{}"><h3 id="docs-group-{}">{}</h3><div class="docs-link-list">"#,
            slugify_docs_id(group.title),
            slugify_docs_id(group.title),
            escape_html(group.title)
        ));
        for item in group.items {
            html.push_str(&format!(
                r#"<a class="docs-link-card" href="/read/{}"><strong>{}</strong><span>{}</span></a>"#,
                escape_html(item.path),
                escape_html(item.title),
                escape_html(item.summary)
            ));
        }
        html.push_str("</div></section>");
    }
    html.push_str("</div>");
    html
}

fn docs_nav_items() -> Vec<DocsNavItem> {
    docs_nav_entries()
        .map(|entry| DocsNavItem {
            title: entry.title.to_string(),
            path: entry.path.to_string(),
        })
        .collect()
}

fn docs_title_for_path(path: &str) -> String {
    docs_nav_entries()
        .find(|entry| entry.path == path)
        .map(|entry| entry.title.to_string())
        .unwrap_or_else(|| path.to_string())
}

fn render_docs_pager(path: &str, items: &[DocsNavItem]) -> String {
    let current_index = items.iter().position(|item| item.path == path);
    let previous = current_index
        .and_then(|index| index.checked_sub(1))
        .and_then(|index| items.get(index));
    let next = current_index.and_then(|index| items.get(index + 1));
    format!(
        r#"<nav class="doc-pager" aria-label="Document navigation">
          {}
          <a class="doc-pager-toc" href="/docs">Docs home</a>
          {}
        </nav>"#,
        render_docs_pager_edge("Previous", previous),
        render_docs_pager_edge("Next", next)
    )
}

fn render_docs_pager_edge(label: &str, item: Option<&DocsNavItem>) -> String {
    match item {
        Some(item) => format!(
            r#"<a class="doc-pager-link" href="/read/{}"><span>{}</span><strong>{}</strong></a>"#,
            escape_html(&item.path),
            label,
            escape_html(&item.title)
        ),
        None => format!(
            r#"<span class="doc-pager-link doc-pager-link-disabled" aria-disabled="true"><span>{label}</span><strong>{}</strong></span>"#,
            if label == "Previous" { "Start" } else { "End" }
        ),
    }
}

fn markdown_for_website(path: &str, markdown: &str) -> String {
    if path != "docs/intent/README.md" {
        return markdown.to_string();
    }
    let body = markdown
        .find("\n## Key Ideas")
        .map(|index| &markdown[index + 1..])
        .unwrap_or(markdown);
    format!(
        "# Design Intent\n\nThese are the durable product principles behind Refine. The full source tree still keeps its GitHub-friendly table of contents, but this web version starts with the ideas a product reader needs first.\n\n{body}"
    )
}

fn slugify_docs_id(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn render_markdown_fragment(path: &str, markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser =
        Parser::new_ext(markdown, options).map(|event| rewrite_markdown_event(path, event));
    let mut rendered = String::new();
    html::push_html(&mut rendered, parser);
    rendered
}

fn rewrite_markdown_event<'a>(path: &str, event: MarkdownEvent<'a>) -> MarkdownEvent<'a> {
    match event {
        MarkdownEvent::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => MarkdownEvent::Start(Tag::Link {
            link_type,
            dest_url: CowStr::from(rewrite_markdown_link(path, dest_url.as_ref())),
            title,
            id,
        }),
        other => other,
    }
}

fn rewrite_markdown_link(path: &str, destination: &str) -> String {
    if destination.starts_with("http:")
        || destination.starts_with("https:")
        || destination.starts_with("mailto:")
        || destination.starts_with('#')
        || destination.starts_with('/')
    {
        return destination.to_string();
    }

    let (target, anchor) = destination
        .split_once('#')
        .map(|(target, anchor)| (target, format!("#{anchor}")))
        .unwrap_or((destination, String::new()));
    if !target.ends_with(".md") {
        return destination.to_string();
    }

    let base = path.rsplit_once('/').map(|(base, _)| base).unwrap_or("");
    let joined = if base.is_empty() {
        target.to_string()
    } else {
        format!("{base}/{target}")
    };
    match normalize_markdown_path(&joined) {
        Some(normalized) => format!("/read/{normalized}{anchor}"),
        None => destination.to_string(),
    }
}

fn normalize_markdown_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            value => parts.push(value),
        }
    }
    Some(parts.join("/"))
}

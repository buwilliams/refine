# Configure Templates

Templates control the context and instructions Refine sends to agents. Open **Settings → Templates** to edit Workflow, Planning Agent, Agent, Goal Agent, or a specialized launch, completion, repair, or transport template. Entries have fixed names and cannot be created or deleted. An empty template is allowed.

The editor shows the built-in default, available variables, and a preview using sample values. Saving checks the revision you opened; if another user saved first, your draft stays open with a conflict message. Without an attached project, defaults can be viewed and previewed.

## Variables

Use `{{skill}}` for the selected Skill, `{{refine_executable}}` for the absolute Refine executable on the executing node, and `{{current_round_goal}}` for the current Round's request. `{{templates.workflow-context}}` includes another Template. The variable picker and `refine templates variables` list the available names. Operation-specific values are supplied by the launch using that template; a required value unavailable in that launch produces a rendering error. Shared optional values are empty outside their applicable context.

Skills and referenced Templates expand their own variables. For example, a Workflow containing `{{skill}}` can include a Skill containing `Run {{refine_executable}} --help`. User messages, Goal text, paths, and other task data remain literal, even when they contain braces. Write `\{{skill}}` in a Template to produce literal `{{skill}}` text. Unknown placeholders in Skills remain literal for compatibility; unknown placeholders in Templates are rejected. Cycles, excessive nesting, and excessive expansion produce errors.

A preview accepts ordinary JSON values as literal data. Use `{"template":"Run {{refine_executable}}"}` for a sample value that should expand as template text.

## CLI and API

```sh
refine templates list
refine templates show workflow
refine templates variables
refine templates save workflow --revision 0 --json '{"prompt":"{{templates.workflow-context}}\n\n{{skill}}"}'
refine templates preview workflow --json '{"values":{"skill":{"template":"Use {{refine_executable}}"}}}'
```

The same operations are available through `GET /api/templates`, `GET /api/templates/:id`, `PUT /api/templates/:id`, and `POST /api/templates/:id/preview`. MCP uses the same daemon API. Previewing never launches an agent.

## Storage and execution

Each edited entry is stored in `templates/<id>.json` within the attached project's Refine state and participates in normal `refine/state` commit and synchronization. Missing files use built-in defaults without creating overrides. Invalid saved files are errors, rather than silently reverting to defaults.

Workflow invocations retain their Template sources and revisions. Retries use that snapshot; later invocations use newly saved Templates. Each chat turn takes a new snapshot. Runtime values, including the executable path, come from the executing node. Provider process metadata retains the snapshot and rendered prompt for managed noninteractive launches.

All Refine-authored prompt instructions belong to editable Templates or Skills. Outer templates decide which context, completion contracts, and repair instructions to include. Removing response instructions does not remove backend response validation: an agent returning an invalid response still fails normally. Provider-owned instructions and existing provider conversation history are outside Refine's Templates.

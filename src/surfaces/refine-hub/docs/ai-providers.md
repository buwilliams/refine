# Configure AI providers

Settings > Runtime lets you add CLI agents and edit their executable paths and arguments. Supporting another CLI does not require rebuilding Refine.

The provider catalog and system default belong to shared Refine state (`providers.json`). A node stores only its optional `agent_cli` selection. Explicit invocation and retained-session selections take precedence, followed by the node selection, then the system default. Choose **Use system default** to remove the node override. Unrelated Runtime settings saves preserve inheritance.

Claude, Codex, Gemini, Copilot, and Smoke AI are supplied as initial definitions, with Claude as the system default. Defaults remain virtual until edited; existing generic executable selections are migrated without changing their case or path. Existing catalogs, including catalogs adopted from another node, are preserved.

## Add a CLI through the command line

Read the complete catalog and its revision:

```sh
refine config providers show > providers-response.json
```

Extract the `catalog` object, add a provider, and save it. For example, using `jq`:

```sh
jq '.catalog | .providers += [{
  "id": "MyAgent",
  "name": "My CLI agent",
  "executable": "/opt/My Agent/bin/agent",
  "automated": {"args": ["run", "--prompt", "{{context}}"]},
  "interactive": {"args": ["--prompt", "{{context}}"]},
  "output_format": "plain"
}]' providers-response.json > providers.json
refine config providers save --file providers.json
refine config providers select MyAgent
```

Change `default_provider` in the catalog to set the shared system default. Run `refine config providers select` with no ID to clear the current node override. `refine config settings set --json '{"agent_cli":null}'` does the same thing.

The API uses `GET /api/providers`, `PUT /api/providers` (complete catalog), and `PATCH /api/settings` (node override). Reads return the catalog, raw `node_override`, `effective_provider`, and `selection_source`. Saves require the revision returned by the read. A stale edit returns a conflict; retain your draft and reapply it to a fresh catalog. A provider referenced by the default or any node cannot be deleted until those references are changed or cleared.

## Launch templates

Each argument is a separate JSON string. Refine executes the configured executable directly, with no shell evaluation or splitting. Paths may contain spaces. `$()`, quotes, newlines, Unicode, and template syntax inside context stay literal.

- `args`: ordered base arguments.
- `context_args`: appended when context is nonempty. Use `["--prompt", "{{context}}"]`, for example.
- `cwd_args`: arguments appended when an explicit automated launch working directory is supplied, before `context_args`. Refine sets the process working directory independently of these optional flags.
- `resume_args`: optional complete replacement for `args` when a retained session ID is supplied. Context arguments still follow. `cwd_on_resume` controls whether cwd arguments also apply (true by default; false for Codex).
- `pin_args`: optional prefix for an interactive CLI that can start a session with a caller-chosen ID.

`{{context}}` is the assembled Refine launch context. It is substituted once. `{{session_id}}` is available in resume and pin templates. `{{cwd}}` represents the supplied automated launch directory; it is empty if none was supplied. Interactive commands do not receive a cwd template value.

The default `inline_or_file` transport puts prepared context in arguments. Large contexts retain Refine's managed file fallback and artifact cleanup: the argument then instructs the CLI to read the complete context from that file. The agent needs file-reading capability for this fallback.

For an automated CLI accepting native stdin, set `transport` to `native_stdin`, set `stdin` to `"{{context}}"`, and keep context out of the argument templates. For example:

```json
{"args":["run","-"],"transport":"native_stdin","stdin":"{{context}}"}
```

Interactive PTY launches use `inline_or_file`; stdin remains available for terminal interaction. Custom providers default to plain output without session continuity. Built-in parsers are available as `claude_json`, `codex_json`, and `copilot_json`. Use a parser only when the executable produces that format.

Definitions can be saved from a host where the executable is unavailable. Each launching host must install the executable and configure its authentication. An unavailable or unknown selected provider produces an error. Editing a definition affects subsequent launches; running processes retain their launch configuration.

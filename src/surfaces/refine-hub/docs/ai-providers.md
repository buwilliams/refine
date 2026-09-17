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

## Settings editor and two configurations of one CLI

In **Settings > Runtime > Shared AI providers**, choose **Add provider**, enter a name and executable, and add one row per argument. Refine suggests a stable ID; it stays fixed when editing. For example, create `local-careful` and `local-fast`, both using `/opt/agent/bin/agent`, with arguments `--model`, `careful` and `--model`, `fast` respectively. Each pair occupies two rows. Spaces, empty arguments, and line breaks stay within their own argument. The editor never splits a command line or requires JSON.

Context arguments follow ordinary arguments when a request has context. Choose argument delivery or standard input. Choosing standard input moves the default context argument to the input template. Remove any additional `{{context}}` argument templates when using standard input. Interactive terminal settings, working directory flags, session arguments, credential references, and output adapters are expandable advanced options.

The shared system default and **AI provider for this node** have separate save controls. Select **Use system default** for the node to restore inheritance. The effective provider and selection source appear beside the shared list. Explicit invocation and retained-session provider selections still take precedence. Running processes keep their prepared configuration.

Unsaved provider edits and selection changes are retained in this browser, separately for each project and node. After a reload, choose **Resume unsaved provider draft**. Cancel or Escape closes the editor while retaining edits. Failed saves leave the form open. A revision conflict offers **Load latest and reapply draft**: unrelated remote changes are preserved; overlapping fields require choosing the draft or latest value before saving. Default conflicts offer **Review latest default** before a deliberate retry. Deletion reference errors identify the selection to clear.

## Local credential references

In **Credentials and output**, map a child environment variable to the name of a variable in the launching process's local environment. For example:

```json
"credentials": {"OPENAI_API_KEY": "MY_AGENT_KEY"}
```

Set `MY_AGENT_KEY` locally in the daemon or CLI launch environment using the host's normal secret provisioning. Restart the daemon after changing its environment. Enter names only in Settings. Refine stores references in the shared catalog, resolves them on the launching host, and injects the value into the child environment after its normal removal of implicit API credentials. This is an explicit opt-in to that provider's authentication and billing mode, not an automatic fallback.

Missing references fail before launch with the missing variable's name. Credential values are excluded from launch debug representations and literal echoed values are redacted before output callbacks, transcripts, and captured logs. Redaction handles values split across reads; it cannot recognize arbitrary transformations or encodings performed by a provider. Configure trusted executables. Detached launches with explicit credential references currently require Linux scope capture; other hosts report the unsupported capability before starting.

Native sessions remain associated with their selected provider. New local session compatibility receipts retain only a hash of the launch definition; changing executable, arguments, or credential references requires a new session or restoring the previous definition. Renaming a provider is compatible. Legacy sessions without receipts keep their existing behavior. Credential value rotation is host-local and is not stored or compared.

A failed or limited provider uses existing failure handling. Users may repair its configuration or choose another provider for a subsequent launch. Refine does not automatically select another configuration, spend against a fallback budget, or transfer native sessions between providers.

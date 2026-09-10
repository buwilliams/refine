# Configure Skills

Use **Controls → Settings → Skills** to configure repeatable agent work. Each Skill has one trigger, plain-text instructions, an enabled state, project or node scope, and optional typed inputs. Choose **Custom** for a manually runnable action, or a workflow or lifecycle point for automatic work. Events remain internal; there is no separate Events editor or CLI group.

Refine supplies Plan, Implement, Quality, and Governance Skills at their respective Enter triggers. Refine determines the expected result from the trigger. Plan and Implement require an enabled required Skill. Quality and Governance pass when no enabled required Skills apply, recording that no agent checks ran while retaining candidate and integration checks. The default Plan Skill chooses its own planning method.

## Discover and edit

```sh
refine skills triggers
refine skills list
refine skills show default-quality
```

In the browser, click a Skill row to edit it or use its Status toggle to enable or disable it directly. The modal shows rendered instructions first; use the edit icon to change their Markdown. Open **Skill settings** below to change the name, trigger, scope, status, inputs, and workflow options. **Clone Skill** copies instructions and inputs into an independent new Skill; choose a different trigger for the copy.

For CLI configuration, create a JSON file such as:

```json
{
  "name": "Release check",
  "prompt": "Inspect the release and report the observed evidence.",
  "parameters": [{"name":"subject","kind":"text","required":true,"default":"release"}],
  "trigger": {"source":"custom"}
}
```

Save using the revision from the latest list or show response:

```sh
refine skills save release-check --revision 1 --file skill.json
refine skills clone release-check quality-release --name "Quality release check" --trigger workflow.quality.enter
```

A Skill and its trigger save atomically. Stale revisions return a conflict and preserve the current configuration. A status or instruction update that omits the trigger keeps the existing assignment. Deleting a Skill removes its assignment without rewriting historical runs.

Automatic triggers support:

- **Required** (`blocking`): every required result must pass before normal advancement.
- **Background** (`background`): its verdict does not gate progression. Checkout serialization still applies.
- **Context only** (`context`): attach these instructions to other agents at the same trigger without launching another agent.

Optional input mappings refer to paths such as `goal.rounds.0.prompt` or `system.node_id`. Explicit manual input takes precedence over context and defaults. Lower order values run first when several Skills use the same automatic trigger.

## Run and inspect

Controls → Skills and the command palette list enabled Custom Skills for the selected node. **Add skill...** opens New Skill from any screen. A launch form collects typed inputs and defaults, then opens the selected Skill in an agent tab. The tab provides the normal live terminal, retained transcript, reconnect, and Stop controls. Opening a Goal does not attach that Goal to a manual Skill run.

CLI runs use the supervised task executor:

```sh
refine skills trigger release-check --param subject=release --request-id release-check-001
refine skills runs
refine skills status INVOCATION_ID
refine skills cancel INVOCATION_ID
```

Reuse a request ID only to retry the same Skill request. Different parameters or node return a conflict. Interactive terminals prompt for missing required values; unattended calls return an error. Automatic missing inputs record an execution error.

Headless invocation history retains selected configuration, inputs, results, process references, and invalid-response diagnostics. Web agent tabs retain managed session and process evidence. Quality retains supervised checks and exact candidate proof. Cancellation prevents later launches and rejects late completion of cancelled work.

## Operational Skills

The example files in [skills](skills/) provide **Release**, **Fetch Goals from Email**, and **Fetch Goals from Email on startup**. They are editable project configuration, not global defaults. Save each through `refine skills save ID --revision REVISION --file FILE`, reading the current revision before each save. The startup Skill queues the Custom fetch Skill with an occurrence-specific request ID and exits immediately; it must not wait for the child agent while holding an execution slot.

System context supplies `project_root`, `workspace`, `node_id`, `runtime_root`, `refine_executable`, and, when discoverable, `refine_checkout`. Use these values to keep maintenance commands tied to the intended app and running installation.

## Migration and recovery

Initial migration converts Governance and Quality policy into default Skill prompts and Guidance into context Skills. Original configuration is archived in `automation/migration-v1.json`. Conversion from the multiple-trigger model archives `automation/migration-v2.json`, then creates an independent Skill for each assignment while preserving prompts, parameters, ordering, effective scope, enabled state, and legacy node overrides. Binding identities and historical invocation snapshots remain intact. Conversion is deterministic, revision-fenced, and safe to retry.

Use supported configuration surfaces after migration. Legacy files do not regain authority. Pristine defaults on a freshly attached node can adopt synchronized project configuration; authored edits retain the normal conflict boundary. Do not repair configuration by deleting Goal history or rewriting results.

The Skill API provides `/skills`, `/skills/catalog`, `/skills/{id}`, `/skills/{id}/inputs`, and `/skills/{id}/trigger`. Saves accept `{revision,item,trigger}`. The browser uses an explicit `skill` profile on `/terminal/session`. Internal Event and invocation routes remain for system execution and compatibility; `/events` retains its streaming meaning. MCP exposes Skill listing, trigger discovery, and manual launch, with generic requests for editing and run control.

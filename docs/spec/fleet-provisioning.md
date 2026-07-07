# Fleet Provisioning Spec

## Summary

Fleet provisioning turns a registered node into a working machine on a cloud
provider. It implements the provisioning half of
`docs/intent/02-foundation/04-fleet.md`: nodes and their agents are creatable,
credentialed, and disposable without manual setup on each machine, and the
same operation works on any infrastructure through configuration.

Provisioning is **data-driven**. A provider is a set of argv command templates
plus defaults, resolved from `.refine/fleet.json` layered over built-in
providers. The control binary renders templates and executes them through the
process supervisor; it has no provider SDKs and no hardcoded deployment logic.
This is what lets the latest released Refine provision workers running the
next version: the worker builds itself from a Git ref, and provisioning
behavior ships as configuration a newer Refine (or an operator) can update
without a new control binary.

## Deployment model (Fly.io, built in)

Each fleet node maps to one Fly.io app (default name `refine-{node_id}`).
Provisioning runs two steps through the local `fly` CLI:

1. `fly apps create {app_name} --org {org} --yes` — marked `allow_failure` so
   re-provisioning an existing app proceeds to deploy.
2. `fly deploy --app {app_name} --config {fleet_dir}/fly.worker.toml
   --dockerfile {fleet_dir}/Dockerfile --build-arg REFINE_REF={refine_ref}
   --build-arg REFINE_REPO_URL={repo_url} --regions {region}
   --vm-size {vm_size} --vm-memory {vm_memory} --remote-only --yes`

`{fleet_dir}` resolves to `scripts/fleet/fly/` inside the local Refine
checkout. The worker image (`scripts/fleet/fly/Dockerfile`) clones the Refine
repository at `REFINE_REF` (branch, tag, or SHA), builds it with
`cargo build --release --locked`, writes the standard `.refine-deployed`
marker, and starts the daemon in the foreground. Workers are ephemeral by
design: durable state lives in the target repo's `.refine/` directory and
syncs through the shared Git remote, never on the worker.

Workers are private by default — `fly.worker.toml` defines no public HTTP
service. Reach a worker's web UI with `fly proxy 8080 --app <app-name>`, or
expose it deliberately behind your own auth layer.

Deprovision destroys the app (`fly apps destroy {app_name} --yes`), disables
the node, and records `deprovisioned` health. Status runs
`fly status --app {app_name} --json` and refreshes node health.

## Configuration schema — `.refine/fleet.json`

Optional. When absent, built-in providers (currently `fly`) are available with
their defaults. When present, entries with the same name fully replace the
built-in definition.

```json
{
  "schema_version": 1,
  "default_provider": "fly",
  "providers": {
    "fly": {
      "display_name": "Fly.io",
      "binary": "fly",
      "credential_env": ["FLY_API_TOKEN"],
      "require_credentials": false,
      "defaults": {
        "org": "personal",
        "region": "iad",
        "vm_size": "shared-cpu-2x",
        "vm_memory": "2048",
        "repo_url": "https://github.com/buwilliams/refine.git",
        "refine_ref": "main"
      },
      "provision": [
        {"argv": ["{binary}", "apps", "create", "{app_name}", "--org", "{org}", "--yes"], "allow_failure": true},
        ["{binary}", "deploy", "--app", "{app_name}", "..."]
      ],
      "deprovision": [["{binary}", "apps", "destroy", "{app_name}", "--yes"]],
      "status": [["{binary}", "status", "--app", "{app_name}", "--json"]]
    }
  }
}
```

Rules:

- `schema_version` must be ≤ the version the binary supports (currently `1`).
  A newer schema is rejected with guidance to update Refine first; unknown
  fields are ignored, so additive evolution does not break released binaries.
- Command steps are argv arrays (no shell interpretation). A step may be a
  bare array or `{"argv": [...], "allow_failure": true}`.
- `{placeholder}` tokens must all resolve; a typo fails loudly before anything
  executes. Computed placeholders: `node_id`, `refine_port`, `binary`,
  `fleet_dir`, and a default `app_name` of `refine-{node_id}`. Provider
  `defaults` and per-node `provisioning` values may reference earlier
  placeholders (for example `"app_name": "acme-{node_id}"`).

## Node schema additions

`Node` gains two optional, backward-compatible fields persisted in
`.refine/nodes.json`:

- `provider` — which fleet provider manages this node (empty for manual/SSH
  nodes; stamped automatically on first provision).
- `provisioning` — a JSON object of per-node placeholder overrides
  (`region`, `app_name`, `refine_ref`, `require_credentials`, and any custom
  keys a provider template uses). Node overrides win over provider defaults.

Provisioning outcomes write `Node.health` (`ready`, `failed`, or
`deprovisioned`) with the executed steps recorded in `health.details.fleet`,
the same shape cluster bootstrap uses.

## Credentials follow policy

Secrets are never written to shared state or Git. Each provider declares
`credential_env`: variables read from the invoking environment and passed
through to the provider process only. Two postures per the fleet intent:

- **Subscription login** (default for `fly`): the provider CLI uses its own
  stored auth (e.g. `fly auth login`); `require_credentials` stays `false`.
- **Metered keys for ephemeral workers**: set `require_credentials: true` on
  the provider, or `"require_credentials": true` in a node's `provisioning`,
  and provisioning refuses to run without the declared env vars.

Every executed step is also authorized through the security service
(`allowed_commands` in settings) and audited, like `cluster run`.

## Distribute

Distribute is the mechanism for moving work between nodes — an operation, not
a scheduler. `cluster distribute`:

- **spread** (default): reassigns eligible Gaps — backlog/todo, unclaimed, not
  Feature-bound — across enabled nodes whose last reported health is not
  `failed`/`deprovisioned`, balancing per-node load deterministically.
- **fill** (`--to <node-id>`): moves all eligible Gaps to one node.
- **converge** (`--converge --to <node-id>`): distribution pointed home —
  moves reviewable (review-status) Gaps to the review node where judgment
  happens.

Reassigning ownership of unclaimed work is the one sanctioned exception to
node ownership enforcement. Gaps with active claims are pinned; Feature-bound
gaps are skipped (transfer the Feature to move them as a unit). `--dry-run`
returns the planned moves without writing.

## Surfaces

CLI (proxied through the daemon, or direct with `--target-root` in tests):

```bash
./r cluster providers
./r cluster edit-node <id> --provider fly --provisioning '{"region": "syd"}'
./r cluster provision <id> [--provider <name>] [--dry-run]
./r cluster provision-status <id>
./r cluster deprovision <id> [--dry-run]
./r cluster distribute [--to <node-id>] [--converge] [--dry-run]
```

HTTP (under the existing `/cluster` group):

- `GET /cluster/providers`
- `POST /cluster/nodes/<id>/provision` — body `{provider?, dry_run?}`
- `POST /cluster/nodes/<id>/deprovision` — body `{dry_run?}`
- `POST /cluster/nodes/<id>/provision-status`
- `POST /cluster/distribute` — body `{to?, converge?, dry_run?}`

MCP reaches all of these through the generic `refine_request` tool, matching
how other cluster operations are exposed.

## Adding a provider

No code required. Add an entry to `.refine/fleet.json` with the provider's
CLI binary and command templates (see the `droplet`/`doctl` example in
`src/tools/host/fleet/mod.rs` tests), optionally add worker assets under
`scripts/fleet/<provider>/` and reference them via `{fleet_dir}`, then allow
the binary in settings `allowed_commands`. Set it as `default_provider` or
per-node via `cluster edit-node <id> --provider <name>`.

## Code map

- `src/model/fleet/mod.rs` — config schema, command steps, placeholder
  rendering, schema-version gate.
- `src/tools/host/fleet/mod.rs` — `FileFleetService`: config layering,
  credential posture, supervised execution, node health writes, built-in
  providers.
- `src/tools/product/work_items/service.rs` — `distribute_gaps_across_nodes`.
- `src/tools/host/cluster/mod.rs` — `distribute_response`, healthy-node
  targeting, claim pinning.
- `scripts/fleet/fly/` — worker Dockerfile and Fly config.

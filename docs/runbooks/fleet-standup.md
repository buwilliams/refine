# Runbook: Stand Up a Fleet Worker

Outcome: a cloud machine running Refine, registered as a node, owning work
your user distributes to it, with an agent CLI installed and credentialed.

## Preconditions

- A project is attached (`refine project status` shows a target root) and its
  repo has a shared Git remote (`origin`) both this machine and workers can
  reach. Durable state syncs through that remote.
- The provider CLI is installed and authenticated on this machine. For the
  built-in Fly.io provider: `fly version` works and `fly auth whoami` shows
  the user's account.
- If settings restrict `allowed_commands`, the provider binary (`fly`) must
  be allowed.

## Ask the user first

1. Which cloud org/account and region? (defaults: Fly `personal`, `iad`)
2. VM size and budget posture — Rust or large builds need
   `performance-2x`/4096 MB or bigger; the default is `shared-cpu-2x`/2048.
3. Credential posture for the worker's agent: metered API key (recommended
   for ephemeral workers) or none for now. The key is read from the
   environment of the process that executes provisioning — the daemon's
   environment when going through the daemon (start it with the variable
   set), or the shell's when running with a direct target root. Without it
   the secrets step is skipped and the worker has no agent credentials until
   you set them at the provider (e.g. `fly secrets set ANTHROPIC_API_KEY=…`).
4. Which Refine version should the worker run? `refine_ref` accepts a branch,
   tag, or commit (default `main`).

## Commands

```bash
refine cluster providers                 # confirm which provider will drive
refine cluster add-node worker-1
refine cluster edit-node worker-1 --provider fly \
  --provisioning '{"region": "iad", "vm_size": "shared-cpu-2x", "vm_memory": "2048"}'
refine cluster provision worker-1 --dry-run   # show the user what will run
refine cluster provision worker-1             # ~3 minutes on Fly
```

Provisioning creates the app, injects worker identity and the API key as
provider secrets (values never appear in logs or state), and deploys an image
that builds Refine from `refine_ref`, installs the agent CLI, and runs
`refine node init` at boot — cloning the target repo, taking on the node
identity, and selecting the agent provider.

## Verify

```bash
refine cluster show worker-1               # health.status == "ready"
refine cluster provision-status worker-1   # provider-side machine state
refine cluster run worker-1 "refine system status"   # daemon up on the worker
refine cluster run worker-1 "refine agent detect"    # agent CLI present + authed
```

`cluster run` reaches provider-managed nodes through the provider's exec
channel (Fly: `fly ssh console`); no SSH setup is needed.

## Undo

```bash
refine cluster deprovision worker-1   # destroys the cloud app, disables the node
```

Workers are ephemeral by design — deprovisioning loses nothing that is not
already in the shared Git remote.

## Common failures

- `provision` step fails with "not authorized": add the provider binary to
  settings `allowed_commands`.
- Worker boots but `node init` reports `clone_target_repo` failed: the target
  repo URL is not reachable from the worker (private repo without
  credentials). Give the worker a readable URL or a deploy token, then
  `refine cluster run worker-1 "refine node init"`.
- Machine lands in an unexpected region: set `primary_region` via the node's
  provisioning or accept the provider's placement.

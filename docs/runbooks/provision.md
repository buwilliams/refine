# Provision a Fleet Worker

Use this runbook when a user wants a Refine worker on infrastructure outside
the current machine. Refine owns node identity, work distribution, workflow,
and Git synchronization. The operating agent owns provider-specific machine
creation and deletion.

This runbook includes a Fly.io recipe. For another provider, preserve the same
contract: deploy the Refine worker image, inject the required `REFINE_*`
environment variables, keep the worker private, and verify `refine node init`
and the daemon before distributing work.

## Preconditions

- A target project is attached and has an `origin` remote where Refine can publish its dedicated `refine/state` branch and implementation branches.
- The target repository is reachable from the worker, including credentials
  for a private repository.
- The provider CLI is installed and authenticated on the control machine.
- The user has approved the account, region, machine size, and expected cost.
- The selected agent provider CLI and credential posture are known.

## Register the node

Choose a stable lowercase node id before creating the machine. The daemon
publishes this durable state automatically:

```bash
refine cluster add-node worker-1
```

## Fly.io recipe

Run these commands from the Refine source checkout. Adjust every value with the
user before executing it:

```bash
export NODE_ID=worker-1
export APP_NAME="refine-$NODE_ID"
export FLY_ORG=personal
export FLY_REGION=iad
export FLY_VM_SIZE=shared-cpu-2x
export FLY_VM_MEMORY=2048
export REFINE_REF=main
export REFINE_REPO_URL=https://github.com/buwilliams/refine.git
export TARGET_ROOT=/path/to/attached/target-app
export TARGET_REPO_URL="$(git -C "$TARGET_ROOT" remote get-url origin)"
export AGENT_PROVIDERS=claude

fly apps create "$APP_NAME" --org "$FLY_ORG" --yes

# Required worker identity and work source. These are not optional credentials.
fly secrets set --app "$APP_NAME" --stage \
  "REFINE_NODE_ID=$NODE_ID" \
  "REFINE_TARGET_REPO_URL=$TARGET_REPO_URL" \
  "REFINE_AGENT_PROVIDERS=$AGENT_PROVIDERS"

# Set only credentials the user approved and exported in this shell.
if [ -n "${ANTHROPIC_API_KEY:-}" ]; then
  fly secrets set --app "$APP_NAME" --stage \
    "ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY"
fi
if [ -n "${OPENAI_API_KEY:-}" ]; then
  fly secrets set --app "$APP_NAME" --stage \
    "OPENAI_API_KEY=$OPENAI_API_KEY"
fi
if [ -n "${GEMINI_API_KEY:-}" ]; then
  fly secrets set --app "$APP_NAME" --stage \
    "GEMINI_API_KEY=$GEMINI_API_KEY"
fi

fly deploy \
  --app "$APP_NAME" \
  --config scripts/fleet/fly/fly.worker.toml \
  --dockerfile scripts/fleet/fly/Dockerfile \
  --build-arg "REFINE_REF=$REFINE_REF" \
  --build-arg "REFINE_REPO_URL=$REFINE_REPO_URL" \
  --build-arg "AGENT_PROVIDERS=$AGENT_PROVIDERS" \
  --regions "$FLY_REGION" \
  --vm-size "$FLY_VM_SIZE" \
  --vm-memory "$FLY_VM_MEMORY" \
  --ha=false \
  --remote-only \
  --yes
```

No agent API key is required to create a worker. Without one, node identity,
repository attachment, synchronization, and daemon operation still work; agent
execution waits until an appropriate credential or subscription login exists.

## Verify

```bash
fly status --app "$APP_NAME"
fly ssh console --app "$APP_NAME" --command "refine system status --port 8080"
fly ssh console --app "$APP_NAME" --command "refine node list"
fly ssh console --app "$APP_NAME" --command "refine agent detect"
```

Confirm that the active node is `worker-1`, the target project is attached, and
the registered fleet state appears on the worker. Only then distribute work:

```bash
refine cluster distribute --dry-run
refine cluster distribute
```

## Undo

Move reviewable or open work away from the worker before destroying it:

```bash
refine cluster distribute --to default --dry-run
refine cluster distribute --to default
refine cluster disable-node "$NODE_ID"
fly apps destroy "$APP_NAME" --yes
```

## Common failures

- `node init` cannot clone the target: provide repository credentials the
  worker can use; interactive Git prompting is disabled.
- The daemon starts without a project: inspect the required `REFINE_*` secrets
  and rerun `refine node init` inside the worker.
- The agent CLI exists but cannot execute: install or inject the selected
  provider's authentication without changing the node identity secrets.
- Fleet state does not appear: inspect `refine system status` and the System
  process diagnostics. Reconciliation retries automatically; users should not
  edit, stash, commit, or force-push application files to repair Refine state.

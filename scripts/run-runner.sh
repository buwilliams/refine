#!/usr/bin/env bash
# Start the refine-runner natively on the host.
#
# Required env vars:
#   REFINE_VOLUME_ROOT   - host path of the volume root (inside the client repo)
#   REFINE_CLIENT_REPO   - host path of the client repository
#   REFINE_RUNNER_SOCKET - Unix socket path (must be visible to the webapp container)
#
# Example:
#   export REFINE_CLIENT_REPO=/srv/clients/acme-app
#   export REFINE_VOLUME_ROOT=/srv/clients/acme-app/refine
#   export REFINE_RUNNER_SOCKET=/var/run/refine/runner.sock
#   ./scripts/run-runner.sh
set -euo pipefail

: "${REFINE_VOLUME_ROOT:?Set REFINE_VOLUME_ROOT}"
: "${REFINE_CLIENT_REPO:?Set REFINE_CLIENT_REPO}"
: "${REFINE_RUNNER_SOCKET:?Set REFINE_RUNNER_SOCKET}"

socket_dir="$(dirname "$REFINE_RUNNER_SOCKET")"
mkdir -p "$socket_dir" "$REFINE_VOLUME_ROOT"

cd "$(dirname "$0")/.."

# Run from repo root so the package imports resolve.
exec python3 -m refine_runner

#!/usr/bin/env bash
# Start the refine-runner. Run from inside the client repo; refine.toml is
# discovered by walking up from cwd.
set -euo pipefail

# If invoked from anywhere, switch to the repo containing this script so the
# Python packages resolve. The refine.toml is still discovered from the
# caller's working directory before we cd.
caller_cwd="$(pwd)"
cd "$(dirname "$0")/.."

# Run the CLI with the caller's cwd preserved for config discovery.
exec env REFINE_INVOKED_FROM="$caller_cwd" \
     python3 -c "import os, sys; os.chdir(os.environ['REFINE_INVOKED_FROM']); sys.argv=['refine','runner']; from refine.cli import main; raise SystemExit(main())"

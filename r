#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
MODE="${REFINE_RUN_MODE:-auto}"
RELEASE_BIN="${REFINE_RELEASE_BIN:-$ROOT/bin/refine}"
DEPLOYED_MARKER="${REFINE_DEPLOYED_MARKER:-$ROOT/.refine-deployed}"

select_mode() {
  case "$MODE" in
    ""|auto)
      if [ -f "$DEPLOYED_MARKER" ] && [ -x "$RELEASE_BIN" ]; then
        printf '%s\n' "binary"
      else
        printf '%s\n' "cargo"
      fi
      ;;
    cargo|dev|development)
      printf '%s\n' "cargo"
      ;;
    binary|release|deployed)
      printf '%s\n' "binary"
      ;;
    *)
      printf 'refine: invalid REFINE_RUN_MODE=%s (expected auto, cargo, or binary)\n' "$MODE" >&2
      exit 2
      ;;
  esac
}

SELECTED_MODE="$(select_mode)"

if [ "${REFINE_R_DRY_RUN:-0}" = "1" ]; then
  printf 'mode=%s\n' "$SELECTED_MODE"
  if [ "$SELECTED_MODE" = "binary" ]; then
    printf 'executable=%s\n' "$RELEASE_BIN"
    printf 'command=%s' "$RELEASE_BIN"
  else
    printf 'executable=cargo\n'
    printf 'command=cargo run --quiet --manifest-path %s/Cargo.toml --' "$ROOT"
  fi
  for arg in "$@"; do
    printf ' %s' "$arg"
  done
  printf '\n'
  exit 0
fi

if [ "$SELECTED_MODE" = "binary" ]; then
  if [ ! -x "$RELEASE_BIN" ]; then
    printf 'refine: deployed binary is missing or not executable: %s\n' "$RELEASE_BIN" >&2
    printf 'refine: run scripts/install.sh again, or use REFINE_RUN_MODE=cargo ./r ...\n' >&2
    exit 127
  fi
  export REFINE_LAUNCH_MODE="binary"
  export REFINE_LAUNCH_EXECUTABLE="$RELEASE_BIN"
  exec "$RELEASE_BIN" "$@"
fi

export REFINE_LAUNCH_MODE="cargo"
export REFINE_LAUNCH_EXECUTABLE="cargo"
exec cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -- "$@"

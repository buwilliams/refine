#!/usr/bin/env bash
set -euo pipefail

# ./r always runs the production (release) binary at bin/refine — never a
# debug build. `system start` and `system build` create or refresh that binary;
# `system service-install` bootstraps it only when it is missing. `system update`
# owns the stop, Git update, rebuild, and start sequence. Every other command
# requires the binary to exist already.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
RELEASE_BIN="${REFINE_RELEASE_BIN:-$ROOT/bin/refine}"
DEPLOYED_MARKER="${REFINE_DEPLOYED_MARKER:-$ROOT/.refine-deployed}"

args_contain_help() {
  for arg in "$@"; do
    case "$arg" in
      --help|-h) return 0 ;;
    esac
  done
  return 1
}

# The production binary is stale when any build input is newer than it. Cargo
# remains the authority on what actually recompiles; this check only decides
# whether invoking Cargo is worth it at all, so `system start` on an unchanged
# tree costs one directory scan instead of a build.
source_changed_since_build() {
  [ -x "$RELEASE_BIN" ] || return 0
  local candidates=()
  local path
  for path in "$ROOT/src" "$ROOT/Cargo.toml" "$ROOT/Cargo.lock" "$ROOT/vendor" "$ROOT/build.rs"; do
    [ -e "$path" ] && candidates+=("$path")
  done
  [ "${#candidates[@]}" -gt 0 ] || return 1
  [ -n "$(find "${candidates[@]}" -newer "$RELEASE_BIN" -print -quit 2>/dev/null)" ]
}

install_release_binary() {
  local built_bin="$ROOT/target/release/refine"
  local staged_bin="$RELEASE_BIN.installing.$$"
  local staged_marker="$DEPLOYED_MARKER.installing.$$"

  cargo build --release --locked --target-dir "$ROOT/target" --manifest-path "$ROOT/Cargo.toml"
  if [ ! -f "$built_bin" ]; then
    printf 'refine: release build succeeded but did not produce %s\n' "$built_bin" >&2
    exit 1
  fi
  if [ -f "$RELEASE_BIN" ] && cmp -s "$built_bin" "$RELEASE_BIN"; then
    # The rebuild reproduced the installed binary. Refresh its timestamp so
    # unchanged sources stop looking newer than it, and keep the marker.
    touch "$RELEASE_BIN"
    [ -f "$DEPLOYED_MARKER" ] || printf 'mode=deployed\nrelease_bin=bin/refine\n' > "$DEPLOYED_MARKER"
    printf 'refine: production binary is already up to date: %s\n' "$RELEASE_BIN"
    return 0
  fi
  mkdir -p "$(dirname "$RELEASE_BIN")" "$(dirname "$DEPLOYED_MARKER")"
  trap 'rm -f "$staged_bin" "$staged_marker"' EXIT
  install -m 755 "$built_bin" "$staged_bin"
  printf 'mode=deployed\nrelease_bin=bin/refine\n' > "$staged_marker"
  mv -f "$staged_bin" "$RELEASE_BIN"
  mv -f "$staged_marker" "$DEPLOYED_MARKER"
  trap - EXIT
  printf 'refine: production binary updated: %s\n' "$RELEASE_BIN"
}

ensure_release_binary() {
  local context="$1"
  if [ ! -x "$RELEASE_BIN" ]; then
    printf 'refine: production binary is missing; building it before %s\n' "$context"
  elif source_changed_since_build; then
    printf 'refine: source changed since the last production build; rebuilding before %s\n' "$context"
  else
    return 0
  fi
  install_release_binary
}

bootstrap_release_binary() {
  local context="$1"
  [ -x "$RELEASE_BIN" ] && return 0
  printf 'refine: production binary is missing; building it before %s\n' "$context"
  install_release_binary
}

system_command_requested() {
  local wanted="$1"
  shift
  [ "${1:-}" = "system" ] && [ "${2:-}" = "$wanted" ] || return 1
  ! args_contain_help "$@"
}

print_test_usage() {
  cat >&2 <<'EOF'
Usage: ./r test [SUITE]

Suites:
  unit                 Run in-crate Rust unit tests only. This is the default.
  integration          Run opt-in CLI, daemon, Docker, and fleet suites.
  full                 Run all test suites and repository checks.

Focused xtask suites:
  rust
  smoke-ai
  cli
  fleet-ssh
  full-workflow
  multi-instance-sync
EOF
}

normalize_test_suite() {
  local suite="${1:-unit}"
  case "$suite" in
    --help|-h) printf '%s\n' "$suite" ;;
    --*) printf '%s\n' "__invalid_dashed_suite__:$suite" ;;
    *) printf '%s\n' "$suite" ;;
  esac
}

run_test_command() {
  local suite
  suite="$(normalize_test_suite "${1:-unit}")"
  shift || true
  if [ "$#" -ne 0 ]; then
    printf 'refine: ./r test accepts one suite option, got extra argument: %s\n' "$1" >&2
    print_test_usage
    exit 2
  fi

  case "$suite" in
    ""|unit)
      exec cargo test --manifest-path "$ROOT/Cargo.toml"
      ;;
    integration)
      exec cargo test --manifest-path "$ROOT/Cargo.toml" -- --integration
      ;;
    full)
      exec cargo test --manifest-path "$ROOT/Cargo.toml" -- --full
      ;;
    rust)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-rust
      ;;
    smoke-ai)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-smoke-ai
      ;;
    cli)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-cli
      ;;
    fleet-ssh)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-fleet-ssh
      ;;
    full-workflow)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-full-workflow
      ;;
    multi-instance-sync)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-multi-instance-sync
      ;;
    help|--help|-h)
      print_test_usage
      exit 0
      ;;
    __invalid_dashed_suite__:*)
      printf 'refine: suite names do not use -- prefixes: %s\n' "${suite#__invalid_dashed_suite__:}" >&2
      print_test_usage
      exit 2
      ;;
    *)
      printf 'refine: unknown test suite option: %s\n' "$suite" >&2
      print_test_usage
      exit 2
      ;;
  esac
}

run_system_update() {
  if args_contain_help "$@"; then
    cat <<'EOF'
Usage: ./r system update

Stop Refine, stash local changes and pull from Git, rebuild the production
binary, then start Refine. If the configured upstream has no new commits,
leave the checkout and running Refine process unchanged.
EOF
    return 0
  fi
  if [ "$#" -ne 0 ]; then
    printf 'refine: ./r system update accepts no arguments\n' >&2
    return 2
  fi
  if [ "${REFINE_R_DRY_RUN:-0}" = "1" ]; then
    printf 'mode=update\n'
    printf 'command=git fetch --quiet\n'
    printf "command=git rev-list --count HEAD..@{upstream}\n"
    printf 'condition=continue only when upstream has new commits\n'
    printf 'command=./r system stop\n'
    printf 'command=git stash\n'
    printf 'command=git pull\n'
    printf 'command=./r system build\n'
    printf 'command=./r system start\n'
    return 0
  fi

  cd "$ROOT"
  git fetch --quiet
  local update_count
  update_count="$(git rev-list --count HEAD..'@{upstream}')"
  if [ "$update_count" = "0" ]; then
    printf 'refine: already up to date; no update required\n'
    return 0
  fi
  ./r system stop
  git stash && git pull
  ./r system build
  ./r system start
}

print_test_dry_run() {
  local suite
  suite="$(normalize_test_suite "${1:-unit}")"
  shift || true
  if [ "$#" -ne 0 ]; then
    printf 'refine: ./r test accepts one suite option, got extra argument: %s\n' "$1" >&2
    print_test_usage
    exit 2
  fi

  case "$suite" in
    ""|unit)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo test --manifest-path %s/Cargo.toml\n' "$ROOT"
      ;;
    integration)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo test --manifest-path %s/Cargo.toml -- --integration\n' "$ROOT"
      ;;
    full)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo test --manifest-path %s/Cargo.toml -- --full\n' "$ROOT"
      ;;
    rust)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-rust\n' "$ROOT"
      ;;
    smoke-ai)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-smoke-ai\n' "$ROOT"
      ;;
    cli)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-cli\n' "$ROOT"
      ;;
    fleet-ssh)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-fleet-ssh\n' "$ROOT"
      ;;
    full-workflow)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-full-workflow\n' "$ROOT"
      ;;
    multi-instance-sync)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-multi-instance-sync\n' "$ROOT"
      ;;
    help|--help|-h)
      print_test_usage
      exit 0
      ;;
    __invalid_dashed_suite__:*)
      printf 'refine: suite names do not use -- prefixes: %s\n' "${suite#__invalid_dashed_suite__:}" >&2
      print_test_usage
      exit 2
      ;;
    *)
      printf 'refine: unknown test suite option: %s\n' "$suite" >&2
      print_test_usage
      exit 2
      ;;
  esac
}

if [ "${1:-}" = "test" ]; then
  shift
  if [ "${REFINE_R_DRY_RUN:-0}" = "1" ]; then
    print_test_dry_run "$@"
    exit 0
  fi
  run_test_command "$@"
fi

if [ "${1:-}" = "system" ] && [ "${2:-}" = "update" ]; then
  shift 2
  run_system_update "$@"
  exit $?
fi

# `system build`, `system clean`, and `system update` are launcher-owned: they
# manage the production binary itself, so they never delegate to it.
if [ "${1:-}" = "system" ] && { [ "${2:-}" = "build" ] || [ "${2:-}" = "clean" ]; }; then
  BINARY_ACTION="$2"
  shift 2
  if args_contain_help "$@"; then
    if [ "$BINARY_ACTION" = "build" ]; then
      printf 'Usage: ./r system build\n\nRebuild the production binary from source and publish it as bin/refine.\n'
    else
      printf 'Usage: ./r system clean\n\nRemove the published production binary (bin/refine) and its deployed marker.\n'
    fi
    exit 0
  fi
  if [ "$#" -ne 0 ]; then
    printf 'refine: ./r system %s accepts no arguments\n' "$BINARY_ACTION" >&2
    exit 2
  fi
  if [ "${REFINE_R_DRY_RUN:-0}" = "1" ]; then
    printf 'mode=%s\n' "$BINARY_ACTION"
    if [ "$BINARY_ACTION" = "build" ]; then
      printf 'executable=cargo\n'
      printf 'command=cargo build --release --locked --target-dir %s/target --manifest-path %s/Cargo.toml\n' "$ROOT" "$ROOT"
    else
      printf 'executable=rm\n'
      printf 'command=rm -f %s %s\n' "$RELEASE_BIN" "$DEPLOYED_MARKER"
    fi
    exit 0
  fi
  if [ "$BINARY_ACTION" = "build" ]; then
    printf 'refine: building production binary from source\n'
    install_release_binary
  else
    rm -f "$RELEASE_BIN" "$DEPLOYED_MARKER"
    printf 'refine: removed production binary %s\n' "$RELEASE_BIN"
  fi
  exit 0
fi

if [ "${REFINE_R_DRY_RUN:-0}" != "1" ]; then
  if system_command_requested service-install "$@"; then
    bootstrap_release_binary "system service-install"
  elif system_command_requested start "$@"; then
    ensure_release_binary "system start"
  fi
fi

if [ "${REFINE_R_DRY_RUN:-0}" = "1" ]; then
  printf 'mode=binary\n'
  printf 'executable=%s\n' "$RELEASE_BIN"
  printf 'command=%s' "$RELEASE_BIN"
  for arg in "$@"; do
    printf ' %s' "$arg"
  done
  printf '\n'
  exit 0
fi

if [ ! -x "$RELEASE_BIN" ]; then
  printf 'refine: production binary is missing: %s\n' "$RELEASE_BIN" >&2
  printf 'refine: build it with ./r system build (system start and system service-install build it automatically)\n' >&2
  exit 127
fi

# Other commands run whatever binary is published; they never build. Surface
# staleness so a stale binary is a visible choice rather than a surprise.
if source_changed_since_build; then
  printf 'refine: note: source changed since the last production build; run ./r system build to refresh bin/refine\n' >&2
fi

export REFINE_LAUNCH_MODE="binary"
export REFINE_LAUNCH_EXECUTABLE="$RELEASE_BIN"
exec "$RELEASE_BIN" "$@"

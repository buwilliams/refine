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

print_test_usage() {
  cat >&2 <<'EOF'
Usage: ./r test [SUITE]

Suites:
  --unit          Run in-crate Rust unit tests only. This is the default.
  --integration   Run opt-in integration, daemon, Docker, cluster, and UI suites.
  --full          Run all test suites and repository checks.

Focused xtask suites:
  --rust
  --surface
  --smoke-ai
  --cli
  --ui
  --cluster-ssh
  --install-uninstall
  --full-workflow
  --multi-instance-sync
EOF
}

run_test_command() {
  local suite="${1:---unit}"
  shift || true
  if [ "$#" -ne 0 ]; then
    printf 'refine: ./r test accepts one suite option, got extra argument: %s\n' "$1" >&2
    print_test_usage
    exit 2
  fi

  case "$suite" in
    ""|--unit|unit)
      exec cargo test --manifest-path "$ROOT/Cargo.toml"
      ;;
    --integration|integration)
      exec cargo test --manifest-path "$ROOT/Cargo.toml" -- --integration
      ;;
    --full|full)
      exec cargo test --manifest-path "$ROOT/Cargo.toml" -- --full
      ;;
    --rust|rust)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-rust
      ;;
    --surface|surface)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-surface
      ;;
    --smoke-ai|smoke-ai)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-smoke-ai
      ;;
    --cli|cli)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-cli
      ;;
    --ui|ui)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-ui
      ;;
    --cluster-ssh|cluster-ssh)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-cluster-ssh
      ;;
    --install-uninstall|install-uninstall)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-install-uninstall
      ;;
    --full-workflow|full-workflow)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-full-workflow
      ;;
    --multi-instance-sync|multi-instance-sync)
      exec cargo run --manifest-path "$ROOT/xtask/Cargo.toml" -- test-multi-instance-sync
      ;;
    --help|-h|help)
      print_test_usage
      exit 0
      ;;
    *)
      printf 'refine: unknown test suite option: %s\n' "$suite" >&2
      print_test_usage
      exit 2
      ;;
  esac
}

print_test_dry_run() {
  local suite="${1:---unit}"
  shift || true
  if [ "$#" -ne 0 ]; then
    printf 'refine: ./r test accepts one suite option, got extra argument: %s\n' "$1" >&2
    print_test_usage
    exit 2
  fi

  case "$suite" in
    ""|--unit|unit)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo test --manifest-path %s/Cargo.toml\n' "$ROOT"
      ;;
    --integration|integration)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo test --manifest-path %s/Cargo.toml -- --integration\n' "$ROOT"
      ;;
    --full|full)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo test --manifest-path %s/Cargo.toml -- --full\n' "$ROOT"
      ;;
    --rust|rust)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-rust\n' "$ROOT"
      ;;
    --surface|surface)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-surface\n' "$ROOT"
      ;;
    --smoke-ai|smoke-ai)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-smoke-ai\n' "$ROOT"
      ;;
    --cli|cli)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-cli\n' "$ROOT"
      ;;
    --ui|ui)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-ui\n' "$ROOT"
      ;;
    --cluster-ssh|cluster-ssh)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-cluster-ssh\n' "$ROOT"
      ;;
    --install-uninstall|install-uninstall)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-install-uninstall\n' "$ROOT"
      ;;
    --full-workflow|full-workflow)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-full-workflow\n' "$ROOT"
      ;;
    --multi-instance-sync|multi-instance-sync)
      printf 'mode=test\n'
      printf 'executable=cargo\n'
      printf 'command=cargo run --manifest-path %s/xtask/Cargo.toml -- test-multi-instance-sync\n' "$ROOT"
      ;;
    --help|-h|help)
      print_test_usage
      exit 0
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

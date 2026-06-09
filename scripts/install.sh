#!/usr/bin/env bash
# Refine first-run and repair installer.
#
# Safe to re-run: it creates missing directories, reuses existing checkouts,
# repairs PATH/profile entries, and asks before installing or overwriting setup.

set -uo pipefail

REFINE_REPO_URL="${REFINE_REPO_URL:-https://github.com/buwilliams/refine.git}"
REFINE_RAW_INSTALL_URL="${REFINE_RAW_INSTALL_URL:-https://raw.githubusercontent.com/buwilliams/refine/main/scripts/install.sh}"
REFINE_INSTALL_CHECKOUT_DEFAULT="${REFINE_INSTALL_CHECKOUT_DEFAULT:-${REFINE_INSTALL_BASE_DEFAULT:-$HOME/refine}}"
REFINE_DEFAULT_PORT="${REFINE_DEFAULT_PORT:-8080}"
REFINE_INSTALL_PORT="${REFINE_INSTALL_PORT:-}"
REFINE_BIND_ADDRESS="${REFINE_BIND_ADDRESS:-}"
REFINE_UPDATE_TARGET_APP="${REFINE_UPDATE_TARGET_APP:-1}"
REFINE_INSTALL_PROVIDER="${REFINE_INSTALL_PROVIDER:-}"
REFINE_INSTALL_TARGET_APP="${REFINE_INSTALL_TARGET_APP:-}"
REFINE_INSTALL_DRY_RUN="${REFINE_INSTALL_DRY_RUN:-0}"
REFINE_INSTALL_ASSUME_DEFAULTS="${REFINE_INSTALL_ASSUME_DEFAULTS:-0}"
REFINE_INSTALL_UPGRADE="${REFINE_INSTALL_UPGRADE:-1}"
REFINE_INSTALL_LOG="${REFINE_INSTALL_LOG:-}"
REFINE_RELEASE_BIN_RELATIVE="${REFINE_RELEASE_BIN_RELATIVE:-bin/refine}"
REFINE_DEPLOYED_MARKER_RELATIVE="${REFINE_DEPLOYED_MARKER_RELATIVE:-.refine-deployed}"
REFINE_INSTALL_RUNTIME_ROOT="${REFINE_INSTALL_RUNTIME_ROOT:-run}"
REFINE_INSTALL_UPDATE_ONLY="${REFINE_INSTALL_UPDATE_ONLY:-0}"
REFINE_INSTALL_PACKAGE_MANAGER="${REFINE_INSTALL_PACKAGE_MANAGER:-}"
REFINE_INSTALL_HOMEBREW_URL="${REFINE_INSTALL_HOMEBREW_URL:-https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh}"
REFINE_PROVIDER_OPTIONS="claude codex gemini copilot smoke-ai"
ORIGINAL_PATH="${PATH:-}"

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  BOLD="$(printf '\033[1m')"
  DIM="$(printf '\033[2m')"
  RED="$(printf '\033[31m')"
  GREEN="$(printf '\033[32m')"
  YELLOW="$(printf '\033[33m')"
  BLUE="$(printf '\033[34m')"
  MAGENTA="$(printf '\033[35m')"
  CYAN="$(printf '\033[36m')"
  RESET="$(printf '\033[0m')"
else
  BOLD=""
  DIM=""
  RED=""
  GREEN=""
  YELLOW=""
  BLUE=""
  MAGENTA=""
  CYAN=""
  RESET=""
fi

REFINE_CHECKOUT=""
TARGET_APP_PATH=""
SELECTED_PROVIDER=""
REFINE_UPGRADED="0"
REFINE_UPGRADED_TO=""
INSTALL_ISSUE_COUNT=0
INSTALL_ISSUES=""
INSTALL_LOG=""
INSTALL_LOG_READY=0
INSTALL_MODE=""
INSTALL_CHECKOUT=""

usage() {
  cat <<'EOF'
Usage: install.sh [--yes|-y] [--upgrade] [--no-upgrade]

Options:
  -y, --yes     Accept default answers without prompting.
  --upgrade     Accepted for compatibility; release upgrades are automatic.
  --no-upgrade  Skip release upgrade checks for an existing Refine checkout.
  -h, --help    Show this help.

Environment:
  REFINE_INSTALL_ASSUME_DEFAULTS=1  Same behavior as --yes.
  REFINE_INSTALL_CHECKOUT_DEFAULT   Default Refine checkout path.
  REFINE_INSTALL_DRY_RUN=1          Print commands instead of running them.
  REFINE_INSTALL_UPGRADE=0          Same behavior as --no-upgrade.
  REFINE_INSTALL_LOG                Install log path. Defaults to /tmp/refine-install-<pid>.log.
  REFINE_RELEASE_BIN_RELATIVE       Installed binary path inside the checkout. Defaults to bin/refine.
  REFINE_INSTALL_RUNTIME_ROOT       Runtime root used by Refine commands. Defaults to run.
  REFINE_INSTALL_UPDATE_ONLY=1      Upgrade/build/repair only; do not start Refine.
  REFINE_INSTALL_PACKAGE_MANAGER    Package manager for installer dependencies. Only brew is supported.
  REFINE_INSTALL_HOMEBREW_URL       Homebrew install script URL.
EOF
}

parse_args() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      -y|--yes)
        REFINE_INSTALL_ASSUME_DEFAULTS=1
        ;;
      --upgrade)
        REFINE_INSTALL_UPGRADE=1
        ;;
      --no-upgrade)
        REFINE_INSTALL_UPGRADE=0
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        die "Unknown option: $1"
        ;;
    esac
    shift
  done
}

say() {
  if [ "$INSTALL_LOG_READY" = "1" ]; then
    printf '%b\n' "$*" >&3
    printf '%b\n' "$*"
  else
    printf '%b\n' "$*"
  fi
}

log_detail() {
  printf '%b\n' "$*"
}

section() {
  say
  say "${BOLD}${BLUE}==> ${CYAN}$*${RESET}"
}

ok() {
  say "${GREEN}[ready]${RESET} $*"
}

info() {
  say "${CYAN}[info]${RESET}  $*"
}

warn() {
  if [ "$INSTALL_LOG_READY" = "1" ]; then
    printf '%b\n' "${YELLOW}[warn]${RESET}  $*" >&3
    printf '%b\n' "${YELLOW}[warn]${RESET}  $*" >&2
  else
    printf '%b\n' "${YELLOW}[warn]${RESET}  $*" >&2
  fi
}

err() {
  if [ "$INSTALL_LOG_READY" = "1" ]; then
    printf '%b\n' "${RED}[error]${RESET} $*" >&3
    printf '%b\n' "${RED}[error]${RESET} $*" >&2
  else
    printf '%b\n' "${RED}[error]${RESET} $*" >&2
  fi
}

die() {
  err "$*"
  exit 1
}

record_install_issue() {
  local failed="$1"
  local needed="$2"
  local action="$3"
  local detail="${4:-}"
  local entry
  INSTALL_ISSUE_COUNT=$((INSTALL_ISSUE_COUNT + 1))
  entry="$(printf '\n- Failed: %s\n  Why it is needed: %s\n  What to do: %s' "$failed" "$needed" "$action")"
  if [ -n "$detail" ]; then
    entry="$(printf '%s\n  Detail: %s' "$entry" "$detail")"
  fi
  if [ -n "$INSTALL_LOG" ]; then
    entry="$(printf '%s\n  Log: %s' "$entry" "$INSTALL_LOG")"
  fi
  INSTALL_ISSUES="${INSTALL_ISSUES}${entry}"
}

warn_issue() {
  local failed="$1"
  local needed="$2"
  local action="$3"
  local detail="${4:-}"
  if [ -n "$detail" ]; then
    warn "$detail"
  else
    warn "$failed"
  fi
  record_install_issue "$failed" "$needed" "$action" "$detail"
}

die_issue() {
  local failed="$1"
  local needed="$2"
  local action="$3"
  local detail="${4:-$failed}"
  err "$detail"
  record_install_issue "$failed" "$needed" "$action" "$detail"
  print_install_issues
  print_rerun_hint
  exit 1
}

print_install_issues() {
  [ "$INSTALL_ISSUE_COUNT" -gt 0 ] || return 0
  section "Needs attention"
  say "Some install steps did not complete:"
  say "$INSTALL_ISSUES"
  say
}

print_rerun_hint() {
  if [ -n "$INSTALL_LOG" ]; then
    say "Install log: $INSTALL_LOG"
  fi
  say "The install.sh script can be used again to: repair or upgrade."
}

start_install_log() {
  [ "$INSTALL_LOG_READY" = "1" ] && return 0
  if [ -n "$REFINE_INSTALL_LOG" ]; then
    INSTALL_LOG="$REFINE_INSTALL_LOG"
  else
    INSTALL_LOG="/tmp/refine-install-$$.log"
  fi
  if ! : >"$INSTALL_LOG"; then
    INSTALL_LOG=""
    warn "Could not open install log in /tmp; command output will stay on the terminal."
    return 0
  fi
  exec 3>&1
  exec 4>&2
  exec >>"$INSTALL_LOG" 2>&1
  INSTALL_LOG_READY=1
}

print_splash() {
  say "${BOLD}${CYAN}           ___            ${RESET}"
  say "${BOLD}${CYAN} _ __ ___ / _(_)_ __   ___ ${RESET}"
  say "${BOLD}${BLUE}| '__/ _ \\ |_| | '_ \\ / _ \\${RESET}"
  say "${BOLD}${MAGENTA}| | |  __/  _| | | | |  __/${RESET}"
  say "${BOLD}${GREEN}|_|  \\___|_| |_|_| |_|\\___|${RESET}"
  say
  say "${BOLD}${CYAN}refine${RESET} ${MAGENTA}install, repair, and upgrade script${RESET}"
  say "${DIM}Quiet terminal, detailed log, clear next steps.${RESET}"
}

canonical_path() {
  local path="$1"
  cd "$path" 2>/dev/null && pwd -P || printf '%s\n' "$path"
}

choose_install_mode() {
  local checkout path attempt
  checkout="$(current_refine_checkout || true)"
  if [ -n "$checkout" ]; then
    INSTALL_MODE="existing"
    INSTALL_CHECKOUT="$checkout"
    info "Detected existing Refine checkout: $INSTALL_CHECKOUT"
    return 0
  fi

  if [ "$REFINE_INSTALL_ASSUME_DEFAULTS" = "1" ]; then
    INSTALL_MODE="fresh"
    info "No existing Refine checkout detected; assuming a fresh install."
    return 0
  fi

  say
  info "No existing Refine checkout was detected here."
  info "Command output will be written to: $INSTALL_LOG"
  if confirm "Is this a new Refine install" "y"; then
    INSTALL_MODE="fresh"
    return 0
  fi

  attempt=1
  while [ "$attempt" -le 2 ]; do
    path="$(prompt "Existing Refine checkout path" "$REFINE_INSTALL_CHECKOUT_DEFAULT")"
    path="${path/#\~/$HOME}"
    if is_any_refine_checkout "$path"; then
      INSTALL_MODE="existing"
      INSTALL_CHECKOUT="$(canonical_path "$path")"
      ok "Using existing Refine checkout: $INSTALL_CHECKOUT"
      return 0
    fi
    warn "Could not find a Refine checkout at: $path"
    attempt=$((attempt + 1))
  done

  die_issue \
    "Existing Refine checkout lookup" \
    "Repair and upgrade need the existing Refine checkout so install.sh can update the right copy." \
    "Re-run install.sh from the Refine checkout, or provide the correct checkout path." \
    "Could not find Refine at the provided location."
}

have() {
  command -v "$1" >/dev/null 2>&1
}

dry_run() {
  [ "$REFINE_INSTALL_DRY_RUN" = "1" ]
}

is_refine_checkout() {
  [ -f "$1/Cargo.toml" ] &&
    [ -f "$1/src/main.rs" ] &&
    [ -x "$1/r" ] &&
    [ -f "$1/scripts/install.sh" ]
}

is_any_refine_checkout() {
  is_refine_checkout "$1"
}

is_git_checkout() {
  git -C "$1" rev-parse --is-inside-work-tree >/dev/null 2>&1
}

refine_manual_prefix() {
  printf 'cd %s && ./r' "$REFINE_CHECKOUT"
}

current_refine_checkout() {
  local dir
  dir="$(pwd -P 2>/dev/null || pwd)"
  while [ -n "$dir" ] && [ "$dir" != "/" ]; do
    if is_any_refine_checkout "$dir"; then
      printf '%s\n' "$dir"
      return 0
    fi
    dir="$(dirname "$dir")"
  done
  return 1
}

bound_target_app() {
  local binding="$REFINE_CHECKOUT/.refine-binding"
  local line path
  [ -f "$binding" ] || return 1
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      ""|\#*) continue ;;
    esac
    path="${line/#\~/$HOME}"
    case "$path" in
      /*) ;;
      *) path="$REFINE_CHECKOUT/$path" ;;
    esac
    if [ -d "$path" ]; then
      cd "$path" 2>/dev/null && pwd -P || printf '%s\n' "$path"
      return 0
    fi
    return 1
  done < "$binding"
  return 1
}

recorded_primary_port() {
  local checkout="$1"
  local runtime="$REFINE_INSTALL_RUNTIME_ROOT"
  [ -n "$checkout" ] || return 1
  case "$runtime" in
    /*) ;;
    *) runtime="$checkout/$runtime" ;;
  esac
  [ -f "$runtime/primary.json" ] || return 1
  sed -n 's/.*"port"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$runtime/primary.json" | head -n 1
}

resolve_refine_port() {
  local port=""
  if [ -n "$REFINE_INSTALL_PORT" ]; then
    port="$REFINE_INSTALL_PORT"
  elif [ -n "$REFINE_CHECKOUT" ]; then
    port="$(recorded_primary_port "$REFINE_CHECKOUT" || true)"
  fi
  [ -n "$port" ] || port="$REFINE_DEFAULT_PORT"
  printf '%s\n' "$port"
}

running_in_container() {
  [ -f /.dockerenv ] && return 0
  [ -f /run/.containerenv ] && return 0
  grep -qaE '/(docker|containerd|kubepods|libpod)(/|[-:])' /proc/1/cgroup 2>/dev/null
}

resolve_bind_address() {
  if [ -n "$REFINE_BIND_ADDRESS" ]; then
    printf '%s\n' "$REFINE_BIND_ADDRESS"
  elif running_in_container; then
    printf '0.0.0.0\n'
  else
    printf '127.0.0.1\n'
  fi
}

terminal_available() {
  [ -r /dev/tty ] && [ -w /dev/tty ]
}

write_prompt() {
  if terminal_available; then
    printf '%b' "$*" >/dev/tty
  elif [ "$INSTALL_LOG_READY" = "1" ]; then
    printf '%b' "$*" >&4
  else
    printf '%b' "$*" >&2
  fi
}

read_answer() {
  if terminal_available; then
    IFS= read -r "$1" </dev/tty
  elif [ -t 0 ]; then
    IFS= read -r "$1"
  else
    return 1
  fi
}

run() {
  if dry_run; then
    log_detail "${DIM}+ $*${RESET}"
    return 0
  fi
  "$@"
}

run_shell() {
  if dry_run; then
    log_detail "${DIM}+ $*${RESET}"
    return 0
  fi
  sh -c "$*"
}

prompt() {
  local message="$1"
  local default_value="${2:-}"
  local answer=""
  if [ "$REFINE_INSTALL_ASSUME_DEFAULTS" = "1" ]; then
    printf '%s\n' "$default_value"
    return 0
  fi
  if [ -n "$default_value" ]; then
    write_prompt "${BOLD}${message}${RESET} ${DIM}[${default_value}]${RESET}: "
  else
    write_prompt "${BOLD}${message}${RESET}: "
  fi
  if ! read_answer answer; then
    warn "No interactive terminal available; using default for: $message"
    answer="$default_value"
  fi
  if [ -z "$answer" ]; then
    answer="$default_value"
  fi
  printf '%s\n' "$answer"
}

confirm() {
  local message="$1"
  local default_answer="${2:-y}"
  local prompt_suffix="[Y/n]"
  local answer=""
  if [ "$default_answer" = "n" ]; then
    prompt_suffix="[y/N]"
  fi
  if [ "$REFINE_INSTALL_ASSUME_DEFAULTS" = "1" ]; then
    [ "$default_answer" = "y" ]
    return $?
  fi
  write_prompt "${BOLD}${message}${RESET} ${DIM}${prompt_suffix}${RESET}: "
  if ! read_answer answer; then
    warn "No interactive terminal available; using default answer '$default_answer' for: $message"
    answer="$default_answer"
  fi
  answer="$(printf '%s' "$answer" | tr '[:upper:]' '[:lower:]')"
  if [ -z "$answer" ]; then
    answer="$default_answer"
  fi
  case "$answer" in
    y|yes) return 0 ;;
    *) return 1 ;;
  esac
}

choice() {
  local message="$1"
  local default_value="$2"
  shift 2
  local options="$*"
  local answer=""
  while true; do
    answer="$(prompt "$message (${options})" "$default_value")"
    answer="$(printf '%s' "$answer" | tr '[:upper:]' '[:lower:]')"
    for opt in "$@"; do
      if [ "$answer" = "$opt" ]; then
        printf '%s\n' "$answer"
        return 0
      fi
    done
    warn "Choose one of: ${options}"
  done
}

append_path_now() {
  case ":$PATH:" in
    *":$1:"*) ;;
    *) export PATH="$1:$PATH" ;;
  esac
}

path_command() {
  PATH="$1" command -v "$2" 2>/dev/null || true
}

ensure_command_on_original_path() {
  local name="$1"
  local target existing candidates old_ifs dir link
  target="$(command -v "$name" 2>/dev/null || true)"
  [ -n "$target" ] || return 0
  existing="$(path_command "$ORIGINAL_PATH" "$name")"
  if [ -n "$existing" ]; then
    return 0
  fi

  candidates=""
  case ":$ORIGINAL_PATH:" in
    *":/usr/local/bin:"*) candidates="/usr/local/bin" ;;
  esac
  old_ifs="$IFS"
  IFS=":"
  for dir in $ORIGINAL_PATH; do
    [ -n "$dir" ] || continue
    case " $candidates " in
      *" $dir "*) ;;
      *) candidates="${candidates:+$candidates }$dir" ;;
    esac
  done
  IFS="$old_ifs"

  for dir in $candidates; do
    [ -d "$dir" ] || continue
    link="$dir/$name"
    if [ -e "$link" ] && [ ! -L "$link" ]; then
      continue
    fi
    if dry_run; then
      log_detail "${DIM}+ link $link -> $target so $name is available in this shell${RESET}"
      return 0
    fi
    if [ -w "$dir" ]; then
      ln -sf "$target" "$link" && {
        ok "Made $name available at $link"
        return 0
      }
    elif [ "$(id -u)" != "0" ] && have sudo; then
      sudo ln -sf "$target" "$link" && {
        ok "Made $name available at $link"
        return 0
      }
    fi
  done

  warn "$name is installed at $target, but it is not on this shell's PATH. Open a new shell or run: export PATH=\"$(dirname "$target"):\$PATH\""
}

profile_file() {
  if [ -n "${BASH_VERSION:-}" ]; then
    printf '%s\n' "$HOME/.bashrc"
  else
    printf '%s\n' "$HOME/.profile"
  fi
}

ensure_profile_path() {
  local dir="$1"
  local profile
  profile="$(profile_file)"
  append_path_now "$dir"
  if dry_run; then
    log_detail "${DIM}+ ensure '$dir' is on PATH in $profile${RESET}"
    return 0
  fi
  touch "$profile" 2>/dev/null || {
    warn "Could not update $profile; add this manually: export PATH=\"$dir:\$PATH\""
    return 0
  }
  if ! grep -F "export PATH=\"$dir:\$PATH\"" "$profile" >/dev/null 2>&1; then
    printf '\nexport PATH="%s:$PATH"\n' "$dir" >>"$profile"
    ok "Added $dir to $profile"
  fi
}

prepend_env_path_now() {
  local var="$1"
  local dir="$2"
  local current
  eval "current=\${$var:-}"
  case ":$current:" in
    *":$dir:"*) ;;
    *)
      if [ -n "$current" ]; then
        export "$var=$dir:$current"
      else
        export "$var=$dir"
      fi
      ;;
  esac
}

ensure_profile_env_path() {
  local var="$1"
  local dir="$2"
  local profile line
  profile="$(profile_file)"
  line="$(printf 'export %s="%s${%s:+:$%s}"' "$var" "$dir" "$var" "$var")"
  prepend_env_path_now "$var" "$dir"
  if dry_run; then
    log_detail "${DIM}+ ensure '$dir' is in $var in $profile${RESET}"
    return 0
  fi
  touch "$profile" 2>/dev/null || {
    warn "Could not update $profile; add this manually: $line"
    return 0
  }
  if ! grep -F "$line" "$profile" >/dev/null 2>&1; then
    printf '\n%s\n' "$line" >>"$profile"
    ok "Added $dir to $var in $profile"
  fi
}

configure_brew_c_toolchain_env() {
  local glibc_prefix
  [ "$(package_manager)" = "brew" ] || return 0
  have brew || return 0
  glibc_prefix="$(brew --prefix glibc 2>/dev/null || true)"
  [ -n "$glibc_prefix" ] || return 0
  [ -d "$glibc_prefix/lib" ] && ensure_profile_env_path LIBRARY_PATH "$glibc_prefix/lib"
  [ -d "$glibc_prefix/include" ] && ensure_profile_env_path C_INCLUDE_PATH "$glibc_prefix/include"
}

is_linux() {
  [ "$(uname -s)" = "Linux" ]
}

is_macos() {
  [ "$(uname -s)" = "Darwin" ]
}

is_wsl() {
  is_linux && { grep -qi microsoft /proc/version 2>/dev/null || grep -qi microsoft /proc/sys/kernel/osrelease 2>/dev/null; }
}

has_systemd() {
  have systemctl && [ -d /run/systemd/system ]
}

sudo_prefix() {
  if [ "$(id -u)" = "0" ]; then
    return 0
  fi
  have sudo
}

with_sudo() {
  if [ "$(id -u)" = "0" ]; then
    run "$@"
  elif have sudo; then
    run sudo "$@"
  else
    warn "sudo is not available. Run manually as an administrator: $*"
    return 127
  fi
}

package_manager_available() {
  case "$1" in
    brew) have brew ;;
    *) return 1 ;;
  esac
}

package_manager() {
  if [ -n "$REFINE_INSTALL_PACKAGE_MANAGER" ]; then
    case "$REFINE_INSTALL_PACKAGE_MANAGER" in
      brew)
        if dry_run; then
          printf '%s\n' "$REFINE_INSTALL_PACKAGE_MANAGER"
          return 0
        fi
        if package_manager_available "$REFINE_INSTALL_PACKAGE_MANAGER"; then
          printf '%s\n' "$REFINE_INSTALL_PACKAGE_MANAGER"
          return 0
        fi
        warn "Requested package manager is not available: $REFINE_INSTALL_PACKAGE_MANAGER"
        ;;
      *)
        warn "Unsupported REFINE_INSTALL_PACKAGE_MANAGER: $REFINE_INSTALL_PACKAGE_MANAGER"
        ;;
    esac
  fi
  if have brew; then
    printf '%s\n' "brew"
  else
    printf '%s\n' ""
  fi
}

install_packages() {
  local packages="$*"
  local pm
  pm="$(package_manager)"
  if [ -z "$pm" ]; then
    warn "No supported package manager found. Install manually: $packages"
    return 1
  fi
  if ! confirm "Install missing packages with $pm: $packages" "y"; then
    warn "Skipped package install. Install manually: $packages"
    return 1
  fi
  case "$pm" in
    brew)
      run brew install $packages
      ;;
  esac
}

find_cc_binary() {
  local prefix path
  if have cc; then
    command -v cc
    return 0
  fi
  if have gcc; then
    command -v gcc
    return 0
  fi
  if have clang; then
    command -v clang
    return 0
  fi
  if have brew; then
    prefix="$(brew --prefix 2>/dev/null || true)"
    if [ -n "$prefix" ]; then
      for path in "$prefix"/opt/llvm/bin/clang "$prefix"/bin/gcc-*; do
        [ -x "$path" ] || continue
        printf '%s\n' "$path"
        return 0
      done
    fi
  fi
  return 1
}

find_ld_binary() {
  local prefix path
  if have ld; then
    command -v ld
    return 0
  fi
  if have brew; then
    prefix="$(brew --prefix 2>/dev/null || true)"
    if [ -n "$prefix" ]; then
      for path in \
        "$prefix"/opt/llvm/bin/ld.lld \
        "$prefix"/opt/llvm/bin/lld \
        "$prefix"/opt/binutils/bin/ld \
        "$prefix"/Cellar/binutils/*/bin/ld \
        "$prefix"/Cellar/binutils/*/x86_64-*/bin/ld; do
        [ -x "$path" ] || continue
        printf '%s\n' "$path"
        return 0
      done
    fi
  fi
  return 1
}

ensure_command_shim() {
  local name="$1"
  local target="$2"
  local bin_dir="$HOME/bin"
  local link="$bin_dir/$name"
  [ -n "$target" ] || return 1
  append_path_now "$bin_dir"
  if have "$name"; then
    return 0
  fi
  if dry_run; then
    log_detail "${DIM}+ link $link -> $target so $name is available in this shell${RESET}"
    return 0
  fi
  mkdir -p "$bin_dir" || return 1
  ln -sf "$target" "$link" || return 1
  ok "Made $name available at $link"
  ensure_profile_path "$bin_dir"
  have "$name"
}

ensure_c_linker() {
  if have cc && have ld; then
    ok "C compiler/linker found: $(command -v cc), $(command -v ld)"
    return 0
  fi
  warn "C compiler/linker is not fully installed"
  case "$(package_manager)" in
    brew)
      if is_macos; then
        install_packages llvm || true
      else
        install_packages gcc binutils glibc || true
      fi
      configure_brew_c_toolchain_env
      ;;
    *)
      ;;
  esac
  configure_brew_c_toolchain_env
  ensure_command_shim cc "$(find_cc_binary || true)" || true
  ensure_command_shim ld "$(find_ld_binary || true)" || true
  if have cc && have ld; then
    ok "C compiler/linker ready: $(command -v cc), $(command -v ld)"
    return 0
  fi
  die_issue \
    "C compiler/linker install" \
    "Rust crates with build scripts need a C compiler and linker during the optimized Refine build." \
    "Install a C toolchain, then re-run install.sh." \
    "Could not find a usable cc compiler/linker."
}

ensure_command() {
  local cmd="$1"
  shift
  local packages="$*"
  if have "$cmd"; then
    ok "$cmd found: $(command -v "$cmd")"
    return 0
  fi
  warn "$cmd is not installed"
  install_packages "$packages" || true
  if have "$cmd"; then
    ok "$cmd installed: $(command -v "$cmd")"
    return 0
  fi
  warn "Still missing $cmd. Install it, then re-run this script."
  return 1
}

node_major() {
  node -v 2>/dev/null | sed -E 's/^v([0-9]+).*/\1/'
}

ensure_node_for_provider() {
  if have node && have npm; then
    local major
    major="$(node_major)"
    if [ -n "$major" ] && [ "$major" -ge 18 ] 2>/dev/null; then
      ok "Node.js $(node -v) and npm found"
      return 0
    fi
    warn "Node.js 18+ is required for provider CLI installs; found ${major:-unknown}"
  else
    warn "Node.js/npm missing"
  fi
  if [ "$(package_manager)" = "brew" ]; then
    install_packages node || true
  else
    install_packages nodejs npm || true
  fi
  if have node && have npm; then
    local major_after
    major_after="$(node_major)"
    if [ -n "$major_after" ] && [ "$major_after" -ge 18 ] 2>/dev/null; then
      ok "Node.js $(node -v) and npm ready"
      return 0
    fi
  fi
  warn "Install Node.js 18+ from https://nodejs.org/, then re-run this script."
  return 1
}

download_and_run() {
  local url="$1"
  local runner="$2"
  local label="$3"
  local tmp=""
  if ! have curl; then
    warn "curl is required for $label"
    return 1
  fi
  if dry_run; then
    log_detail "${DIM}+ curl -fsSL '$url' -o /tmp/refine-installer && $runner /tmp/refine-installer${RESET}"
    return 0
  fi
  tmp="$(mktemp)"
  if ! curl -fsSL "$url" -o "$tmp"; then
    rm -f "$tmp"
    warn "Could not download $label from $url"
    return 1
  fi
  if ! "$runner" "$tmp"; then
    rm -f "$tmp"
    warn "$label failed"
    return 1
  fi
  rm -f "$tmp"
}

install_rust_toolchain() {
  local tmp=""
  if ! have curl; then
    warn "curl is required for rustup"
    return 1
  fi
  if dry_run; then
    log_detail "${DIM}+ curl -fsSL https://sh.rustup.rs -o /tmp/refine-rustup && sh /tmp/refine-rustup -y${RESET}"
    return 0
  fi
  tmp="$(mktemp)"
  if ! curl -fsSL https://sh.rustup.rs -o "$tmp"; then
    rm -f "$tmp"
    warn "Could not download rustup"
    return 1
  fi
  if ! sh "$tmp" -y; then
    rm -f "$tmp"
    warn "rustup installer failed"
    return 1
  fi
  rm -f "$tmp"
  append_path_now "$HOME/.cargo/bin"
}

install_cargo_toolchain() {
  if [ "$(package_manager)" = "brew" ]; then
    install_packages rust || true
    return 0
  fi
  install_rust_toolchain
}

missing_core_dependencies() {
  local missing=""
  append_path_now "$HOME/.cargo/bin"
  have curl || missing="${missing}${missing:+, }curl"
  have git || missing="${missing}${missing:+, }git"
  { have cc && have ld; } || missing="${missing}${missing:+, }C compiler/linker"
  have cargo || missing="${missing}${missing:+, }Rust Cargo"
  printf '%s\n' "$missing"
}

manual_dependency_steps() {
  say "Install the missing dependencies, then re-run install.sh:"
  if is_macos; then
    say "  - Command Line Tools: xcode-select --install"
    say "  - Rust Cargo: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    say "  - curl and git: install them with your preferred macOS package manager or tools setup."
  else
    say "  - curl and git from your OS package manager."
    say "  - C compiler/linker from your OS package manager, such as build-essential, gcc/make, or base-devel."
    say "  - Rust Cargo: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  fi
}

append_homebrew_paths_now() {
  append_path_now "/opt/homebrew/bin"
  append_path_now "/usr/local/bin"
  append_path_now "/home/linuxbrew/.linuxbrew/bin"
}

install_homebrew_package_manager() {
  local tmp=""
  if have brew; then
    ok "Homebrew found: $(command -v brew)"
    return 0
  fi
  if ! have curl; then
    warn "curl is required to install Homebrew automatically."
    return 1
  fi
  if dry_run; then
    if [ "$REFINE_INSTALL_ASSUME_DEFAULTS" = "1" ]; then
      log_detail "${DIM}+ NONINTERACTIVE=1 bash <(curl -fsSL '$REFINE_INSTALL_HOMEBREW_URL')${RESET}"
    else
      log_detail "${DIM}+ bash <(curl -fsSL '$REFINE_INSTALL_HOMEBREW_URL')${RESET}"
    fi
    append_homebrew_paths_now
    REFINE_INSTALL_PACKAGE_MANAGER="brew"
    return 0
  fi
  tmp="$(mktemp)" || return 1
  if ! curl -fsSL "$REFINE_INSTALL_HOMEBREW_URL" -o "$tmp"; then
    rm -f "$tmp"
    warn "Could not download Homebrew installer from $REFINE_INSTALL_HOMEBREW_URL"
    return 1
  fi
  if [ "$REFINE_INSTALL_ASSUME_DEFAULTS" = "1" ]; then
    NONINTERACTIVE=1 bash "$tmp" || {
      rm -f "$tmp"
      warn "Homebrew installer failed"
      return 1
    }
  else
    bash "$tmp" || {
      rm -f "$tmp"
      warn "Homebrew installer failed"
      return 1
    }
  fi
  rm -f "$tmp"
  append_homebrew_paths_now
  REFINE_INSTALL_PACKAGE_MANAGER="brew"
  if have brew; then
    ok "Homebrew installed: $(command -v brew)"
    return 0
  fi
  warn "Homebrew install finished, but brew is not on PATH yet."
  return 1
}

ensure_dependency_manager_for_missing_core_deps() {
  local missing="$1"
  local default_answer="n"
  [ -n "$missing" ] || return 0
  section "Install dependencies"
  warn "Missing required install dependencies: $missing"
  manual_dependency_steps
  say
  if have brew; then
    ok "Homebrew found: $(command -v brew)"
    return 0
  fi
  if [ "$REFINE_INSTALL_ASSUME_DEFAULTS" = "1" ]; then
    default_answer="y"
  fi
  if ! confirm "Install Homebrew to manage these dependencies" "$default_answer"; then
    die_issue \
      "Required install dependencies" \
      "Refine needs these tools before it can clone, build, and run the native CLI: $missing." \
      "Install the missing dependencies manually, then re-run install.sh." \
      "Missing required install dependencies: $missing"
  fi
  install_homebrew_package_manager || die_issue \
    "Homebrew install" \
    "Homebrew was selected to install missing Refine dependencies: $missing." \
    "Install Homebrew or install the missing dependencies manually, then re-run install.sh." \
    "Could not install Homebrew automatically."
}

ensure_cargo() {
  append_path_now "$HOME/.cargo/bin"
  if have cargo; then
    local cargo_path
    cargo_path="$(command -v cargo)"
    ok "Cargo found: $cargo_path"
    if [ -z "$(path_command "$ORIGINAL_PATH" cargo)" ]; then
      ensure_profile_path "$(dirname "$cargo_path")"
    fi
    ensure_command_on_original_path cargo
    return 0
  fi
  warn "Rust Cargo is not installed"
  if [ "$(package_manager)" = "brew" ]; then
    if confirm "Install Rust with Homebrew" "y"; then
      install_cargo_toolchain || true
    fi
  elif confirm "Install Rust with rustup" "y"; then
    install_cargo_toolchain || true
  fi
  if have cargo; then
    local cargo_path_after
    cargo_path_after="$(command -v cargo)"
    ok "Cargo installed: $cargo_path_after"
    ensure_profile_path "$(dirname "$cargo_path_after")"
    ensure_command_on_original_path cargo
    return 0
  fi
  die_issue \
    "Rust Cargo install" \
    "Refine uses Cargo to build the optimized native CLI during install and upgrade." \
    "Install Rust with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh, then re-run install.sh." \
    "Cargo is required. Install Rust with rustup, then re-run install.sh."
}

provider_binary() {
  case "$1" in
    claude) printf '%s\n' "claude" ;;
    codex) printf '%s\n' "codex" ;;
    gemini) printf '%s\n' "gemini" ;;
    copilot) printf '%s\n' "copilot" ;;
    smoke-ai) printf '%s\n' "${REFINE_SMOKE_AI_PATH:-smoke-ai}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

provider_in_list() {
  case " $2 " in
    *" $1 "*) return 0 ;;
    *) return 1 ;;
  esac
}

detect_installed_providers() {
  local installed=""
  local provider binary
  for provider in $REFINE_PROVIDER_OPTIONS; do
    binary="$(provider_binary "$provider")"
    if have "$binary"; then
      installed="${installed:+$installed }$provider"
    fi
  done
  printf '%s\n' "$installed"
}

first_provider_or_default() {
  local providers="$1"
  local provider
  for provider in $providers; do
    printf '%s\n' "$provider"
    return 0
  done
  printf '%s\n' "claude"
}

report_provider_detection() {
  local installed="$1"
  local missing=""
  local provider
  if [ -n "$installed" ]; then
    ok "Installed provider CLIs: $installed"
  else
    warn "No supported provider CLIs found on PATH"
  fi
  for provider in $REFINE_PROVIDER_OPTIONS; do
    if ! provider_in_list "$provider" "$installed"; then
      missing="${missing:+$missing }$provider"
    fi
  done
  if [ -n "$missing" ]; then
    info "Missing provider CLIs: $missing"
  fi
}

ensure_provider_cli() {
  local provider="$1"
  local binary="$provider"
  local install_cmd=""
  local login_cmd=""
  case "$provider" in
    claude)
      binary="claude"
      install_cmd="npm install -g @anthropic-ai/claude-code"
      login_cmd="claude"
      ;;
    codex)
      binary="codex"
      install_cmd="npm install -g @openai/codex"
      login_cmd="codex login"
      ;;
    gemini)
      binary="gemini"
      install_cmd="npm install -g @google/gemini-cli"
      login_cmd="gemini auth login"
      ;;
    copilot)
      binary="copilot"
      install_cmd="curl -fsSL https://gh.io/copilot-install | bash"
      login_cmd="copilot login"
      ;;
    smoke-ai)
      binary="$(provider_binary "$provider")"
      install_cmd="set REFINE_SMOKE_AI_PATH to the smoke-ai executable path"
      login_cmd="REFINE_SMOKE_AI_PATH=/path/to/smoke-ai"
      ;;
  esac

  if have "$binary"; then
    ok "$binary found: $(command -v "$binary")"
  elif [ "$provider" = "smoke-ai" ]; then
    warn "smoke-ai is not configured"
  else
    warn "$binary is not installed"
    if [ "$provider" = "copilot" ]; then
      if confirm "Install GitHub Copilot CLI with GitHub's installer" "y"; then
        download_and_run "https://gh.io/copilot-install" bash "GitHub Copilot CLI installer" || true
      fi
    else
      ensure_node_for_provider || true
      if have npm && confirm "Install $provider CLI with npm: $install_cmd" "y"; then
        run_shell "$install_cmd" || warn "$install_cmd failed"
      fi
    fi
  fi

  if have "$binary"; then
    ok "$binary ready"
    if [ "$provider" = "smoke-ai" ]; then
      info "Smoke AI is configured from REFINE_SMOKE_AI_PATH"
      return 0
    fi
    if confirm "Run provider login/check now: $login_cmd" "n"; then
      run_shell "$login_cmd" || warn_issue \
        "$provider login/check" \
        "Provider auth lets Refine start agent sessions with $provider." \
        "Run $login_cmd, then re-run install.sh if Refine still cannot start agent work." \
        "Provider login/check failed. You can run it later: $login_cmd"
    else
      info "Provider auth can be completed later with: $login_cmd"
    fi
    return 0
  fi

  warn_issue \
    "$provider CLI install" \
    "Refine uses the selected agent CLI to implement, review, and repair Gaps." \
    "Run $install_cmd, complete provider auth if required, then re-run install.sh." \
    "Provider CLI still missing. Run this later, then re-run install.sh: $install_cmd"
  return 1
}

ensure_playwright_headless() {
  section "Playwright"
  local default_answer="y"
  if [ "$REFINE_UPGRADED" = "1" ]; then
    default_answer="n"
  fi
  if ! confirm "Install or repair Playwright Chromium for regression screenshots" "$default_answer"; then
    warn "Skipped Playwright. Managed regression screenshots may fail until Playwright is installed."
    return 0
  fi
  ensure_node_for_provider || true
  if ! have npx; then
    warn_issue \
      "Playwright Chromium install" \
      "Managed regression screenshots use Playwright Chromium." \
      "Install Node.js/npm 18+, then run: npx --yes playwright install --with-deps chromium" \
      "npx is missing. Install Node.js/npm 18+, then run: npx --yes playwright install --with-deps chromium"
    return 0
  fi
  if dry_run; then
    log_detail "${DIM}+ npx --yes playwright install --with-deps chromium${RESET}"
    return 0
  fi
  if npx --yes playwright install --with-deps chromium; then
    ok "Playwright Chromium ready"
  else
    warn_issue \
      "Playwright Chromium install" \
      "Managed regression screenshots use Playwright Chromium." \
      "Run manually: npx --yes playwright install --with-deps chromium" \
      "Playwright install failed. Run manually: npx --yes playwright install --with-deps chromium"
  fi
}

ensure_rootless_docker() {
  if have docker && docker info >/dev/null 2>&1; then
    ok "Docker is reachable"
    return 0
  fi
  if ! is_linux; then
    warn "Rootless Docker's installer is Linux-only. Install Docker Desktop if this target app needs containers."
    return 0
  fi
  if ! confirm "Install or repair rootless Docker for app workflows that need containers" "n"; then
    warn "Skipped Docker. Refine can run, but container-based target apps may fail."
    return 0
  fi
  if is_linux && ! have newuidmap; then
    install_packages uidmap || true
  fi
  download_and_run "https://get.docker.com/rootless" sh "Docker rootless installer" || {
    warn_issue \
      "Rootless Docker install" \
      "Container-based target app workflows use Docker." \
      "Install Docker manually, confirm docker info works, then re-run install.sh." \
      "Rootless Docker install could not complete. The installer above usually prints the exact missing prerequisite."
    return 0
  }
  ensure_profile_path "$HOME/bin"
  append_path_now "$HOME/bin"
  if has_systemd && have loginctl; then
    if confirm "Enable linger so rootless Docker can survive terminal close" "y"; then
      with_sudo loginctl enable-linger "$(whoami)" || warn "Could not enable linger. Run manually: sudo loginctl enable-linger $(whoami)"
    fi
  fi
  if have systemctl; then
    run systemctl --user start docker >/dev/null 2>&1 || true
  fi
  if have docker && docker info >/dev/null 2>&1; then
    ok "Rootless Docker is reachable"
  else
    warn_issue \
      "Docker reachability check" \
      "Container-based target app workflows need a reachable Docker daemon." \
      "Open a new shell and try: docker info, then re-run install.sh if Docker is still unavailable." \
      "Docker is installed or repaired, but not reachable yet. Open a new shell and try: docker info"
  fi
}

is_semver_tag() {
  case "$1" in
    ""|v*) return 1 ;;
  esac
  printf '%s\n' "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'
}

latest_semver_from_lines() {
  awk '/^[0-9]+\.[0-9]+\.[0-9]+$/ { print }' |
    sort -t. -k1,1n -k2,2n -k3,3n |
    tail -n 1
}

latest_remote_semver_release_tag() {
  local ref
  if dry_run; then
    printf '%s\n' "${REFINE_INSTALL_DRY_RUN_RELEASE_TAG:-1.0.0}"
    return 0
  fi
  git ls-remote --tags --refs "$REFINE_REPO_URL" 2>/dev/null |
    while read -r _ ref; do
      ref="${ref#refs/tags/}"
      is_semver_tag "$ref" && printf '%s\n' "$ref"
    done |
    latest_semver_from_lines
}

current_checkout_semver_tag() {
  git -C "$1" tag --merged HEAD 2>/dev/null | latest_semver_from_lines
}

checkout_ahead_of_semver_tag() {
  local checkout="$1"
  local version="$2"
  [ -n "$version" ] || return 1
  [ "$(git -C "$checkout" rev-parse HEAD 2>/dev/null)" != "$(git -C "$checkout" rev-parse "$version^{}" 2>/dev/null)" ]
}

upgrade_refine_checkout() {
  local checkout="$1"
  local force="${2:-1}"
  local latest current
  if [ "$force" = "0" ]; then
    return 0
  fi
  run git -C "$checkout" fetch --tags origin || {
    warn "Could not fetch Refine release tags. Keeping existing checkout."
    return 0
  }
  latest="$(latest_remote_semver_release_tag)"
  if [ -z "$latest" ]; then
    warn "No published semver releases found. Keeping existing checkout."
    return 0
  fi
  current="$(current_checkout_semver_tag "$checkout")"
  if [ -z "$current" ]; then
    warn "Current Refine checkout is not on a semver release tag."
  fi
  if [ -n "$current" ] && checkout_ahead_of_semver_tag "$checkout" "$current"; then
    ok "Refine checkout is ahead of release $current; assuming local development and skipping release upgrade."
    return 0
  fi
  if [ -n "$current" ] && [ "$current" = "$latest" ] && git -C "$checkout" merge-base --is-ancestor HEAD "$latest" 2>/dev/null; then
    ok "Refine already at latest release: $latest"
    return 0
  fi
  if [ -n "$(git -C "$checkout" status --porcelain 2>/dev/null)" ]; then
    warn "Refine checkout has local changes; not switching to release $latest."
    warn "Commit or stash changes, then re-run this installer."
    return 0
  fi
  run git -C "$checkout" checkout --detach "$latest" || {
    warn "Could not switch Refine checkout to $latest. Keeping existing checkout."
    return 0
  }
  REFINE_UPGRADED="1"
  REFINE_UPGRADED_TO="$latest"
  ok "Refine upgraded to release $latest"
}

clone_or_update_refine() {
  local checkout="$1"
  REFINE_CHECKOUT="$checkout"
  if is_git_checkout "$checkout"; then
    if ! is_any_refine_checkout "$checkout"; then
      die_issue \
        "Refine checkout setup" \
        "Refine needs its own git checkout so install.sh can repair or upgrade it safely." \
        "Choose an empty checkout path or an existing Refine git checkout, then re-run install.sh." \
        "$checkout is a git checkout, but it does not look like Refine. Choose another checkout path, then re-run."
    fi
    ok "Refine checkout exists: $checkout"
    upgrade_refine_checkout "$checkout" "$REFINE_INSTALL_UPGRADE"
    return 0
  fi
  if [ -e "$checkout" ]; then
    die_issue \
      "Refine checkout setup" \
      "Refine needs its own git checkout so install.sh can repair or upgrade it safely." \
      "Choose an empty checkout path or an existing Refine git checkout, then re-run install.sh." \
      "$checkout exists but is not a git checkout. Choose another checkout path, then re-run."
  fi
  run mkdir -p "$(dirname "$checkout")"
  local latest
  latest="$(latest_remote_semver_release_tag)"
  [ -n "$latest" ] || die_issue \
    "Refine release lookup" \
    "The installer clones a published Refine release for stable installs and upgrades." \
    "Check network or GitHub release access, then re-run install.sh." \
    "No published semver releases found for $REFINE_REPO_URL"
  if dry_run; then
    log_detail "${DIM}+ git clone --branch '$latest' '$REFINE_REPO_URL' '$checkout'${RESET}"
  else
    git clone --branch "$latest" "$REFINE_REPO_URL" "$checkout" || die_issue \
      "Refine clone" \
      "The installer needs the Refine checkout before it can configure or start Refine." \
      "Check git/network access to $REFINE_REPO_URL, then re-run install.sh." \
      "Could not clone Refine release $latest from $REFINE_REPO_URL"
  fi
  ok "Cloned Refine release $latest to $checkout"
}

release_binary_path() {
  printf '%s/%s\n' "$REFINE_CHECKOUT" "$REFINE_RELEASE_BIN_RELATIVE"
}

deployed_marker_path() {
  printf '%s/%s\n' "$REFINE_CHECKOUT" "$REFINE_DEPLOYED_MARKER_RELATIVE"
}

write_deployed_marker() {
  local marker="$1"
  if dry_run; then
    log_detail "${DIM}+ write deployed marker $marker${RESET}"
    return 0
  fi
  {
    printf 'mode=deployed\n'
    printf 'release_bin=%s\n' "$REFINE_RELEASE_BIN_RELATIVE"
    printf 'built_at=%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  } >"$marker"
}

build_refine_release() {
  section "Build Refine"
  local release_bin
  local marker
  release_bin="$(release_binary_path)"
  marker="$(deployed_marker_path)"
  run cd "$REFINE_CHECKOUT" || die_issue \
    "Refine checkout access" \
    "The installer needs to enter the Refine checkout before building the release binary." \
    "Fix permissions for $REFINE_CHECKOUT, then re-run install.sh." \
    "Could not enter $REFINE_CHECKOUT"
  run cargo build --release --locked || die_issue \
    "Refine release build" \
    "Installed Refine runs from an optimized production binary so restarts do not depend on Cargo." \
    "Run manually: cd $REFINE_CHECKOUT && cargo build --release --locked, then re-run install.sh." \
    "Could not build the optimized Refine binary."
  run mkdir -p "$(dirname "$release_bin")" || die_issue \
    "Refine binary directory" \
    "The installed Refine binary needs a stable path under the checkout." \
    "Fix permissions for $(dirname "$release_bin"), then re-run install.sh." \
    "Could not create $(dirname "$release_bin")"
  run install -m 755 "$REFINE_CHECKOUT/target/release/refine" "$release_bin" || die_issue \
    "Refine binary install" \
    "The ./r wrapper uses the stable installed binary for deployed runs." \
    "Run manually: install -m 755 $REFINE_CHECKOUT/target/release/refine $release_bin, then re-run install.sh." \
    "Could not install the optimized Refine binary to $release_bin."
  write_deployed_marker "$marker" || die_issue \
    "Refine deployed marker" \
    "The ./r wrapper uses this marker to select deployed binary mode automatically." \
    "Fix permissions for $REFINE_CHECKOUT, then re-run install.sh." \
    "Could not write deployed marker $marker."
  ok "Optimized Refine binary ready: $release_bin"
}

install_state_paths() {
  local runtime="$REFINE_INSTALL_RUNTIME_ROOT"
  local found="0"
  case "$runtime" in
    /*) ;;
    *) runtime="$REFINE_CHECKOUT/$runtime" ;;
  esac
  if [ -d "$runtime" ]; then
    for state in "$runtime"/*/install-state.json; do
      [ -f "$state" ] || continue
      found="1"
      printf '%s\n' "$state"
    done
  fi
  if [ -f "$runtime/install-state.json" ]; then
    found="1"
    printf '%s\n' "$runtime/install-state.json"
  fi
  [ "$found" = "1" ]
}

repair_existing_refine_install() {
  section "Repair installed service"
  local install_state port repaired="0"
  run cd "$REFINE_CHECKOUT" || die_issue \
    "Refine checkout access" \
    "The installer needs to enter the Refine checkout before refreshing service metadata." \
    "Fix permissions for $REFINE_CHECKOUT, then re-run install.sh." \
    "Could not enter $REFINE_CHECKOUT"
  if ! install_state_paths >/tmp/refine-install-states-$$; then
    info "No install-state.json files found under $REFINE_INSTALL_RUNTIME_ROOT; skipping persistent service repair."
    return 0
  fi
  while IFS= read -r install_state || [ -n "$install_state" ]; do
    case "$(dirname "$install_state")" in
      "$REFINE_CHECKOUT/$REFINE_INSTALL_RUNTIME_ROOT"|"$REFINE_INSTALL_RUNTIME_ROOT")
        port="$(resolve_refine_port)"
        ;;
      *)
        port="$(basename "$(dirname "$install_state")")"
        ;;
    esac
    case "$port" in
      ''|*[!0-9]*)
        warn "Skipping install state with non-port directory: $install_state"
        continue
        ;;
    esac
    repaired="1"
    run ./r system repair --port "$port" --runtime-root "$REFINE_INSTALL_RUNTIME_ROOT" || die_issue \
      "Refine service repair" \
      "Persistent services must be refreshed so they point at the deployed Refine binary." \
      "Run manually: $(refine_manual_prefix) system repair --port $port --runtime-root $REFINE_INSTALL_RUNTIME_ROOT" \
      "Could not refresh Refine service metadata for port $port."
  done </tmp/refine-install-states-$$
  rm -f /tmp/refine-install-states-$$
  [ "$repaired" = "1" ] || info "No port-scoped install states were repairable."
}

finish_update_only() {
  repair_existing_refine_install
  section "Done"
  say "Refine checkout: ${BOLD}$REFINE_CHECKOUT${RESET}"
  say "Binary:          ${BOLD}$(release_binary_path)${RESET}"
  print_install_issues
  print_rerun_hint
}

target_from_remote() {
  local remote="$1"
  local default_dir="$2"
  local path
  path="$default_dir"
  info "Cloning target app into $path"
  path="${path/#\~/$HOME}"
  if [ -d "$path/.git" ]; then
    ok "Target app checkout already exists: $path"
  elif [ -e "$path" ] && [ "$(find "$path" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')" != "0" ]; then
    die_issue \
      "Target app clone" \
      "Refine needs a target application repository to attach work to." \
      "Choose an empty target app path or an existing git checkout, then re-run install.sh." \
      "$path exists and is not empty. Choose another target app path."
  else
    run git clone "$remote" "$path" || die_issue \
      "Target app clone" \
      "Refine needs a target application repository to attach work to." \
      "Check git/network access to $remote, then re-run install.sh." \
      "Could not clone target app from $remote"
  fi
  TARGET_APP_PATH="$(cd "$path" 2>/dev/null && pwd -P || printf '%s\n' "$path")"
}

choose_target_app() {
  section "Target application"
  local input=""
  if [ -n "$REFINE_INSTALL_TARGET_APP" ]; then
    input="$REFINE_INSTALL_TARGET_APP"
    info "Using target app from REFINE_INSTALL_TARGET_APP"
  else
    local existing_target
    existing_target="$(bound_target_app || true)"
    if [ -n "$existing_target" ]; then
      TARGET_APP_PATH="$existing_target"
      ok "Using existing target app binding: $TARGET_APP_PATH"
      return 0
    fi
    TARGET_APP_PATH=""
    info "No target app attached. Add a local path or Git remote from the Refine Guide in the browser."
    return 0
  fi
  if [ -z "$input" ]; then
    TARGET_APP_PATH=""
    info "No target app attached. Refine will open in setup mode so you can choose one in the browser."
    return 0
  fi
  case "$input" in
    http://*|https://*|git@*|ssh://*)
      local name
      name="$(basename "$input")"
      name="${name%.git}"
      [ -n "$name" ] || name="target-app"
      target_from_remote "$input" "$(dirname "$REFINE_CHECKOUT")/$name"
      ;;
    *)
      input="${input/#\~/$HOME}"
      if [ ! -d "$input" ]; then
        if confirm "Create target app directory $input" "n"; then
          run mkdir -p "$input"
          run git -C "$input" init -q || die_issue \
            "Target app git init" \
            "Refine target apps must be git repositories so changes can be tracked and merged." \
            "Fix permissions for $input or choose another path, then re-run install.sh." \
            "Could not initialize git in $input"
        else
          die_issue \
            "Target app selection" \
            "Refine needs a target application repository to attach work to." \
            "Create $input or choose an existing git checkout, then re-run install.sh." \
            "Target app path does not exist: $input"
        fi
      fi
      if [ ! -d "$input/.git" ]; then
        if confirm "$input is not a git repo. Initialize git there" "y"; then
          run git -C "$input" init -q || die_issue \
            "Target app git init" \
            "Refine target apps must be git repositories so changes can be tracked and merged." \
            "Fix permissions for $input or choose another path, then re-run install.sh." \
            "Could not initialize git in $input"
        else
          die_issue \
            "Target app git repository" \
            "Refine target apps must be git repositories so changes can be tracked and merged." \
            "Initialize git in $input or choose another git checkout, then re-run install.sh." \
            "Refine target apps must be git repositories."
        fi
      fi
      TARGET_APP_PATH="$(cd "$input" 2>/dev/null && pwd -P || printf '%s\n' "$input")"
      ;;
  esac
  ok "Target app: $TARGET_APP_PATH"
}

target_refine() {
  section "Refine target app"
  if [ ! -d "$REFINE_CHECKOUT" ]; then
    if dry_run; then
      info "Dry run: checkout would exist after clone/build at $REFINE_CHECKOUT"
    else
      die_issue \
        "Refine checkout setup" \
        "The Refine checkout is needed to run the Refine CLI and write configuration." \
        "Check $REFINE_CHECKOUT, then re-run install.sh." \
        "Refine checkout missing: $REFINE_CHECKOUT"
    fi
  fi
  run cd "$REFINE_CHECKOUT" || die_issue \
    "Refine checkout access" \
    "The installer needs to enter the Refine checkout to run setup commands." \
    "Fix permissions for $REFINE_CHECKOUT, then re-run install.sh." \
    "Could not enter $REFINE_CHECKOUT"
  if [ -z "$TARGET_APP_PATH" ]; then
    info "Skipping target-app attachment until you attach an app in Refine."
    return 0
  fi
  [ -d "$TARGET_APP_PATH" ] || die_issue \
    "Target app selection" \
    "Refine needs the target application directory to attach work to it." \
    "Check $TARGET_APP_PATH or choose another target app, then re-run install.sh." \
    "Target app missing: $TARGET_APP_PATH"
  run ./r project attach "$TARGET_APP_PATH" --runtime-root run || die_issue \
    "Refine target attachment" \
    "Target attachment tells Refine which application repository it should manage." \
    "Run manually: $(refine_manual_prefix) project attach $TARGET_APP_PATH --runtime-root run" \
    "refine project attach failed"
  info "Configure provider selection and target-app commands in Refine Settings after startup."
}

start_refine() {
  section "Start Refine"
  local port
  local bind_address
  local refine_started="1"
  port="$(prompt "Refine port" "$(resolve_refine_port)")"
  bind_address="$(resolve_bind_address)"
  if has_systemd && confirm "Prepare Refine as a persistent service with: ./r system install --target linux-cli-web --port $port" "y"; then
    if run ./r system install --target linux-cli-web --port "$port" --runtime-root run; then
      ok "Refine installed as a persistent service"
    else
      warn_issue \
        "Persistent Refine service install" \
        "The persistent service keeps Refine running after terminal close and host restarts." \
        "Run manually: $(refine_manual_prefix) system install --target linux-cli-web --port $port --runtime-root run" \
        "Persistent install failed. Trying non-installed background start."
      if ! run ./r system start --port "$port" --bind-address "$bind_address" --runtime-root run; then
        warn_issue \
          "Refine background start" \
          "Refine must be running for the browser UI." \
          "Run manually: $(refine_manual_prefix) system start --port $port --bind-address $bind_address --runtime-root run" \
          "Could not start Refine. Run manually: $(refine_manual_prefix) system start --port $port --bind-address $bind_address --runtime-root run"
        refine_started="0"
      fi
    fi
  else
    if ! has_systemd; then
      info "Persistent service install requires systemd. Starting with: ./r system start --port $port --bind-address $bind_address --runtime-root run"
    fi
    if ! run ./r system start --port "$port" --bind-address "$bind_address" --runtime-root run; then
      warn_issue \
        "Refine background start" \
        "Refine must be running for the browser UI." \
        "Run manually: $(refine_manual_prefix) system start --port $port --bind-address $bind_address --runtime-root run" \
        "Could not start Refine. Run manually: $(refine_manual_prefix) system start --port $port --bind-address $bind_address --runtime-root run"
      refine_started="0"
    fi
  fi
  run ./r system status --port "$port" --runtime-root run || true
  say
  if [ "$refine_started" = "1" ]; then
    ok "Open Refine: http://localhost:$port"
  else
    warn "Refine did not start cleanly; use the follow-up steps below before opening http://localhost:$port"
  fi
}

restart_refine_after_upgrade() {
  section "Restart Refine"
  local release="$REFINE_UPGRADED_TO"
  local port
  [ -n "$release" ] || release="the new release"
  port="$(resolve_refine_port)"
  if ! confirm "Restart Refine now to run $release" "y"; then
    info "Refine was upgraded but not restarted. Restart later with: $(refine_manual_prefix) system restart --port $port --runtime-root run"
    return 0
  fi
  run cd "$REFINE_CHECKOUT" || die_issue \
    "Refine checkout access" \
    "The installer needs to enter the Refine checkout before restarting Refine." \
    "Fix permissions for $REFINE_CHECKOUT, then re-run install.sh." \
    "Could not enter $REFINE_CHECKOUT"
  if run ./r system restart --port "$port" --runtime-root run; then
    ok "Refine restarted"
  else
    warn_issue \
    "Refine restart after upgrade" \
    "The running service must restart before it uses the upgraded Refine release." \
    "Run manually: $(refine_manual_prefix) system restart --port $port --runtime-root run" \
    "Could not restart Refine. Run manually: $(refine_manual_prefix) system restart --port $port --runtime-root run"
  fi
}

preflight() {
  section "System check"
  local missing
  if is_wsl; then
    ok "Running inside WSL"
  elif is_linux; then
    ok "Running on Linux"
  elif is_macos; then
    ok "Running on macOS"
  else
    warn "Unsupported OS: $(uname -s). Refine is tested on Linux/WSL and macOS."
  fi
  missing="$(missing_core_dependencies)"
  ensure_dependency_manager_for_missing_core_deps "$missing"
  ensure_command curl curl || die_issue \
    "curl install" \
    "The installer uses curl to download setup helpers and check Refine releases." \
    "Install curl with your OS package manager, then re-run install.sh." \
    "curl is required"
  ensure_command git git || die_issue \
    "git install" \
    "Refine uses git to clone, update, and manage the Refine and target app repositories." \
    "Install git with your OS package manager, then re-run install.sh." \
    "git is required"
  ensure_c_linker
  ensure_cargo
}

provider_flow() {
  section "AI provider"
  say "Choose the agent CLI Refine should drive."
  local installed_providers
  installed_providers="$(detect_installed_providers)"
  report_provider_detection "$installed_providers"
  if [ -n "$REFINE_INSTALL_PROVIDER" ]; then
    SELECTED_PROVIDER="$(printf '%s' "$REFINE_INSTALL_PROVIDER" | tr '[:upper:]' '[:lower:]')"
    case "$SELECTED_PROVIDER" in
      claude|codex|gemini|copilot|smoke-ai) ;;
      *) die "REFINE_INSTALL_PROVIDER must be one of: claude codex gemini copilot smoke-ai" ;;
    esac
    info "Using provider from REFINE_INSTALL_PROVIDER: $SELECTED_PROVIDER"
  else
    SELECTED_PROVIDER="$(choice "Provider" "$(first_provider_or_default "$installed_providers")" claude codex gemini copilot smoke-ai)"
  fi
  ensure_provider_cli "$SELECTED_PROVIDER" || true
  run cd "$REFINE_CHECKOUT" || return 0
  run ./r agent configure --provider "$SELECTED_PROVIDER" || warn_issue \
    "AI provider configure check" \
    "Provider configuration verifies that the selected agent CLI is known to native Refine." \
    "Configure the provider manually after startup: $(refine_manual_prefix) agent configure --provider $SELECTED_PROVIDER" \
    "Could not verify provider configuration for $SELECTED_PROVIDER"
}

main() {
  parse_args "$@"
  start_install_log
  print_splash
  choose_install_mode
  if dry_run; then
    warn "Dry run mode: commands will be logged, not executed."
  fi

  preflight

  section "Workspace"
  local checkout
  if [ -n "$INSTALL_CHECKOUT" ]; then
    checkout="$INSTALL_CHECKOUT"
    ok "Using current Refine checkout: $checkout"
  else
    checkout="$(prompt "Refine checkout path" "$REFINE_INSTALL_CHECKOUT_DEFAULT")"
    checkout="${checkout/#\~/$HOME}"
  fi
  clone_or_update_refine "$checkout"
  build_refine_release
  if [ "$REFINE_INSTALL_UPDATE_ONLY" = "1" ]; then
    finish_update_only
    return 0
  fi

  provider_flow
  ensure_playwright_headless
  choose_target_app
  ensure_rootless_docker
  target_refine
  if [ "$REFINE_UPGRADED" = "1" ]; then
    restart_refine_after_upgrade
  else
    start_refine
  fi

  if [ "$INSTALL_ISSUE_COUNT" -gt 0 ]; then
    section "Done with follow-up"
  else
    section "Done"
  fi
  say "Refine checkout: ${BOLD}$REFINE_CHECKOUT${RESET}"
  if [ -n "$TARGET_APP_PATH" ]; then
    say "Target app:       ${BOLD}$TARGET_APP_PATH${RESET}"
  else
    say "Target app:       ${BOLD}not attached yet${RESET}"
  fi
  say "Provider:         ${BOLD}$SELECTED_PROVIDER${RESET}"
  say
  print_install_issues
  print_rerun_hint
}

main "$@"

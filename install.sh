#!/usr/bin/env bash
# Refine first-run and repair installer.
#
# Safe to re-run: it creates missing directories, reuses existing checkouts,
# repairs PATH/profile entries, and asks before installing or overwriting setup.

set -uo pipefail

REFINE_REPO_URL="${REFINE_REPO_URL:-https://github.com/buwilliams/refine.git}"
REFINE_RAW_INSTALL_URL="${REFINE_RAW_INSTALL_URL:-https://raw.githubusercontent.com/buwilliams/refine/main/install.sh}"
REFINE_INSTALL_BASE_DEFAULT="${REFINE_INSTALL_BASE_DEFAULT:-$HOME/refine-work}"
REFINE_CHECKOUT_NAME="${REFINE_CHECKOUT_NAME:-refine}"
REFINE_DEFAULT_PORT="${REFINE_DEFAULT_PORT:-8080}"
REFINE_INSTALL_PROVIDER="${REFINE_INSTALL_PROVIDER:-}"
REFINE_INSTALL_TARGET_APP="${REFINE_INSTALL_TARGET_APP:-}"
REFINE_INSTALL_DRY_RUN="${REFINE_INSTALL_DRY_RUN:-0}"
REFINE_INSTALL_ASSUME_DEFAULTS="${REFINE_INSTALL_ASSUME_DEFAULTS:-0}"

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
  BOLD="$(printf '\033[1m')"
  DIM="$(printf '\033[2m')"
  RED="$(printf '\033[31m')"
  GREEN="$(printf '\033[32m')"
  YELLOW="$(printf '\033[33m')"
  BLUE="$(printf '\033[34m')"
  CYAN="$(printf '\033[36m')"
  RESET="$(printf '\033[0m')"
else
  BOLD=""
  DIM=""
  RED=""
  GREEN=""
  YELLOW=""
  BLUE=""
  CYAN=""
  RESET=""
fi

REFINE_CHECKOUT=""
TARGET_APP_PATH=""
SELECTED_PROVIDER=""

say() {
  printf '%b\n' "$*"
}

section() {
  printf '\n%b\n' "${BOLD}${BLUE}==> $*${RESET}"
}

ok() {
  say "${GREEN}OK${RESET}  $*"
}

info() {
  say "${CYAN}..${RESET}  $*"
}

warn() {
  say "${YELLOW}!!${RESET}  $*" >&2
}

err() {
  say "${RED}!!${RESET}  $*" >&2
}

die() {
  err "$*"
  exit 1
}

have() {
  command -v "$1" >/dev/null 2>&1
}

dry_run() {
  [ "$REFINE_INSTALL_DRY_RUN" = "1" ]
}

terminal_available() {
  [ -r /dev/tty ] && [ -w /dev/tty ]
}

write_prompt() {
  if terminal_available; then
    printf '%b' "$*" >/dev/tty
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
    say "${DIM}+ $*${RESET}"
    return 0
  fi
  "$@"
}

run_shell() {
  if dry_run; then
    say "${DIM}+ $*${RESET}"
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
    say "${DIM}+ ensure '$dir' is on PATH in $profile${RESET}"
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

package_manager() {
  if have apt-get; then
    printf '%s\n' "apt"
  elif have dnf; then
    printf '%s\n' "dnf"
  elif have yum; then
    printf '%s\n' "yum"
  elif have pacman; then
    printf '%s\n' "pacman"
  elif have brew; then
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
    apt)
      with_sudo apt-get update || return 1
      with_sudo apt-get install -y $packages
      ;;
    dnf)
      with_sudo dnf install -y $packages
      ;;
    yum)
      with_sudo yum install -y $packages
      ;;
    pacman)
      with_sudo pacman -Sy --needed --noconfirm $packages
      ;;
    brew)
      run brew install $packages
      ;;
  esac
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
    say "${DIM}+ curl -fsSL '$url' -o /tmp/refine-installer && $runner /tmp/refine-installer${RESET}"
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

ensure_uv() {
  append_path_now "$HOME/.local/bin"
  append_path_now "$HOME/.cargo/bin"
  if have uv; then
    ok "uv found: $(command -v uv)"
    return 0
  fi
  warn "uv is not installed"
  if confirm "Install uv from Astral's official installer" "y"; then
    download_and_run "https://astral.sh/uv/install.sh" sh "uv installer" || true
    append_path_now "$HOME/.local/bin"
    append_path_now "$HOME/.cargo/bin"
  fi
  if have uv; then
    ok "uv installed: $(command -v uv)"
    ensure_profile_path "$HOME/.local/bin"
    return 0
  fi
  die "uv is required. Install it with: curl -LsSf https://astral.sh/uv/install.sh | sh"
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
  esac

  if have "$binary"; then
    ok "$binary found: $(command -v "$binary")"
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
    if confirm "Run provider login/check now: $login_cmd" "n"; then
      run_shell "$login_cmd" || warn "Provider login/check failed. You can run it later: $login_cmd"
    else
      info "Provider auth can be completed later with: $login_cmd"
    fi
    return 0
  fi

  warn "Provider CLI still missing. Run this later, then re-run install.sh: $install_cmd"
  return 1
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
  if ! confirm "Install or repair rootless Docker for app workflows that need containers" "y"; then
    warn "Skipped Docker. Refine can run, but container-based target apps may fail."
    return 0
  fi
  if is_linux && ! have newuidmap; then
    install_packages uidmap || true
  fi
  download_and_run "https://get.docker.com/rootless" sh "Docker rootless installer" || {
    warn "Rootless Docker install could not complete. The installer above usually prints the exact missing prerequisite."
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
    warn "Docker is installed or repaired, but not reachable yet. Open a new shell and try: docker info"
  fi
}

clone_or_update_refine() {
  local base="$1"
  local checkout="$base/$REFINE_CHECKOUT_NAME"
  REFINE_CHECKOUT="$checkout"
  if [ -d "$checkout/.git" ]; then
    ok "Refine checkout exists: $checkout"
    if confirm "Fetch latest Refine changes in this checkout" "y"; then
      run git -C "$checkout" fetch origin main || true
      if run git -C "$checkout" rev-parse --abbrev-ref HEAD >/dev/null 2>&1; then
        local branch
        branch="$(git -C "$checkout" rev-parse --abbrev-ref HEAD 2>/dev/null || printf '')"
        if [ "$branch" = "main" ]; then
          run git -C "$checkout" pull --ff-only origin main || warn "Could not fast-forward Refine. Keeping existing checkout."
        else
          warn "Refine checkout is on branch $branch; fetched but did not switch branches."
        fi
      fi
    fi
    return 0
  fi
  if [ -e "$checkout" ]; then
    die "$checkout exists but is not a git checkout. Move it or set REFINE_CHECKOUT_NAME, then re-run."
  fi
  run mkdir -p "$base"
  run git clone "$REFINE_REPO_URL" "$checkout" || die "Could not clone Refine from $REFINE_REPO_URL"
  ok "Cloned Refine to $checkout"
}

target_from_remote() {
  local remote="$1"
  local default_dir="$2"
  local path
  path="$(prompt "Where should the target app be cloned?" "$default_dir")"
  path="${path/#\~/$HOME}"
  if [ -d "$path/.git" ]; then
    ok "Target app checkout already exists: $path"
  elif [ -e "$path" ] && [ "$(find "$path" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l | tr -d ' ')" != "0" ]; then
    die "$path exists and is not empty. Choose another target app path."
  else
    run git clone "$remote" "$path" || die "Could not clone target app from $remote"
  fi
  TARGET_APP_PATH="$(cd "$path" 2>/dev/null && pwd -P || printf '%s\n' "$path")"
}

choose_target_app() {
  section "Target application"
  say "Refine works against your application repository. Use a local path or paste a Git remote."
  local input=""
  if [ -n "$REFINE_INSTALL_TARGET_APP" ]; then
    input="$REFINE_INSTALL_TARGET_APP"
    info "Using target app from REFINE_INSTALL_TARGET_APP"
  else
    input="$(prompt "Target app path or Git remote" "")"
  fi
  [ -n "$input" ] || die "A target app path or Git remote is required."
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
          run git -C "$input" init -q || die "Could not initialize git in $input"
        else
          die "Target app path does not exist: $input"
        fi
      fi
      if [ ! -d "$input/.git" ]; then
        if confirm "$input is not a git repo. Initialize git there" "y"; then
          run git -C "$input" init -q || die "Could not initialize git in $input"
        else
          die "Refine target apps must be git repositories."
        fi
      fi
      TARGET_APP_PATH="$(cd "$input" 2>/dev/null && pwd -P || printf '%s\n' "$input")"
      ;;
  esac
  ok "Target app: $TARGET_APP_PATH"
}

configure_refine_setting() {
  local key="$1"
  local value="$2"
  if dry_run; then
    say "${DIM}+ set Refine setting $key=$value${RESET}"
    return 0
  fi
  REFINE_SETTING_KEY="$key" REFINE_SETTING_VALUE="$value" uv run python - <<'PY'
import os
from refine_server import config, db

cfg = config.get(reload=True)
db.init_db(cfg.sqlite_path)
conn = db.connect(cfg.sqlite_path)
try:
    db.set_setting(conn, os.environ["REFINE_SETTING_KEY"], os.environ["REFINE_SETTING_VALUE"])
finally:
    conn.close()
PY
}

configure_target_app_commands() {
  if ! confirm "Configure target app run/build/status commands now" "n"; then
    info "You can configure the application later in Refine Settings -> Application."
    return 0
  fi
  local start_cmd stop_cmd rebuild_cmd status_cmd app_url
  start_cmd="$(prompt "Start command" "")"
  stop_cmd="$(prompt "Stop command" "")"
  rebuild_cmd="$(prompt "Rebuild command" "")"
  status_cmd="$(prompt "Status command" "")"
  app_url="$(prompt "Application URL" "")"
  [ -n "$start_cmd" ] && configure_refine_setting "target_app_start_command" "$start_cmd"
  [ -n "$stop_cmd" ] && configure_refine_setting "target_app_stop_command" "$stop_cmd"
  [ -n "$rebuild_cmd" ] && configure_refine_setting "target_app_rebuild_command" "$rebuild_cmd"
  [ -n "$status_cmd" ] && configure_refine_setting "target_app_status_command" "$status_cmd"
  [ -n "$app_url" ] && configure_refine_setting "target_app_url" "$app_url"
}

init_refine() {
  section "Refine project setup"
  [ -d "$REFINE_CHECKOUT" ] || die "Refine checkout missing: $REFINE_CHECKOUT"
  [ -d "$TARGET_APP_PATH" ] || die "Target app missing: $TARGET_APP_PATH"
  run cd "$REFINE_CHECKOUT" || die "Could not enter $REFINE_CHECKOUT"
  run uv run refine init "$TARGET_APP_PATH" --force || die "refine init failed"
  configure_refine_setting "agent_cli" "$SELECTED_PROVIDER"
  configure_target_app_commands
}

start_refine() {
  section "Start Refine"
  local port
  port="$(prompt "Refine port" "$REFINE_DEFAULT_PORT")"
  if has_systemd && confirm "Install Refine as a persistent system service on port $port" "y"; then
    if run uv run refine install "$port"; then
      ok "Refine installed as a persistent service"
    else
      warn "Persistent install failed. Trying non-installed background start."
      run uv run refine start "$port" || warn "Could not start Refine. Run manually: cd $REFINE_CHECKOUT && uv run refine start $port"
    fi
  else
    run uv run refine start "$port" || warn "Could not start Refine. Run manually: cd $REFINE_CHECKOUT && uv run refine start $port"
  fi
  run uv run refine status "$port" || true
  say
  ok "Open Refine: http://localhost:$port"
}

preflight() {
  section "System check"
  local python_package="python3"
  if [ "$(package_manager)" = "brew" ]; then
    python_package="python"
  fi
  if is_wsl; then
    ok "Running inside WSL"
  elif is_linux; then
    ok "Running on Linux"
  elif is_macos; then
    ok "Running on macOS"
  else
    warn "Unsupported OS: $(uname -s). Refine is tested on Linux/WSL and macOS."
  fi
  ensure_command curl curl || die "curl is required"
  ensure_command git git || die "git is required"
  ensure_command python3 "$python_package" || die "Python 3 is required"
  ensure_uv
}

provider_flow() {
  section "AI provider"
  say "Choose the agent CLI Refine should drive."
  if [ -n "$REFINE_INSTALL_PROVIDER" ]; then
    SELECTED_PROVIDER="$(printf '%s' "$REFINE_INSTALL_PROVIDER" | tr '[:upper:]' '[:lower:]')"
    case "$SELECTED_PROVIDER" in
      claude|codex|gemini|copilot) ;;
      *) die "REFINE_INSTALL_PROVIDER must be one of: claude codex gemini copilot" ;;
    esac
    info "Using provider from REFINE_INSTALL_PROVIDER: $SELECTED_PROVIDER"
  else
    SELECTED_PROVIDER="$(choice "Provider" "claude" claude codex gemini copilot)"
  fi
  ensure_provider_cli "$SELECTED_PROVIDER" || true
}

main() {
  say "${BOLD}Refine installer${RESET}"
  say "${DIM}First-run setup and non-destructive repair for Linux, macOS, and Windows via WSL.${RESET}"
  if dry_run; then
    warn "Dry run mode: commands will be printed, not executed."
  fi

  preflight

  section "Workspace"
  local base
  base="$(prompt "Install workspace" "$REFINE_INSTALL_BASE_DEFAULT")"
  base="${base/#\~/$HOME}"
  clone_or_update_refine "$base"

  provider_flow
  choose_target_app
  ensure_rootless_docker
  init_refine
  start_refine

  section "Done"
  say "Refine checkout: ${BOLD}$REFINE_CHECKOUT${RESET}"
  say "Target app:       ${BOLD}$TARGET_APP_PATH${RESET}"
  say "Provider:         ${BOLD}$SELECTED_PROVIDER${RESET}"
  say
  say "Repair later with:"
  say "  curl -fsSL $REFINE_RAW_INSTALL_URL | bash"
}

main "$@"

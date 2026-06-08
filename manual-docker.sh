#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE_FILE="$ROOT/compose.manual.yml"
SERVICE="linux"

usage() {
  cat <<'USAGE'
Usage: ./manual-docker.sh [command]

Commands:
  up       Start the manual Linux container in the background.
  shell    Open a bash shell as the refine user. Starts the container first.
  status   Show container status.
  logs     Follow container logs.
  down     Stop and remove the container.
  restart  Recreate the container, then open a shell.
  wait     Wait until the container is ready for manual testing.
  help     Show this help text.

Options:
  -h, --help  Show this help text.

The container does not install Refine. Run the install one-liner manually inside it.
First startup bootstraps Linuxbrew so the installer can use brew for dependencies.
Published browser ports: 8080, 18080, 19080.
USAGE
}

compose() {
  docker compose -f "$COMPOSE_FILE" "$@"
}

ensure_compose_file() {
  if [ ! -f "$COMPOSE_FILE" ]; then
    echo "Missing compose file: $COMPOSE_FILE" >&2
    exit 1
  fi
}

up() {
  ensure_compose_file
  compose up -d "$SERVICE"
}

wait_ready() {
  ensure_compose_file
  local deadline=$((SECONDS + 900))
  printf 'Waiting for manual Linux container setup'
  while [ "$SECONDS" -lt "$deadline" ]; do
    if compose exec -T "$SERVICE" bash -lc 'id -u refine >/dev/null 2>&1 && command -v curl >/dev/null 2>&1 && command -v brew >/dev/null 2>&1'; then
      printf '\n'
      return 0
    fi
    printf '.'
    sleep 2
  done
  printf '\n' >&2
  echo "Timed out waiting for the manual Linux container to finish setup." >&2
  echo "Recent logs:" >&2
  compose logs --tail=80 "$SERVICE" >&2 || true
  exit 1
}

shell() {
  up
  wait_ready
  compose exec --user refine "$SERVICE" bash
}

case "${1:-shell}" in
  up)
    up
    ;;
  shell)
    shell
    ;;
  status)
    ensure_compose_file
    compose ps
    ;;
  wait)
    wait_ready
    ;;
  logs)
    ensure_compose_file
    compose logs -f "$SERVICE"
    ;;
  down)
    ensure_compose_file
    compose down
    ;;
  restart)
    ensure_compose_file
    compose down
    up
    shell
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

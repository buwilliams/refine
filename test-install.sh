#!/usr/bin/env bash
set -euo pipefail

docker compose -f docker-compose.install-test.yml run --rm installer-shell

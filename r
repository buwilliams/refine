#!/usr/bin/env bash
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
exec uv --project "$ROOT/python" run refine "$@"

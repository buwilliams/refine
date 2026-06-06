#!/usr/bin/env bash
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
exec cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -- "$@"

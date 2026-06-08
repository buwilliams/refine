# Refine Rust xtask

Repository automation for the native Rust port belongs here: code generation,
API contract export, fixture refresh, release packaging, installer smoke tests,
and migration checks.

Commands:

- `cargo run --manifest-path xtask/Cargo.toml -- api-contract`
- `cargo run --manifest-path xtask/Cargo.toml -- check-static-assets`
- `cargo run --manifest-path xtask/Cargo.toml -- runtime-layout`
- `cargo run --manifest-path xtask/Cargo.toml -- test-cluster-ssh`
- `cargo run --manifest-path xtask/Cargo.toml -- check`

# Refine Rust xtask

Repository automation for the native Rust port belongs here: code generation,
API contract export, fixture refresh, release packaging, installer smoke tests,
and migration checks.

Commands:

- `cargo run --manifest-path xtask/Cargo.toml -- api-contract`
- `cargo run --manifest-path xtask/Cargo.toml -- check-static-assets`
- `cargo run --manifest-path xtask/Cargo.toml -- runtime-layout`
- `cargo run --manifest-path xtask/Cargo.toml -- release-plan patch`
- `cargo run --manifest-path xtask/Cargo.toml -- release-check`
- `cargo run --manifest-path xtask/Cargo.toml -- test-unit`
- `cargo run --manifest-path xtask/Cargo.toml -- test-integration`
- `cargo run --manifest-path xtask/Cargo.toml -- test-rust`
- `cargo run --manifest-path xtask/Cargo.toml -- test-smoke-ai`
- `cargo run --manifest-path xtask/Cargo.toml -- test-cli`
- `cargo run --manifest-path xtask/Cargo.toml -- test-cluster-ssh`
- `cargo run --manifest-path xtask/Cargo.toml -- test-install-uninstall`
- `cargo run --manifest-path xtask/Cargo.toml -- test-full-workflow`
- `cargo run --manifest-path xtask/Cargo.toml -- test-multi-instance-sync`
- `cargo run --manifest-path xtask/Cargo.toml -- test-all`
- `cargo run --manifest-path xtask/Cargo.toml -- check`

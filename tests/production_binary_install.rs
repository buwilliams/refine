use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn wrapper_auto_mode_selects_cargo_for_source_and_binary_for_deployed_checkout() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir("wrapper-mode");
    let marker = temp_root.join(".refine-deployed");
    fs::create_dir_all(&temp_root).unwrap();

    let source = Command::new("bash")
        .arg("r")
        .arg("--help")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .env("REFINE_DEPLOYED_MARKER", &marker)
        .env("REFINE_RELEASE_BIN", "/bin/echo")
        .output()
        .unwrap();
    assert!(source.status.success());
    let source_output = String::from_utf8_lossy(&source.stdout);
    assert!(source_output.contains("mode=cargo"));
    assert!(source_output.contains("command=cargo run --quiet"));

    fs::write(&marker, "mode=deployed\n").unwrap();
    let deployed = Command::new("bash")
        .arg("r")
        .arg("system")
        .arg("status")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .env("REFINE_DEPLOYED_MARKER", &marker)
        .env("REFINE_RELEASE_BIN", "/bin/echo")
        .output()
        .unwrap();
    assert!(deployed.status.success());
    let deployed_output = String::from_utf8_lossy(&deployed.stdout);
    assert!(deployed_output.contains("mode=binary"));
    assert!(deployed_output.contains("executable=/bin/echo"));
    assert!(deployed_output.contains("command=/bin/echo system status"));

    let forced = Command::new("bash")
        .arg("r")
        .arg("system")
        .arg("status")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .env("REFINE_RUN_MODE", "cargo")
        .env("REFINE_DEPLOYED_MARKER", &marker)
        .env("REFINE_RELEASE_BIN", "/bin/echo")
        .output()
        .unwrap();
    assert!(forced.status.success());
    let forced_output = String::from_utf8_lossy(&forced.stdout);
    assert!(forced_output.contains("mode=cargo"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn wrapper_test_command_routes_to_cargo_and_xtask_suites() {
    let repo = env!("CARGO_MANIFEST_DIR");

    let unit = Command::new("bash")
        .arg("r")
        .arg("test")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .output()
        .unwrap();
    assert!(unit.status.success());
    let unit_output = String::from_utf8_lossy(&unit.stdout);
    assert!(unit_output.contains("mode=test"));
    assert!(unit_output.contains(&format!(
        "command=cargo test --manifest-path {repo}/Cargo.toml"
    )));

    let full = Command::new("bash")
        .arg("r")
        .arg("test")
        .arg("full")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .env("REFINE_RUN_MODE", "binary")
        .output()
        .unwrap();
    assert!(full.status.success());
    let full_output = String::from_utf8_lossy(&full.stdout);
    assert!(full_output.contains("mode=test"));
    assert!(full_output.contains(&format!(
        "command=cargo test --manifest-path {repo}/Cargo.toml -- --full"
    )));

    let cli = Command::new("bash")
        .arg("r")
        .arg("test")
        .arg("cli")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .output()
        .unwrap();
    assert!(cli.status.success());
    let cli_output = String::from_utf8_lossy(&cli.stdout);
    assert!(cli_output.contains(&format!(
        "command=cargo run --manifest-path {repo}/xtask/Cargo.toml -- test-cli"
    )));

    let help = Command::new("bash")
        .arg("r")
        .arg("test")
        .arg("--help")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help_stderr = String::from_utf8_lossy(&help.stderr);
    assert!(
        help_stderr.contains("full                 Run all test suites and repository checks.")
    );
    assert!(!help_stderr.contains("--full"));

    let dashed_suite = Command::new("bash")
        .arg("r")
        .arg("test")
        .arg("--surface")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .output()
        .unwrap();
    assert!(!dashed_suite.status.success());
    let dashed_suite_stderr = String::from_utf8_lossy(&dashed_suite.stderr);
    assert!(dashed_suite_stderr.contains("suite names do not use -- prefixes: --surface"));

    let unknown = Command::new("bash")
        .arg("r")
        .arg("test")
        .arg("--unknown")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    let unknown_stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(unknown_stderr.contains("suite names do not use -- prefixes: --unknown"));
    assert!(unknown_stderr.contains("Usage: ./r test [SUITE]"));
}

#[test]
fn install_runbook_builds_and_installs_release_binary_before_start_commands() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let runbook = fs::read_to_string(format!("{repo}/docs/runbooks/install.md")).unwrap();

    assert!(!runbook.contains("scripts/install.sh"));
    assert!(runbook.contains("## Update Refine"));
    assert!(runbook.contains("cargo build --release --locked"));
    assert!(runbook.contains("install -m 755 target/release/refine bin/refine"));
    assert!(
        runbook.contains("printf 'mode=deployed\\nrelease_bin=bin/refine\\n' > .refine-deployed")
    );
    assert!(runbook.contains("./r system start --port <port>"));
    assert!(runbook.contains("Default: `8082`"));
    assert!(runbook.contains("Do not offer `smoke-ai` during installation"));
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

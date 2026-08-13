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
fn wrapper_system_install_builds_and_publishes_release_before_service_registration() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir("wrapper-system-install");
    let fake_bin = temp_root.join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    fs::copy(format!("{repo}/r"), temp_root.join("r")).unwrap();
    fs::write(temp_root.join("Cargo.toml"), "[package]\nname='fixture'\n").unwrap();
    let cargo_log = temp_root.join("cargo.log");
    let fake_cargo = fake_bin.join("cargo");
    fs::write(
        &fake_cargo,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" > '{}'\nmkdir -p '{}/target/release'\ncat > '{}/target/release/refine' <<'EOF'\n#!/usr/bin/env bash\nprintf 'installed-command=%s\\n' \"$*\"\nEOF\nchmod +x '{}/target/release/refine'\n",
            cargo_log.display(),
            temp_root.display(),
            temp_root.display(),
            temp_root.display(),
        ),
    )
    .unwrap();
    make_executable(&fake_cargo);
    make_executable(&temp_root.join("r"));

    let output = Command::new("bash")
        .arg("r")
        .args(["system", "install", "--port", "8082"])
        .current_dir(&temp_root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake_bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&cargo_log).unwrap().trim(),
        format!(
            "build --release --locked --target-dir {}/target --manifest-path {}/Cargo.toml",
            temp_root.display(),
            temp_root.display()
        )
    );
    assert!(temp_root.join("bin/refine").is_file());
    assert_eq!(
        fs::read_to_string(temp_root.join(".refine-deployed")).unwrap(),
        "mode=deployed\nrelease_bin=bin/refine\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "installed-command=system install --port 8082"
    );

    fs::remove_file(&cargo_log).unwrap();
    let dry_run = Command::new("bash")
        .arg("r")
        .args(["system", "install", "--port", "8082"])
        .current_dir(&temp_root)
        .env("REFINE_R_DRY_RUN", "1")
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake_bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();
    assert!(dry_run.status.success());
    assert!(!cargo_log.exists(), "dry-run must not build the release");

    let help = Command::new("bash")
        .arg("r")
        .args(["system", "install", "--port", "8082", "--help"])
        .current_dir(&temp_root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake_bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(!cargo_log.exists(), "help must not build the release");

    let installed_before = fs::read(temp_root.join("bin/refine")).unwrap();
    let marker_before = fs::read(temp_root.join(".refine-deployed")).unwrap();
    fs::write(
        &fake_cargo,
        format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" > '{}'\nexit 23\n",
            cargo_log.display()
        ),
    )
    .unwrap();
    make_executable(&fake_cargo);
    let failed = Command::new("bash")
        .arg("r")
        .args(["system", "install", "--port", "8082"])
        .current_dir(&temp_root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake_bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(23));
    assert!(
        failed.stdout.is_empty(),
        "service registration must not run"
    );
    assert_eq!(
        fs::read(temp_root.join("bin/refine")).unwrap(),
        installed_before
    );
    assert_eq!(
        fs::read(temp_root.join(".refine-deployed")).unwrap(),
        marker_before
    );

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
fn install_runbook_uses_system_install_as_the_complete_build_and_registration_boundary() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let runbook = fs::read_to_string(format!("{repo}/docs/runbooks/install.md")).unwrap();
    let gitignore = fs::read_to_string(format!("{repo}/.gitignore")).unwrap();

    assert!(!runbook.contains("scripts/install.sh"));
    assert!(runbook.contains("## Update Refine"));
    assert!(runbook.contains("./r system install --port <port>"));
    assert!(runbook.contains("builds the locked release binary"));
    assert!(runbook.contains("./r system start --port <port>"));
    assert!(runbook.contains("Default: `8082`"));
    assert!(runbook.contains("Do not offer `smoke-ai` during installation"));
    assert!(gitignore.lines().any(|line| line == "/bin/"));
    assert!(gitignore.lines().any(|line| line == "/.refine-deployed"));
}

fn make_executable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

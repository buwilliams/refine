use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn wrapper_always_selects_the_production_binary() {
    let repo = env!("CARGO_MANIFEST_DIR");

    let plain = Command::new("bash")
        .arg("r")
        .arg("--help")
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .env("REFINE_RELEASE_BIN", "/bin/echo")
        .output()
        .unwrap();
    assert!(plain.status.success());
    let plain_output = String::from_utf8_lossy(&plain.stdout);
    assert!(plain_output.contains("mode=binary"));
    assert!(plain_output.contains("executable=/bin/echo"));
    assert!(plain_output.contains("command=/bin/echo --help"));

    // The launcher never runs a debug build: the historical cargo-mode
    // override is ignored rather than honored.
    let forced = Command::new("bash")
        .arg("r")
        .args(["system", "status"])
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .env("REFINE_RUN_MODE", "cargo")
        .env("REFINE_RELEASE_BIN", "/bin/echo")
        .output()
        .unwrap();
    assert!(forced.status.success());
    let forced_output = String::from_utf8_lossy(&forced.stdout);
    assert!(forced_output.contains("mode=binary"));
    assert!(forced_output.contains("command=/bin/echo system status"));
}

#[test]
fn wrapper_system_service_install_bootstraps_only_a_missing_binary() {
    let temp_root = wrapper_fixture("wrapper-system-service-install");
    let cargo_log = temp_root.join("cargo.log");
    let path_env = fixture_path_env(&temp_root);

    // First registration: no production binary yet, so the wrapper builds and
    // publishes it before delegating to the installed binary.
    let output = Command::new("bash")
        .arg("r")
        .args(["system", "service-install", "--port", "8082"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("production binary is missing; building it before system service-install")
    );
    assert!(stdout.contains("production binary updated"));
    assert!(stdout.contains("installed-command=system service-install --port 8082"));
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

    // A later registration with unchanged source delegates without rebuilding.
    fs::remove_file(&cargo_log).unwrap();
    let unchanged = Command::new("bash")
        .arg("r")
        .args(["system", "service-install", "--port", "8082"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(unchanged.status.success());
    assert_eq!(
        String::from_utf8_lossy(&unchanged.stdout).trim(),
        "installed-command=system service-install --port 8082"
    );
    assert!(!cargo_log.exists(), "unchanged source must not rebuild");

    // Dry runs and help never build.
    let dry_run = Command::new("bash")
        .arg("r")
        .args(["system", "service-install", "--port", "8082"])
        .current_dir(&temp_root)
        .env("REFINE_R_DRY_RUN", "1")
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(dry_run.status.success());
    assert!(!cargo_log.exists(), "dry-run must not build the release");
    let help = Command::new("bash")
        .arg("r")
        .args(["system", "service-install", "--port", "8082", "--help"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(!cargo_log.exists(), "help must not build the release");

    // Changed source does not make service registration an implicit build or
    // update operation. The existing published binary remains authoritative.
    let installed_before = fs::read(temp_root.join("bin/refine")).unwrap();
    let marker_before = fs::read(temp_root.join(".refine-deployed")).unwrap();
    fs::write(temp_root.join("Cargo.toml"), "[package]\nname='fixture'\n").unwrap();
    write_failing_fake_cargo(&temp_root, &cargo_log, 23);
    let changed = Command::new("bash")
        .arg("r")
        .args(["system", "service-install", "--port", "8082"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(changed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&changed.stdout).trim(),
        "installed-command=system service-install --port 8082"
    );
    assert!(!cargo_log.exists(), "service registration must not rebuild");
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
fn wrapper_system_start_builds_missing_production_binary() {
    let temp_root = wrapper_fixture("wrapper-system-start");
    let cargo_log = temp_root.join("cargo.log");
    let path_env = fixture_path_env(&temp_root);

    let output = Command::new("bash")
        .arg("r")
        .args(["system", "start", "--port", "9099"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("production binary is missing; building it before system start"));
    assert!(stdout.contains("installed-command=system start --port 9099"));
    assert!(temp_root.join("bin/refine").is_file());

    // Unchanged source: start delegates without rebuilding.
    fs::remove_file(&cargo_log).unwrap();
    let unchanged = Command::new("bash")
        .arg("r")
        .args(["system", "start", "--port", "9099"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(unchanged.status.success());
    assert_eq!(
        String::from_utf8_lossy(&unchanged.stdout).trim(),
        "installed-command=system start --port 9099"
    );
    assert!(!cargo_log.exists(), "unchanged source must not rebuild");

    // Changed source: start rebuilds before delegating.
    fs::write(temp_root.join("Cargo.toml"), "[package]\nname='fixture'\n").unwrap();
    let rebuilt = Command::new("bash")
        .arg("r")
        .args(["system", "start", "--port", "9099"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(rebuilt.status.success());
    let rebuilt_stdout = String::from_utf8_lossy(&rebuilt.stdout);
    assert!(rebuilt_stdout.contains("source changed since the last production build"));
    assert!(rebuilt_stdout.contains("installed-command=system start --port 9099"));
    assert!(cargo_log.exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn wrapper_system_build_and_clean_manage_the_production_binary() {
    let temp_root = wrapper_fixture("wrapper-build-clean");
    let cargo_log = temp_root.join("cargo.log");
    let path_env = fixture_path_env(&temp_root);

    let build = Command::new("bash")
        .arg("r")
        .args(["system", "build"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    let build_stdout = String::from_utf8_lossy(&build.stdout);
    assert!(build_stdout.contains("building production binary from source"));
    assert!(build_stdout.contains("production binary updated"));
    assert!(temp_root.join("bin/refine").is_file());
    assert!(temp_root.join(".refine-deployed").is_file());

    // A repeat build always invokes Cargo but reports an unchanged binary.
    fs::remove_file(&cargo_log).unwrap();
    let rebuild = Command::new("bash")
        .arg("r")
        .args(["system", "build"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(rebuild.status.success());
    assert!(
        String::from_utf8_lossy(&rebuild.stdout)
            .contains("production binary is already up to date")
    );
    assert!(cargo_log.exists(), "system build always runs the build");

    let clean = Command::new("bash")
        .arg("r")
        .args(["system", "clean"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert!(clean.status.success());
    assert!(String::from_utf8_lossy(&clean.stdout).contains("removed production binary"));
    assert!(!temp_root.join("bin/refine").exists());
    assert!(!temp_root.join(".refine-deployed").exists());

    // Commands other than start/install/build never build; without the
    // production binary they fail with guidance instead of falling back to a
    // debug build.
    fs::remove_file(&cargo_log).unwrap();
    let status = Command::new("bash")
        .arg("r")
        .args(["system", "status"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .output()
        .unwrap();
    assert_eq!(status.status.code(), Some(127));
    let status_stderr = String::from_utf8_lossy(&status.stderr);
    assert!(status_stderr.contains("production binary is missing"));
    assert!(status_stderr.contains("./r system build"));
    assert!(!cargo_log.exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn wrapper_system_update_runs_the_fixed_local_sequence() {
    let temp_root = wrapper_fixture("wrapper-system-update");
    let operation_log = temp_root.join("operation.log");
    let path_env = fixture_path_env(&temp_root);

    fs::create_dir_all(temp_root.join("bin")).unwrap();
    fs::write(
        temp_root.join("bin/refine"),
        "#!/usr/bin/env bash\nprintf 'refine:%s\\n' \"$*\" >> \"$REFINE_TEST_OPERATION_LOG\"\n",
    )
    .unwrap();
    make_executable(&temp_root.join("bin/refine"));

    let fake_git = temp_root.join("fake-bin/git");
    fs::write(
        &fake_git,
        "#!/usr/bin/env bash\nprintf 'git:%s\\n' \"$*\" >> \"$REFINE_TEST_OPERATION_LOG\"\nif [ \"${1:-}\" = \"rev-list\" ]; then printf '1\\n'; fi\n",
    )
    .unwrap();
    make_executable(&fake_git);

    let fake_cargo = temp_root.join("fake-bin/cargo");
    fs::write(
        &fake_cargo,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf 'cargo:%s\\n' \"$*\" >> \"$REFINE_TEST_OPERATION_LOG\"\nmkdir -p '{0}/target/release'\ncat > '{0}/target/release/refine' <<'EOF'\n#!/usr/bin/env bash\nprintf 'refine:%s\\n' \"$*\" >> \"$REFINE_TEST_OPERATION_LOG\"\nEOF\nchmod +x '{0}/target/release/refine'\n",
            temp_root.display(),
        ),
    )
    .unwrap();
    make_executable(&fake_cargo);

    let output = Command::new("bash")
        .arg("r")
        .args(["system", "update"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .env("REFINE_TEST_OPERATION_LOG", &operation_log)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let operations = fs::read_to_string(&operation_log).unwrap();
    let lines: Vec<_> = operations.lines().collect();
    assert_eq!(lines[0], "git:fetch --quiet");
    assert_eq!(lines[1], "git:rev-list --count HEAD..@{upstream}");
    assert_eq!(lines[2], "refine:system stop");
    assert_eq!(lines[3], "git:stash");
    assert_eq!(lines[4], "git:pull");
    assert!(lines[5].starts_with("cargo:build --release --locked"));
    assert_eq!(lines[6], "refine:system start");
    assert_eq!(lines.len(), 7, "unexpected operations:\n{operations}");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn wrapper_system_update_does_nothing_when_upstream_has_no_new_commits() {
    let temp_root = wrapper_fixture("wrapper-system-update-current");
    let operation_log = temp_root.join("operation.log");
    let path_env = fixture_path_env(&temp_root);

    fs::create_dir_all(temp_root.join("bin")).unwrap();
    fs::write(
        temp_root.join("bin/refine"),
        "#!/usr/bin/env bash\nprintf 'refine:%s\\n' \"$*\" >> \"$REFINE_TEST_OPERATION_LOG\"\n",
    )
    .unwrap();
    make_executable(&temp_root.join("bin/refine"));

    let fake_git = temp_root.join("fake-bin/git");
    fs::write(
        &fake_git,
        "#!/usr/bin/env bash\nprintf 'git:%s\\n' \"$*\" >> \"$REFINE_TEST_OPERATION_LOG\"\nif [ \"${1:-}\" = \"rev-list\" ]; then printf '0\\n'; fi\n",
    )
    .unwrap();
    make_executable(&fake_git);

    let output = Command::new("bash")
        .arg("r")
        .args(["system", "update"])
        .current_dir(&temp_root)
        .env("PATH", &path_env)
        .env("REFINE_TEST_OPERATION_LOG", &operation_log)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "refine: already up to date; no update required\n"
    );
    assert_eq!(
        fs::read_to_string(&operation_log).unwrap(),
        "git:fetch --quiet\ngit:rev-list --count HEAD..@{upstream}\n"
    );
    assert!(!temp_root.join("cargo.log").exists());

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn wrapper_system_update_dry_run_reports_the_fixed_sequence() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("bash")
        .arg("r")
        .args(["system", "update"])
        .current_dir(repo)
        .env("REFINE_R_DRY_RUN", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "mode=update\ncommand=git fetch --quiet\ncommand=git rev-list --count HEAD..@{upstream}\ncondition=continue only when upstream has new commits\ncommand=./r system stop\ncommand=git stash\ncommand=git pull\ncommand=./r system build\ncommand=./r system start\n"
    );
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
fn install_runbook_distinguishes_product_installation_from_service_registration() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let runbook = fs::read_to_string(format!("{repo}/docs/runbooks/install.md")).unwrap();
    let gitignore = fs::read_to_string(format!("{repo}/.gitignore")).unwrap();

    assert!(!runbook.contains("scripts/install.sh"));
    assert!(runbook.contains("## Update Refine"));
    assert!(runbook.contains("./r system service-install --port <port>"));
    assert!(runbook.contains("./r system service-uninstall --port <port>"));
    assert!(!runbook.contains("./r system install --port"));
    assert!(!runbook.contains("./r system uninstall --port"));
    assert!(runbook.contains("bootstraps the locked release binary"));
    assert!(runbook.contains("./r system start --port <port>"));
    assert!(runbook.contains("Default: `8082`"));
    assert!(runbook.contains("Do not offer `smoke-ai` during installation"));
    assert!(runbook.contains("./r system build"));
    assert!(runbook.contains("./r system clean"));
    assert!(gitignore.lines().any(|line| line == "/bin/"));
    assert!(gitignore.lines().any(|line| line == "/.refine-deployed"));
}

/// A minimal checkout: the launcher script, a Cargo manifest, and a fake
/// `cargo` on PATH whose `build` writes a shell script standing in for the
/// release binary and records its arguments.
fn wrapper_fixture(prefix: &str) -> std::path::PathBuf {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir(prefix);
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
    temp_root
}

fn fixture_path_env(temp_root: &std::path::Path) -> String {
    format!(
        "{}:{}",
        temp_root.join("fake-bin").display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn write_failing_fake_cargo(temp_root: &std::path::Path, cargo_log: &std::path::Path, code: i32) {
    let fake_cargo = temp_root.join("fake-bin/cargo");
    fs::write(
        &fake_cargo,
        format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" > '{}'\nexit {code}\n",
            cargo_log.display()
        ),
    )
    .unwrap();
    make_executable(&fake_cargo);
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

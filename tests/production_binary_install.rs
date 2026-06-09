use std::fs;
#[cfg(unix)]
use std::os::unix::fs as unix_fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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
fn install_dry_run_builds_and_installs_release_binary_before_start_commands() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir("install-dry-run");
    let checkout = temp_root.join("refine");
    let log = temp_root.join("install.log");
    fs::create_dir_all(&temp_root).unwrap();

    let output = Command::new("bash")
        .arg(format!("{repo}/scripts/install.sh"))
        .arg("--yes")
        .current_dir(&temp_root)
        .env("REFINE_INSTALL_DRY_RUN", "1")
        .env("REFINE_INSTALL_ASSUME_DEFAULTS", "1")
        .env("REFINE_INSTALL_CHECKOUT_DEFAULT", &checkout)
        .env("REFINE_INSTALL_LOG", &log)
        .env("REFINE_INSTALL_ALLOW_TEST_PROVIDERS", "1")
        .env("REFINE_INSTALL_PROVIDER", "smoke-ai")
        .env("REFINE_INSTALL_TARGET_APP", "")
        .env("REFINE_REPO_URL", "https://example.invalid/refine.git")
        .env("REFINE_INSTALL_DRY_RUN_RELEASE_TAG", "9.8.7")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "installer failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log = fs::read_to_string(&log).unwrap();
    assert!(log.contains("cargo build --release --locked"));
    assert!(log.contains(&format!(
        "install -m 755 {}/target/release/refine {}/bin/refine",
        checkout.display(),
        checkout.display()
    )));
    assert!(log.contains(&format!(
        "write deployed marker {}/.refine-deployed",
        checkout.display()
    )));
    assert!(
        log.contains("./r system install --target linux-cli-web --port 8080 --runtime-root run")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn install_update_only_dry_run_builds_repairs_and_skips_start_commands() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir("install-update-only-dry-run");
    let runtime_root = temp_root.join("run");
    let log = temp_root.join("install.log");
    fs::create_dir_all(runtime_root.join("19080")).unwrap();
    fs::write(runtime_root.join("19080").join("install-state.json"), "{}").unwrap();

    let output = Command::new("bash")
        .arg(format!("{repo}/scripts/install.sh"))
        .arg("--yes")
        .arg("--upgrade")
        .current_dir(repo)
        .env("REFINE_INSTALL_DRY_RUN", "1")
        .env("REFINE_INSTALL_ASSUME_DEFAULTS", "1")
        .env("REFINE_INSTALL_UPDATE_ONLY", "1")
        .env("REFINE_INSTALL_CHECKOUT_DEFAULT", repo)
        .env("REFINE_INSTALL_RUNTIME_ROOT", &runtime_root)
        .env("REFINE_INSTALL_LOG", &log)
        .env("REFINE_REPO_URL", "https://example.invalid/refine.git")
        .env("REFINE_INSTALL_DRY_RUN_RELEASE_TAG", "9.8.7")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "installer failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log = fs::read_to_string(&log).unwrap();
    assert!(log.contains("cargo build --release --locked"));
    assert!(log.contains(&format!(
        "install -m 755 {repo}/target/release/refine {repo}/bin/refine"
    )));
    assert!(log.contains(&format!("write deployed marker {repo}/.refine-deployed")));
    assert!(log.contains("./r system repair --port 19080 --runtime-root"));
    assert!(!log.contains("./r system start"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
#[cfg(unix)]
fn install_update_only_dry_run_refreshes_unpacked_checkout() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir("install-unpacked-update-dry-run");
    let checkout = temp_root.join("refine");
    let log = temp_root.join("install.log");
    create_minimal_refine_checkout(&checkout);

    let output = Command::new("bash")
        .arg(format!("{repo}/scripts/install.sh"))
        .arg("--yes")
        .arg("--upgrade")
        .current_dir(&temp_root)
        .env("REFINE_INSTALL_DRY_RUN", "1")
        .env("REFINE_INSTALL_ASSUME_DEFAULTS", "1")
        .env("REFINE_INSTALL_UPDATE_ONLY", "1")
        .env("REFINE_INSTALL_CHECKOUT_DEFAULT", &checkout)
        .env("REFINE_INSTALL_LOG", &log)
        .env("REFINE_REPO_URL", "https://example.invalid/refine.git")
        .env("REFINE_INSTALL_DRY_RUN_RELEASE_TAG", "9.8.7")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "installer failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let log = fs::read_to_string(&log).unwrap();
    assert!(log.contains("Refine unpacked checkout exists"));
    assert!(log.contains("remove release-owned files"));
    assert!(log.contains("copy release files from /tmp/refine-release"));
    assert!(!log.contains("exists but is not a git checkout"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn install_rejects_smoke_ai_without_test_provider_gate() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir("install-reject-smoke-ai");
    let checkout = temp_root.join("refine");
    let log = temp_root.join("install.log");
    fs::create_dir_all(&temp_root).unwrap();

    let output = Command::new("bash")
        .arg(format!("{repo}/scripts/install.sh"))
        .arg("--yes")
        .current_dir(&temp_root)
        .env("REFINE_INSTALL_DRY_RUN", "1")
        .env("REFINE_INSTALL_ASSUME_DEFAULTS", "1")
        .env("REFINE_INSTALL_CHECKOUT_DEFAULT", &checkout)
        .env("REFINE_INSTALL_LOG", &log)
        .env("REFINE_INSTALL_PROVIDER", "smoke-ai")
        .env("REFINE_REPO_URL", "https://example.invalid/refine.git")
        .env("REFINE_INSTALL_DRY_RUN_RELEASE_TAG", "9.8.7")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "installer unexpectedly accepted smoke-ai: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&log).unwrap_or_default()
    );
    assert!(
        combined.contains("REFINE_INSTALL_PROVIDER must be one of: claude codex gemini copilot")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
#[cfg(unix)]
fn installer_reports_missing_core_dependencies_and_exits_without_homebrew() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir("install-missing-deps");
    let checkout = temp_root.join("refine");
    let log = temp_root.join("install.log");
    let path = command_path(&temp_root, &["dirname", "grep", "tr", "uname"]);
    fs::create_dir_all(&temp_root).unwrap();

    let output = Command::new("/bin/bash")
        .arg(format!("{repo}/scripts/install.sh"))
        .current_dir(&temp_root)
        .stdin(std::process::Stdio::null())
        .env_clear()
        .env("HOME", &temp_root)
        .env("PATH", &path)
        .env("REFINE_INSTALL_DRY_RUN", "1")
        .env("REFINE_INSTALL_CHECKOUT_DEFAULT", &checkout)
        .env("REFINE_INSTALL_LOG", &log)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "installer unexpectedly succeeded: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&log).unwrap_or_default()
    );
    assert!(combined.contains("Missing required install dependencies"));
    assert!(combined.contains("curl"));
    assert!(combined.contains("git"));
    assert!(combined.contains("C compiler/linker"));
    assert!(combined.contains("Rust Cargo"));
    assert!(combined.contains("Install Homebrew to manage these dependencies"));
    assert!(
        combined.contains("Install the missing dependencies manually, then re-run install.sh.")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
#[cfg(unix)]
fn installer_yes_dry_run_bootstraps_homebrew_for_missing_core_dependencies() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let temp_root = unique_temp_dir("install-homebrew-dry-run");
    let checkout = temp_root.join("refine");
    let log = temp_root.join("install.log");
    let path = command_path(&temp_root, &["curl", "dirname", "grep", "tr", "uname"]);
    fs::create_dir_all(&temp_root).unwrap();

    let output = Command::new("/bin/bash")
        .arg(format!("{repo}/scripts/install.sh"))
        .arg("--yes")
        .current_dir(&temp_root)
        .stdin(std::process::Stdio::null())
        .env_clear()
        .env("HOME", &temp_root)
        .env("PATH", &path)
        .env("REFINE_INSTALL_DRY_RUN", "1")
        .env("REFINE_INSTALL_CHECKOUT_DEFAULT", &checkout)
        .env("REFINE_INSTALL_LOG", &log)
        .env("REFINE_REPO_URL", "https://example.invalid/refine.git")
        .env("REFINE_INSTALL_DRY_RUN_RELEASE_TAG", "9.8.7")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "installer unexpectedly succeeded without real git/cargo: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&log).unwrap_or_default()
    );
    assert!(combined.contains("NONINTERACTIVE=1 bash <(curl -fsSL"));
    assert!(combined.contains("brew install git"));

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

#[cfg(unix)]
fn command_path(root: &Path, commands: &[&str]) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for command in commands {
        let target = format!("/usr/bin/{command}");
        unix_fs::symlink(&target, bin.join(command)).unwrap_or_else(|error| {
            panic!("failed to symlink {command} from {target}: {error}");
        });
    }
    bin
}

#[cfg(unix)]
fn create_minimal_refine_checkout(path: &Path) {
    fs::create_dir_all(path.join("src")).unwrap();
    fs::create_dir_all(path.join("scripts")).unwrap();
    fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"refine\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    fs::write(path.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(path.join("scripts/install.sh"), "#!/bin/sh\n").unwrap();
    fs::write(path.join("r"), "#!/bin/sh\n").unwrap();
    let mut permissions = fs::metadata(path.join("r")).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path.join("r"), permissions).unwrap();
}

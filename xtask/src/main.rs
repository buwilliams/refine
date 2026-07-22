use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use refine::process::supervisor::runtime::{DEFAULT_APP_ID, RuntimePathLayout};
use refine::surfaces::web_server::{API_CONTRACT_VERSION, API_GROUPS};
use refine::tools::host::release::{FileReleaseService, ReleaseBump};
use serde_json::json;

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("api-contract") => print_api_contract(),
        Some("cli-reference") => write_cli_reference(),
        Some("check-static-assets") => check_static_assets(),
        Some("runtime-layout") => print_runtime_layout(),
        Some("release-plan") => release_plan(),
        Some("release-check") => release_check(),
        Some("test-unit") => test_unit(),
        Some("test-integration") => test_integration(),
        Some("test-rust") => test_rust(),
        Some("test-smoke-ai") => test_smoke_ai(),
        Some("test-cli") => test_cli(),
        Some("test-browser") => test_browser(),
        Some("test-cluster-ssh") => test_cluster_ssh(),
        Some("test-install-uninstall") => test_install_uninstall(),
        Some("test-full-workflow") => test_full_workflow(),
        Some("test-multi-instance-sync") => test_multi_instance_sync(),
        Some("test-all") => test_all(),
        Some("check") | None => check_all(),
        Some(command) => Err(format!("unknown xtask command: {command}")),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn release_plan() -> Result<(), String> {
    let root = repo_root()?;
    let bump = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "patch".to_string());
    let bump = ReleaseBump::parse(&bump).map_err(|error| error.to_string())?;
    let plan = FileReleaseService::new(&root, root.join("run"))
        .plan(bump)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&plan).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn release_check() -> Result<(), String> {
    let root = repo_root()?;
    let checks = [
        ("format", vec!["fmt", "--all", "--", "--check"]),
        (
            "unit tests",
            vec!["test", "--lib", "--bins", "--", "--test-threads=1"],
        ),
        (
            "locked release build",
            vec!["build", "--release", "--locked"],
        ),
    ];
    for (label, args) in checks {
        run(Command::new("cargo").args(args).current_dir(&root), label)?;
    }
    check_git_diff()?;
    println!("{}", serde_json::to_string_pretty(&json!({
        "release_ready": true,
        "checks": ["cargo fmt --all -- --check", "cargo test --lib --bins -- --test-threads=1", "cargo build --release --locked", "git diff --check"]
    })).map_err(|error| error.to_string())?);
    Ok(())
}

fn check_all() -> Result<(), String> {
    print_api_contract()?;
    check_static_assets()?;
    test_browser()?;
    print_runtime_layout()
}

fn test_all() -> Result<(), String> {
    test_unit()?;
    test_doc()?;
    check_all()?;
    test_integration()?;
    check_git_diff()
}

fn test_unit() -> Result<(), String> {
    let repo_root = repo_root()?;
    run(
        Command::new("cargo")
            .args(["test", "--lib", "--bins", "--", "--test-threads=1"])
            .current_dir(&repo_root),
        "run Rust unit tests",
    )
}

fn test_rust() -> Result<(), String> {
    test_unit()?;
    test_cargo_integrations()?;
    test_doc()
}

fn test_doc() -> Result<(), String> {
    let repo_root = repo_root()?;
    run(
        Command::new("cargo")
            .args(["test", "--doc"])
            .current_dir(&repo_root),
        "run Rust doc tests",
    )
}

fn test_cargo_integrations() -> Result<(), String> {
    let repo_root = repo_root()?;
    run(
        Command::new("cargo")
            .args([
                "test",
                "--test",
                "cli_target_root",
                "--test",
                "production_binary_install",
                "--",
                "--test-threads=1",
            ])
            .current_dir(&repo_root),
        "run Cargo integration tests",
    )
}

fn test_integration() -> Result<(), String> {
    test_cargo_integrations()?;
    test_smoke_ai()?;
    test_cli()?;
    test_cluster_ssh()?;
    test_install_uninstall()?;
    test_full_workflow()?;
    test_multi_instance_sync()
}

fn test_smoke_ai() -> Result<(), String> {
    let repo_root = repo_root()?;
    run(
        Command::new("cargo")
            .args([
                "build",
                "--manifest-path",
                "tests/fixtures/smoke-ai/Cargo.toml",
            ])
            .current_dir(&repo_root),
        "build smoke-ai fixture",
    )?;
    let smoke_ai = fixture_binary_path(&repo_root, "smoke-ai");
    let stderr_path = repo_root.join("target/refine-integration/artifacts/smoke-ai/stderr.log");
    if let Some(parent) = stderr_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create smoke-ai artifact directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let mut command = Command::new("cargo");
    command
        .args(["test", "--test", "smoke_ai_contract", "--", "--nocapture"])
        .current_dir(&repo_root)
        .env("REFINE_SMOKE_AI_PATH", &smoke_ai)
        .env("SMOKE_AI_DEBUG", "1")
        .env("REFINE_SMOKE_AI_STDERR_ARTIFACT", &stderr_path);
    run(&mut command, "run smoke-ai contract")
}

fn test_cli() -> Result<(), String> {
    let repo_root = repo_root()?;
    let smoke_ai = ensure_smoke_ai_built(&repo_root)?;
    let mut command = Command::new("cargo");
    command
        .args([
            "test",
            "--test",
            "cli_surface",
            "--",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .current_dir(&repo_root)
        .env("REFINE_TEST_PORT", test_port())
        .env("REFINE_DAEMON_PORT", test_port())
        .env("REFINE_SMOKE_AI_PATH", smoke_ai);
    run(&mut command, "run CLI surface tests")
}

fn test_browser() -> Result<(), String> {
    let repo_root = repo_root()?;
    let tests_root = repo_root.join("tests");
    let mut tests = Vec::new();
    for entry in fs::read_dir(&tests_root)
        .map_err(|error| format!("failed to read {}: {error}", tests_root.display()))?
    {
        let path = entry
            .map_err(|error| {
                format!(
                    "failed to inspect an entry under {}: {error}",
                    tests_root.display()
                )
            })?
            .path();
        if path.to_string_lossy().ends_with(".test.js") {
            tests.push(path);
        }
    }
    tests.sort();
    if tests.is_empty() {
        return Err(format!(
            "no browser JavaScript tests found under {}",
            tests_root.display()
        ));
    }
    let mut command = Command::new("node");
    command.arg("--test").args(tests).current_dir(&repo_root);
    run(&mut command, "run browser JavaScript tests")
}

fn test_cluster_ssh() -> Result<(), String> {
    let repo_root = repo_root()?;
    let smoke_ai = ensure_smoke_ai_built(&repo_root)?;
    let mut command = Command::new("cargo");
    command
        .args([
            "test",
            "--test",
            "cluster_ssh_cli",
            "--",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .current_dir(&repo_root)
        .env("REFINE_TEST_PORT", test_port())
        .env("REFINE_DAEMON_PORT", test_port())
        .env("REFINE_SMOKE_AI_PATH", smoke_ai);
    run(&mut command, "run SSH-backed cluster CLI tests")
}

fn test_install_uninstall() -> Result<(), String> {
    let repo_root = repo_root()?;
    let mut command = Command::new("cargo");
    command
        .args([
            "test",
            "--test",
            "install_uninstall_docker",
            "--",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .current_dir(&repo_root);
    run(&mut command, "run Docker-backed install/uninstall tests")
}

fn test_full_workflow() -> Result<(), String> {
    let repo_root = repo_root()?;
    let smoke_ai = ensure_smoke_ai_built(&repo_root)?;
    let mut command = Command::new("cargo");
    command
        .args([
            "test",
            "--test",
            "full_workflow",
            "--",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .current_dir(&repo_root)
        .env("REFINE_TEST_PORT", test_port())
        .env("REFINE_DAEMON_PORT", test_port())
        .env("REFINE_SMOKE_AI_PATH", smoke_ai);
    run(&mut command, "run full workflow integration test")
}

fn test_multi_instance_sync() -> Result<(), String> {
    let repo_root = repo_root()?;
    let smoke_ai = ensure_smoke_ai_built(&repo_root)?;
    let mut command = Command::new("cargo");
    command
        .args([
            "test",
            "--test",
            "multi_instance_sync",
            "--",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .current_dir(&repo_root)
        .env("REFINE_SMOKE_AI_PATH", smoke_ai);
    run(&mut command, "run multi-instance sync tests")
}

fn print_api_contract() -> Result<(), String> {
    let groups = API_GROUPS
        .iter()
        .map(|group| json!({"prefix": group.prefix, "capability": group.capability}))
        .collect::<Vec<_>>();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "product": "refine",
            "api_contract_version": API_CONTRACT_VERSION,
            "groups": groups
        }))
        .map_err(|error| format!("failed to encode API contract: {error}"))?
    );
    Ok(())
}

fn write_cli_reference() -> Result<(), String> {
    let repo_root = repo_root()?;
    let path = repo_root.join("docs/spec/cli-reference.md");
    let markdown = refine::surfaces::cli::command_reference_markdown();
    fs::write(&path, &markdown)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    println!("wrote {} ({} bytes)", path.display(), markdown.len());
    Ok(())
}

fn print_runtime_layout() -> Result<(), String> {
    let layout = RuntimePathLayout::current_user(DEFAULT_APP_ID);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "deployment": layout.deployment,
            "os": layout.os,
            "app_support_dir": layout.app_support_dir,
            "runtime_root": layout.runtime_root,
            "cache_dir": layout.cache_dir,
            "logs_dir": layout.logs_dir,
            "service_metadata_path": layout.service_metadata_path
        }))
        .map_err(|error| format!("failed to encode runtime layout: {error}"))?
    );
    Ok(())
}

fn check_static_assets() -> Result<(), String> {
    let repo_root = repo_root()?;
    let static_root = repo_root.join("src/surfaces/web/static");
    let assets = collect_files(&static_root)?;
    if assets.is_empty() {
        return Err(format!(
            "no static assets found under {}",
            static_root.display()
        ));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "static_assets_present": true,
            "files": assets.len()
        }))
        .map_err(|error| format!("failed to encode static asset report: {error}"))?
    );
    Ok(())
}

fn repo_root() -> Result<PathBuf, String> {
    let mut current =
        std::env::current_dir().map_err(|error| format!("failed to inspect cwd: {error}"))?;
    loop {
        if current.join("docs/spec/rust-spec.md").is_file() {
            return Ok(current);
        }
        if !current.pop() {
            return Err("failed to locate repository root from cwd".to_string());
        }
    }
}

fn ensure_smoke_ai_built(repo_root: &Path) -> Result<PathBuf, String> {
    run(
        Command::new("cargo")
            .args([
                "build",
                "--manifest-path",
                "tests/fixtures/smoke-ai/Cargo.toml",
            ])
            .current_dir(repo_root),
        "build smoke-ai fixture",
    )?;
    Ok(fixture_binary_path(repo_root, "smoke-ai"))
}

fn fixture_binary_path(repo_root: &Path, name: &str) -> PathBuf {
    repo_root
        .join("tests/fixtures")
        .join(name)
        .join("target/debug")
        .join(executable_name(name))
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn test_port() -> String {
    std::env::var("REFINE_TEST_PORT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "18080".to_string())
}

fn check_git_diff() -> Result<(), String> {
    let repo_root = repo_root()?;
    run(
        Command::new("git")
            .args(["diff", "--check"])
            .current_dir(&repo_root),
        "check git diff whitespace",
    )
}

fn run(command: &mut Command, label: &str) -> Result<(), String> {
    eprintln!("==> {label}");
    let status = command
        .status()
        .map_err(|error| format!("{label}: failed to start command: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} failed with status {status}"))
    }
}

fn collect_files(root: &Path) -> Result<BTreeMap<String, u64>, String> {
    let mut files = BTreeMap::new();
    collect_files_inner(root, root, &mut files)?;
    Ok(files)
}

fn collect_files_inner(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, u64>,
) -> Result<(), String> {
    for entry in fs::read_dir(current)
        .map_err(|error| format!("failed to read {}: {error}", current.display()))?
    {
        let entry = entry.map_err(|error| {
            format!(
                "failed to inspect static asset under {}: {error}",
                current.display()
            )
        })?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|error| format!("failed to stat {}: {error}", path.display()))?;
        if metadata.is_dir() {
            collect_files_inner(root, &path, files)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("failed to relativize {}: {error}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        files.insert(relative, metadata.len());
    }
    Ok(())
}

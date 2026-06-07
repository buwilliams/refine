use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use refine::core::supervisor::runtime::{DEFAULT_APP_ID, RuntimePathLayout};
use refine::surfaces::web_server::{API_CONTRACT_VERSION, API_GROUPS};
use serde_json::json;

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("api-contract") => print_api_contract(),
        Some("check-static-assets") => check_static_assets(),
        Some("runtime-layout") => print_runtime_layout(),
        Some("test-smoke-ai") => test_smoke_ai(),
        Some("test-cli") => test_cli(),
        Some("test-ui") => test_ui(),
        Some("test-surface") => test_surface(),
        Some("check") | None => check_all(),
        Some(command) => Err(format!("unknown xtask command: {command}")),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn check_all() -> Result<(), String> {
    print_api_contract()?;
    check_static_assets()?;
    print_runtime_layout()
}

fn test_surface() -> Result<(), String> {
    test_smoke_ai()?;
    test_cli()?;
    test_ui()
}

fn test_smoke_ai() -> Result<(), String> {
    let repo_root = repo_root()?;
    run(
        Command::new("cargo")
            .args(["build", "--manifest-path", "tests/fixtures/smoke-ai/Cargo.toml"])
            .current_dir(&repo_root),
        "build smoke-ai fixture",
    )?;
    let smoke_ai = fixture_binary_path(&repo_root, "smoke-ai");
    let stderr_path = repo_root
        .join("target/refine-integration/artifacts/smoke-ai/stderr.log");
    if let Some(parent) = stderr_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("failed to create smoke-ai artifact directory {}: {error}", parent.display())
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

fn test_ui() -> Result<(), String> {
    let repo_root = repo_root()?;
    let smoke_ai = ensure_smoke_ai_built(&repo_root)?;
    run(
        Command::new("cargo")
            .args(["build", "--bin", "refine"])
            .current_dir(&repo_root),
        "build refine binary",
    )?;
    ensure_playwright_package(&repo_root)?;
    let refine_bin = repo_root
        .join("target/debug")
        .join(executable_name("refine"));
    let mut command = Command::new("npx");
    command
        .args(["playwright", "test", "--config", "playwright.config.ts"])
        .current_dir(&repo_root)
        .env("REFINE_TEST_REFINE_BIN", refine_bin)
        .env("REFINE_TEST_PORT", test_port())
        .env("REFINE_DAEMON_PORT", test_port())
        .env("REFINE_TEST_BASE_URL", format!("http://127.0.0.1:{}", test_port()))
        .env("REFINE_SMOKE_AI_PATH", smoke_ai);
    run(&mut command, "run Playwright UI tests").map_err(|error| {
        if error.contains("Executable doesn't exist") || error.contains("browserType.launch") {
            format!(
                "{error}\nPlaywright browsers are missing. Install them with `npx playwright install --with-deps chromium` and rerun `cargo run --manifest-path xtask/Cargo.toml -- test-ui`."
            )
        } else {
            error
        }
    })
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
        return Err(format!("no static assets found under {}", static_root.display()));
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
            .args(["build", "--manifest-path", "tests/fixtures/smoke-ai/Cargo.toml"])
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

fn ensure_playwright_package(repo_root: &Path) -> Result<(), String> {
    if repo_root.join("node_modules/@playwright/test").is_dir() {
        return Ok(());
    }
    if !repo_root.join("package-lock.json").is_file() {
        run(
            Command::new("npm")
                .args(["install", "--package-lock-only"])
                .current_dir(repo_root),
            "create package-lock.json",
        )?;
    }
    run(
        Command::new("npm").args(["install"]).current_dir(repo_root),
        "install Playwright npm dependencies",
    )
}

fn run(command: &mut Command, label: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|error| format!("{label}: failed to start command: {error}"))?;
    if output.status.success() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        Ok(())
    } else {
        Err(format!(
            "{label} failed with status {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
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

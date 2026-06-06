use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use refine_native::core::supervisor::runtime::{DEFAULT_APP_ID, RuntimePathLayout};
use refine_native::surfaces::web_server::{API_CONTRACT_VERSION, API_GROUPS};
use serde_json::json;

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("api-contract") => print_api_contract(),
        Some("check-static-assets") => check_static_assets(),
        Some("runtime-layout") => print_runtime_layout(),
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

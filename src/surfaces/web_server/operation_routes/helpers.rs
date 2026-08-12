use super::*;

pub(in crate::surfaces::web_server) fn diagnostics_cache_key(
    runtime_root: &std::path::Path,
    refine_dir: Option<&PathBuf>,
    repo_root: &std::path::Path,
) -> String {
    format!(
        "{}|{}|{}",
        runtime_root.display(),
        refine_dir
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_string()),
        repo_root.display()
    )
}

pub(in crate::surfaces::web_server) fn live_process_summary(
    runtime_root: &std::path::Path,
    refine_dir: Option<&std::path::Path>,
) -> RefineResult<Value> {
    match refine_dir {
        Some(refine_dir) => {
            FileProcessStatusService::with_refine_dir(runtime_root, refine_dir).summary()
        }
        None => FileProcessStatusService::new(runtime_root).summary(),
    }
}

pub(in crate::surfaces::web_server) fn secret_scope_name_from_path(
    path: &str,
) -> Option<(String, String)> {
    let rest = path.strip_prefix("/agents/secrets/")?;
    let mut parts = rest.split('/');
    let scope = parts.next()?.trim();
    let name = parts.next()?.trim();
    if scope.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some((scope.to_string(), name.to_string()))
}

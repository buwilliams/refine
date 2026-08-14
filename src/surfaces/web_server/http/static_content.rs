use super::*;

pub(super) fn warm_static_cache_dir(static_root: &Path, root: &Path) -> RefineResult<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).map_err(|error| {
        RefineError::Io(format!(
            "failed to read static directory {}: {error}",
            root.display()
        ))
    })? {
        let entry = entry
            .map_err(|error| RefineError::Io(format!("failed to read static entry: {error}")))?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            RefineError::Io(format!(
                "failed to stat static asset {}: {error}",
                path.display()
            ))
        })?;
        if metadata.is_dir() {
            warm_static_cache_dir(static_root, &path)?;
        } else if metadata.is_file() {
            let _ = cached_static_asset(static_root, &path)?;
        }
    }
    Ok(())
}

pub(super) fn cached_static_asset(
    static_root: &Path,
    full_path: &Path,
) -> RefineResult<StaticAssetCacheEntry> {
    let metadata = fs::metadata(full_path).map_err(|error| {
        RefineError::Io(format!(
            "failed to stat static asset {}: {error}",
            full_path.display()
        ))
    })?;
    let modified = metadata.modified().ok();
    let key = static_asset_key(static_root, full_path);
    let cache = STATIC_ASSET_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    {
        let cache = cache
            .lock()
            .map_err(|_| RefineError::Io("static asset cache lock was poisoned".to_string()))?;
        if let Some(entry) = cache.get(&key)
            && entry.modified == modified
        {
            return Ok(entry.clone());
        }
    }

    let entry = StaticAssetCacheEntry {
        modified,
        content_type: content_type_for_path(full_path).to_string(),
        bytes: fs::read(full_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read static asset {}: {error}",
                full_path.display()
            ))
        })?,
    };
    cache
        .lock()
        .map_err(|_| RefineError::Io("static asset cache lock was poisoned".to_string()))?
        .insert(key, entry.clone());
    Ok(entry)
}

pub(super) fn static_asset_key(static_root: &Path, full_path: &Path) -> String {
    format!("{}|{}", static_root.display(), full_path.display())
}

pub(super) fn record_http_request_metric(
    server: &InProcessWebServer,
    request: &HttpRequest,
    response: &WireResponse,
    cache_mode: &'static str,
    elapsed_ms: f64,
) -> bool {
    let Some(runtime_root) = server.runtime_root.clone() else {
        return false;
    };
    if !screen_critical_http_request(&request.method, &request.path, cache_mode) {
        return false;
    }
    let status = response.status;
    // Recording is an in-memory ring push; no writer thread or disk involved.
    let _ = FileMetricsService::new(runtime_root).record_operation(
        "http.request",
        elapsed_ms,
        status < 400,
        json!({
            "method": request.method,
            "path": normalize_api_path(&request.path),
            "raw_path": request.path,
            "status": status,
            "bytes": response.body.len() as u64,
            "cache_mode": cache_mode,
            "budget_ms": 50.0,
            "over_budget": elapsed_ms > 50.0
        }),
    );
    true
}

pub(super) fn screen_critical_http_request(method: &str, path: &str, cache_mode: &str) -> bool {
    if cache_mode == "static" {
        return method == "GET";
    }
    let normalized = normalize_api_path(path);
    if matches!(normalized.as_str(), "/performance" | "/events") {
        return false;
    }
    if method != "GET" {
        return true;
    }
    matches!(
        normalized.as_str(),
        "/project/status"
            | "/apps/status"
            | "/dashboard"
            | "/work/goals"
            | "/work/features"
            | "/activity"
            | "/changes"
            | "/nodes"
            | "/settings"
            | "/processes"
            | "/diagnostics"
            | "/agents"
            | "/governance"
            | "/guidance"
            | "/reporters"
            | "/todos"
            | "/fleet"
            | "/quality"
            | "/upgrade"
            | "/target-app/status"
    ) || normalized.starts_with("/work/goals/")
        || normalized.starts_with("/work/features/")
}

pub(super) fn is_within(root: &Path, path: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    path.starts_with(root)
}

pub(super) fn website_index_path(static_root: &Path) -> PathBuf {
    static_root.join("src/surfaces/website/index.html")
}

pub(super) fn website_public_path_allowed(path: &str) -> bool {
    path == "README.md"
        || path == "LICENSE"
        || path.starts_with("docs/")
        || path == "src/surfaces/website/index.html"
        || path.starts_with("src/surfaces/website/assets/")
        || path.starts_with("src/surfaces/web/static/images/")
}

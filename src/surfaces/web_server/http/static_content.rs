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
    let method = request.method.clone();
    let raw_path = request.path.clone();
    let normalized_path = normalize_api_path(&raw_path);
    let status = response.status;
    let bytes = response.body.len() as u64;
    // One long-lived writer drains a bounded queue. A thread per request piled
    // up stacks behind the global metrics-log mutex under concurrent load; a
    // full queue drops the sample instead — metrics are best-effort.
    let _ = metrics_writer().try_send(MetricsSample {
        runtime_root,
        elapsed_ms,
        success: status < 400,
        details: json!({
            "method": method,
            "path": normalized_path,
            "raw_path": raw_path,
            "status": status,
            "bytes": bytes,
            "cache_mode": cache_mode,
            "budget_ms": 50.0,
            "over_budget": elapsed_ms > 50.0
        }),
    });
    true
}

struct MetricsSample {
    runtime_root: PathBuf,
    elapsed_ms: f64,
    success: bool,
    details: serde_json::Value,
}

fn metrics_writer() -> &'static std::sync::mpsc::SyncSender<MetricsSample> {
    static WRITER: std::sync::OnceLock<std::sync::mpsc::SyncSender<MetricsSample>> =
        std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<MetricsSample>(1024);
        thread::spawn(move || {
            while let Ok(sample) = receiver.recv() {
                let _ = FileMetricsService::new(sample.runtime_root).record_operation(
                    "http.request",
                    sample.elapsed_ms,
                    sample.success,
                    sample.details,
                );
            }
        });
        sender
    })
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

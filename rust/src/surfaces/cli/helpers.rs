use std::path::PathBuf;

use crate::core::supervisor::errors::RefineResult;

pub(super) fn default_static_root() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("src/surfaces/web/static"),
        PathBuf::from("rust/src/surfaces/web/static"),
    ];
    candidates.into_iter().find(|path| path.is_dir())
}

pub(super) fn absolutize_cli_path(path: &str) -> RefineResult<PathBuf> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err(crate::core::supervisor::errors::RefineError::InvalidInput(
            "path is required".to_string(),
        ));
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| {
                crate::core::supervisor::errors::RefineError::Io(format!(
                    "failed to inspect cwd: {error}"
                ))
            })?
            .join(path))
    }
}

pub(super) fn cli_timestamp() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

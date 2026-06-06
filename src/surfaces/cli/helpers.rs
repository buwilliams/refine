use std::path::PathBuf;

pub(super) fn default_static_root() -> Option<PathBuf> {
    let candidates = [PathBuf::from("src/surfaces/web/static")];
    candidates.into_iter().find(|path| path.is_dir())
}

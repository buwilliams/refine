use std::path::PathBuf;

pub(super) fn default_static_root() -> Option<PathBuf> {
    let relative = PathBuf::from("src/surfaces/web/static");
    if relative.is_dir() {
        return Some(relative);
    }
    let exe = std::env::current_exe().ok()?;
    for ancestor in exe.ancestors().skip(1) {
        let candidate = ancestor.join(&relative);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

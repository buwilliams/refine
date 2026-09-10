//! Bridge retained workflow fixtures to the new transport while preserving their
//! actual checkout mutations, commands, and findings. Event tests use native fixtures.
use std::path::{Path, PathBuf};

pub fn adapt_fixture(path: &Path) -> PathBuf {
    let wrapper = std::env::temp_dir().join(format!(
        "refine-event-fixture-{}-{}.py",
        std::process::id(),
        super::execution::stable_id(&path.display().to_string())
    ));
    let script = format!(
        "#!/usr/bin/env python3\nORIGINAL = {}\n{}",
        serde_json::to_string(&path.display().to_string()).unwrap(),
        include_str!("test_provider.py")
    );
    std::fs::write(&wrapper, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    wrapper
}

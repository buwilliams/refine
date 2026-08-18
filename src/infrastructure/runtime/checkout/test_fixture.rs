use std::fs;
use std::path::PathBuf;

use super::INSTALL_RUNBOOK;

/// One canonical source-product home for executable-ownership tests.
pub(crate) struct SyntheticSourceProduct {
    root: PathBuf,
}

impl SyntheticSourceProduct {
    pub(crate) fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "refine-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("docs/runbooks")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join("run")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname='refine'\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join(INSTALL_RUNBOOK), "# Install\n").unwrap();
        fs::write(root.join("r"), "#!/bin/sh\n").unwrap();
        fs::write(
            root.join("target/debug/refine"),
            "synthetic debug executable\n",
        )
        .unwrap();
        Self {
            root: root.canonicalize().unwrap(),
        }
    }

    pub(crate) fn runtime_root(&self) -> PathBuf {
        self.root.join("run")
    }

    pub(crate) fn port_runtime_root(&self, port: u16) -> PathBuf {
        self.runtime_root().join(port.to_string())
    }

    pub(crate) fn debug_executable(&self) -> PathBuf {
        self.root.join("target/debug/refine")
    }

    pub(crate) fn installed_executable(&self) -> PathBuf {
        self.root.join("bin/refine")
    }
}

impl Drop for SyntheticSourceProduct {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

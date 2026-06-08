use std::path::PathBuf;
use std::process::Command;

fn main() {
    let repo_root = repo_root();
    let status = Command::new("cargo")
        .args([
            "run",
            "--manifest-path",
            "xtask/Cargo.toml",
            "--",
            "test-all",
        ])
        .current_dir(&repo_root)
        .status()
        .expect("failed to start xtask test-all");

    if !status.success() {
        eprintln!("xtask test-all failed with status {status}");
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn repo_root() -> PathBuf {
    let mut current = std::env::current_dir().expect("failed to inspect cwd");
    loop {
        if current.join("docs/spec/rust-spec.md").is_file() {
            return current;
        }
        assert!(current.pop(), "failed to locate repository root");
    }
}

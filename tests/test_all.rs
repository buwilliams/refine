use std::path::PathBuf;
use std::process::Command;

#[test]
fn cargo_test_runs_full_suite() {
    let repo_root = repo_root();
    let output = Command::new("cargo")
        .args([
            "run",
            "--manifest-path",
            "xtask/Cargo.toml",
            "--",
            "test-all",
        ])
        .current_dir(&repo_root)
        .output()
        .expect("failed to start xtask test-all");

    assert!(
        output.status.success(),
        "xtask test-all failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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

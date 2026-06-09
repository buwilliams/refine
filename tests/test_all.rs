use std::path::PathBuf;
use std::process::Command;

fn main() {
    let Some(command) = selected_xtask_command() else {
        print_usage();
        return;
    };
    let repo_root = repo_root();
    let status = Command::new("cargo")
        .args(["run", "--manifest-path", "xtask/Cargo.toml", "--", command])
        .current_dir(&repo_root)
        .status()
        .unwrap_or_else(|error| panic!("failed to start xtask {command}: {error}"));

    if !status.success() {
        eprintln!("xtask {command} failed with status {status}");
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn selected_xtask_command() -> Option<&'static str> {
    let mut selected = "test-unit";
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--unit" => selected = "test-unit",
            "--integration" => selected = "test-integration",
            "--full" => selected = "test-all",
            "--help" | "-h" => return None,
            _ => {
                eprintln!("unsupported cargo test suite flag: {arg}\n");
                print_usage();
                std::process::exit(2);
            }
        }
    }
    Some(selected)
}

fn print_usage() {
    eprintln!(
        "Usage: cargo test [-- SUITE]\n\
\n\
Suites:\n\
  --unit         Run in-crate Rust unit tests only. This is the default.\n\
  --integration  Run opt-in integration, daemon, Docker, cluster, and UI suites.\n\
  --full         Run all test suites and repository checks.\n\
\n\
Examples:\n\
  cargo test\n\
  cargo test -- --integration\n\
  cargo test -- --full"
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

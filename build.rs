use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=REFINE_BUILD_SOURCE_COMMIT");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=Cargo.lock");

    register_git_inputs();

    let observed_commit = git(&["rev-parse", "HEAD"]);
    let requested_commit = env_value("REFINE_BUILD_SOURCE_COMMIT");
    let source_exact = git_checkout_is_clean()
        && requested_commit
            .as_deref()
            .is_none_or(|commit| Some(commit) == observed_commit.as_deref());
    let source_commit = observed_commit;
    if let Some(commit) = source_commit.as_deref() {
        println!("cargo:rustc-env=REFINE_BUILD_SOURCE_COMMIT={commit}");
    }
    println!(
        "cargo:rustc-env=REFINE_BUILD_SOURCE_EXACT={}",
        if source_exact { "true" } else { "false" }
    );

    let version = env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let version_tag = version.clone();
    let prefixed_tag = format!("v{version}");
    let release_tag = (|| {
        let commit = source_commit.as_deref()?;
        if !source_exact {
            return None;
        }
        [&version_tag, &prefixed_tag]
            .into_iter()
            .find(|tag| tag_commit(tag).as_deref() == Some(commit))
            .cloned()
    })();
    if release_tag
        .as_deref()
        .is_some_and(|tag| tag == version_tag || tag == prefixed_tag)
    {
        println!(
            "cargo:rustc-env=REFINE_BUILD_RELEASE_TAG={}",
            release_tag.unwrap()
        );
    }
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_checkout_is_clean() -> bool {
    Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=normal"])
        .output()
        .is_ok_and(|output| output.status.success() && output.stdout.is_empty())
}

fn tag_commit(tag: &str) -> Option<String> {
    git(&[
        "rev-parse",
        "--verify",
        &format!("refs/tags/{tag}^{{commit}}"),
    ])
}

fn register_git_inputs() {
    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from) else {
        return;
    };
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={}", PathBuf::from(path).display());
    }
    println!("cargo:rerun-if-changed={}", git_dir.join("index").display());
}

use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

fn email_fixture() -> (PathBuf, PathBuf, PathBuf) {
    let root =
        std::env::temp_dir().join(format!("refine email dispatch's {}", uuid::Uuid::new_v4()));
    let runtime = root.join("runtime's state/run/8082");
    let target = root.join("selected project's checkout");
    fs::create_dir_all(&runtime).unwrap();
    init_target(&target);
    fs::write(
        runtime.join("self-development-email.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "target_root": target,
            "address": "goal@example.com",
            "allowed_senders": ["sender@example.com"]
        }))
        .unwrap(),
    )
    .unwrap();
    (root, runtime, target)
}

fn init_target(target: &Path) {
    fs::create_dir_all(target).unwrap();
    let output = Command::new("git")
        .args(["init", "--quiet"])
        .arg(target)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

fn fetch_email(runtime: &Path, target: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_refine"))
        .args(["system", "fetch-email-goals", "--runtime-root"])
        .arg(runtime)
        .arg("--target-root")
        .arg(target)
        .current_dir(target)
        .output()
        .expect("failed to run refine binary")
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let contents = if path.is_dir() {
                visit(root, &path, entries);
                None
            } else {
                Some(fs::read(&path).unwrap())
            };
            entries.insert(path.strip_prefix(root).unwrap().to_path_buf(), contents);
        }
    }
    let mut entries = BTreeMap::new();
    visit(root, root, &mut entries);
    entries
}

#[test]
fn production_email_fetch_reaches_capability_with_the_configured_target() {
    let (root, runtime, target) = email_fixture();
    let output = fetch_email(&runtime, &target);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("secret email/fastmail_jmap_token was not found"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(!runtime.join("secrets").exists());
    assert!(!runtime.join("self-development-email").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_email_fetch_rejects_mismatched_target_before_secrets_or_ledger() {
    let (root, runtime, _target) = email_fixture();
    let other = root.join("other project's checkout");
    init_target(&other);
    let secrets = runtime.join("secrets");
    fs::create_dir_all(&secrets).unwrap();
    fs::write(secrets.join("secret-index.json"), b"malformed secret index").unwrap();
    let retained = runtime.join("self-development-email/requests/retained");
    fs::create_dir_all(&retained).unwrap();
    fs::write(retained.join("request.json"), b"retained ledger sentinel").unwrap();
    fs::write(retained.join("source.eml"), b"retained raw email sentinel").unwrap();
    let before = snapshot(&root);

    let output = fetch_email(&runtime, &other);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Email connection belongs to a different target app"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty(), "{output:?}");
    // Includes secret and ledger bytes, both Git targets, and any new lock directories.
    assert_eq!(snapshot(&root), before);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_cli_rejects_target_root_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_refine"))
        .args(["goal", "create", "--target-root", ".refine", "direct write"])
        .output()
        .expect("failed to run refine binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument '--target-root'"));
}

#[test]
fn production_cli_product_commands_require_daemon() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("failed to reserve test port");
    let port = listener
        .local_addr()
        .expect("failed to read test port")
        .port()
        .to_string();
    drop(listener);
    let output = Command::new(env!("CARGO_BIN_EXE_refine"))
        .args(["goal", "list"])
        .env("REFINE_DAEMON_PORT", port)
        .output()
        .expect("failed to run refine binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Refine daemon is required"), "{stderr}");
    assert!(stderr.contains("refine system start"), "{stderr}");
}

use std::net::TcpListener;
use std::process::Command;

#[test]
fn production_cli_rejects_target_root_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_refine"))
        .args(["gap", "create", "--target-root", ".refine", "direct write"])
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
        .args(["gap", "list"])
        .env("REFINE_DAEMON_PORT", port)
        .output()
        .expect("failed to run refine binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Refine daemon is required"), "{stderr}");
    assert!(stderr.contains("refine system start"), "{stderr}");
}

use std::process::Command;

#[test]
fn production_cli_rejects_durable_root_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_refine-native"))
        .args(["gap", "create", "--durable-root", ".refine", "direct write"])
        .output()
        .expect("failed to run refine-native binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unexpected argument '--durable-root'"));
}

use super::*;

#[test]
fn log_bundle_exports_redacted_support_bundle() {
    let temp_root = unique_temp_dir("cli-log-bundle");
    let target_root = temp_root.clone();
    let refine_dir = target_root.join(".refine");
    let runtime_root = temp_root.join("run");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(
        refine_dir.join("nodes.json"),
        r#"{"nodes":[{"id":"default","display_name":"Default","created_at":"2026-06-16T00:00:00Z","updated_at":"2026-06-16T00:00:00Z","settings":{"provider_token":"secret-value","visible":"ok"}}]}"#,
    )
    .unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "log",
            "bundle",
            "--target-root",
            target_root.to_str().unwrap(),
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--repo-root",
            temp_root.to_str().unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();

    let bundle_dir = refine_dir.join("support-bundles");
    let bundle_path = fs::read_dir(&bundle_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let body = fs::read_to_string(bundle_path).unwrap();
    assert!(body.contains("[redacted]"));
    assert!(!body.contains("secret-value"));

    fs::remove_dir_all(temp_root).unwrap();
}

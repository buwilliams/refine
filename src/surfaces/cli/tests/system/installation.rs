use super::*;

#[test]
fn system_install_repair_and_uninstall_use_installation_service() {
    let temp_root = unique_temp_dir("cli-installation");
    let runtime_root = temp_root.join("run");
    write_installed_binary(&temp_root);

    for argv in [
        ["refine", "system", "install"],
        ["refine", "system", "repair"],
        ["refine", "system", "rollback"],
        ["refine", "system", "uninstall"],
    ] {
        assert!(Cli::try_parse_from(argv).is_err());
    }

    for argv in [
        vec![
            "refine",
            "system",
            "install",
            "--port",
            "4557",
            "--target",
            "linux-cli-web",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.0.0",
        ],
        vec![
            "refine",
            "system",
            "repair",
            "--port",
            "4557",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.0.0",
        ],
        vec![
            "refine",
            "system",
            "uninstall",
            "--port",
            "4557",
            "--runtime-root",
            runtime_root.to_str().unwrap(),
            "--version",
            "1.0.0",
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let state: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(runtime_root.join("4557").join("install-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["status"]["installed"], false);
    assert_eq!(state["status"]["port"], 4557);
    assert_eq!(state["status"]["version"], "1.0.0");

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn system_update_no_longer_accepts_metadata_version_argument() {
    let err = Cli::try_parse_from([
        "refine",
        "system",
        "update",
        "1.1.0",
        "--runtime-root",
        "run",
    ])
    .unwrap_err();

    assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);

    Cli::try_parse_from(["refine", "system", "update", "--runtime-root", "run"]).unwrap();
}

#[test]
fn system_update_accepts_rescue_and_deprecated_confirmation_flags() {
    let parsed = Cli::try_parse_from([
        "refine",
        "system",
        "update",
        "--no-rescue",
        "--provider",
        "claude",
        "--port",
        "9090",
    ])
    .unwrap();
    let Commands::System {
        action:
            SystemAction::Update {
                yes,
                provider,
                no_rescue,
                port,
                ..
            },
    } = parsed.command
    else {
        panic!("expected a system update command");
    };
    assert!(!yes);
    assert!(no_rescue);
    assert_eq!(provider.as_deref(), Some("claude"));
    assert_eq!(port, 9090);

    // `--yes` stays accepted (hidden) so existing scripts keep working.
    Cli::try_parse_from(["refine", "system", "update", "--yes"]).unwrap();
}

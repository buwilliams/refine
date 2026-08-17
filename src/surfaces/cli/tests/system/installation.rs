use super::*;

#[test]
fn system_service_install_repair_and_service_uninstall_use_installation_service() {
    let temp_root = unique_temp_dir("cli-installation");
    let runtime_root = temp_root.join("run");
    write_installed_binary(&temp_root);

    for argv in [
        ["refine", "system", "service-install"],
        ["refine", "system", "repair"],
        ["refine", "system", "rollback"],
        ["refine", "system", "service-uninstall"],
    ] {
        assert!(Cli::try_parse_from(argv).is_err());
    }

    for argv in [
        vec![
            "refine",
            "system",
            "service-install",
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
            "service-uninstall",
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
fn retired_system_install_and_uninstall_commands_are_not_parseable() {
    for retired in ["install", "uninstall"] {
        assert!(
            Cli::try_parse_from(["refine", "system", retired, "--port", "4557"]).is_err(),
            "retired system {retired} command unexpectedly parsed"
        );
    }
}

#[test]
fn system_update_is_owned_by_the_launcher_not_the_deployed_binary() {
    assert!(Cli::try_parse_from(["refine", "system", "update"]).is_err());
}

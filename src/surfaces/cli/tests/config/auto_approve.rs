use super::*;

#[test]
fn config_auto_approve_show_set_and_invalid_input_use_shared_settings() {
    let root = unique_temp_dir("cli-auto-approve");
    let target = root.join("app");
    fs::create_dir_all(&target).unwrap();
    let run = |args: &[&str]| {
        let mut command = vec!["refine", "config", "settings"];
        command.extend_from_slice(args);
        command.extend_from_slice(&["--target-root", target.to_str().unwrap()]);
        dispatch_config(Cli::try_parse_from(command).unwrap().command.into_config())
    };
    assert_eq!(run(&["show"]).unwrap()["settings"]["auto_approve"], "false");
    for value in ["true", "false"] {
        let assignment = format!("auto_approve={value}");
        assert_eq!(
            run(&["set", "--set", &assignment]).unwrap()["settings"]["auto_approve"],
            value
        );
        assert_eq!(run(&["show"]).unwrap()["settings"]["auto_approve"], value);
    }
    let before = run(&["show"]).unwrap();
    assert!(matches!(
        run(&["set", "--set", "auto_approve=yes"]),
        Err(RefineError::InvalidInput(_))
    ));
    assert_eq!(run(&["show"]).unwrap(), before);
    fs::remove_dir_all(root).unwrap();
}

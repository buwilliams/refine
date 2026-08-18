use super::*;

#[test]
fn file_process_supervisor_enforces_allowed_commands() {
    let temp_root = unique_temp_dir("process-allowed");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::with_allowed_commands(&runtime_root, ["printf"]);

    let denied = supervisor.launch(ManagedProcessSpec {
        owner: ProcessOwner::UserHelper,
        command: shell_binary().to_string(),
        args: shell_args("rm -rf target").to_vec(),
        cwd: None,
        env: Vec::new(),
        stdin: None,
        limits: None,
        authorization_command: Some("rm -rf target".to_string()),
        sensitive: false,
        metadata: Default::default(),
    });

    assert!(matches!(denied, Err(RefineError::Unauthorized(_))));
    let audit = fs::read_to_string(runtime_root.join("security-audit.jsonl")).unwrap();
    assert!(audit.contains("\"outcome\":\"denied\""));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_redacts_sensitive_process_details_and_stdin() {
    let temp_root = unique_temp_dir("process-sensitive");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);

    let process = supervisor
        .run_to_completion(ManagedProcessSpec {
            owner: ProcessOwner::Maintenance,
            command: shell_binary().to_string(),
            args: shell_args("cat >/dev/null").to_vec(),
            cwd: None,
            env: Vec::new(),
            stdin: Some("secret-value".to_string()),
            limits: None,
            authorization_command: None,
            sensitive: true,
            metadata: Default::default(),
        })
        .unwrap()
        .process;

    assert_eq!(process.details.as_deref(), Some("redacted"));
    assert!(process.stdin_path.is_none());
    assert!(
        !supervisor
            .processes_dir()
            .join(format!("{}.json", process.id))
            .exists()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_redacts_managed_prompt_argv_and_stdin_only() {
    let temp_root = unique_temp_dir("process-prompt-sensitive");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let secret = "PROMPT_CONTENT_MUST_NOT_PERSIST";
    let process = supervisor
        .run_to_completion(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: shell_binary().to_string(),
            args: shell_args("cat >/dev/null")
                .into_iter()
                .chain([secret.to_string()])
                .collect(),
            cwd: None,
            env: Vec::new(),
            stdin: Some(secret.to_string()),
            limits: None,
            authorization_command: Some(
                "sh [refine-managed-prompt kind=stdin bytes=31]".to_string(),
            ),
            sensitive: false,
            metadata: Map::from_iter([(
                "prompt_transport".to_string(),
                json!({
                    "kind": "stdin",
                    "utf8_bytes": 31,
                    "sha256": "safe-digest",
                    "owner": "safe-owner",
                    "lifecycle": "owned"
                }),
            )]),
        })
        .unwrap()
        .process;

    let details = process.details.unwrap();
    assert!(!details.contains(secret));
    assert!(details.contains("safe-digest"));
    assert!(details.contains("refine-managed-prompt"));
    assert!(process.stdin_path.is_none());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_process_supervisor_strips_direct_api_keys_from_agent_processes() {
    let temp_root = unique_temp_dir("process-agent-env");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);

    let output = supervisor
        .run_to_completion(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: shell_binary().to_string(),
            args: shell_args(
                "printf '%s:%s' \"${OPENAI_API_KEY-unset}\" \"${ANTHROPIC_API_KEY-unset}\"",
            )
            .to_vec(),
            cwd: None,
            env: vec![
                ("OPENAI_API_KEY".to_string(), "bad-openai-key".to_string()),
                (
                    "ANTHROPIC_API_KEY".to_string(),
                    "bad-anthropic-key".to_string(),
                ),
            ],
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();

    assert_eq!(output.stdout, "unset:unset");
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn managed_child_observes_the_exact_preflighted_effective_environment() {
    use std::collections::BTreeMap;

    let temp_root = unique_temp_dir("process-effective-env-parity");
    let runtime_root = temp_root.join("run/8080");
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let overrides = vec![
        ("REFINE_ENV_PARITY".to_string(), "first-value".to_string()),
        ("REFINE_ENV_PARITY".to_string(), "final-🙂é".to_string()),
        ("REFINE_SESSION_ROLE".to_string(), "parity-test".to_string()),
        (
            "OPENAI_API_KEY".to_string(),
            "must-not-be-observed".to_string(),
        ),
    ];
    let expected =
        crate::infrastructure::process::launch_environment::EffectiveLaunchEnvironment::assemble(
            &ProcessOwner::Agent,
            &overrides,
        )
        .unwrap();
    let output = supervisor
        .run_to_completion(ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: "/usr/bin/env".to_string(),
            args: vec!["-0".to_string()],
            cwd: None,
            env: overrides,
            stdin: None,
            limits: None,
            authorization_command: Some("env -0".to_string()),
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();
    let observed = output
        .stdout
        .split('\0')
        .filter_map(|entry| entry.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<BTreeMap<_, _>>();
    let expected = expected
        .entries()
        .iter()
        .map(|(key, value)| {
            (
                key.to_string_lossy().to_string(),
                value.to_string_lossy().to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    assert_eq!(observed, expected);
    assert_eq!(observed["REFINE_ENV_PARITY"], "final-🙂é");
    assert_eq!(observed["REFINE_SESSION_ROLE"], "parity-test");
    assert!(!observed.contains_key("OPENAI_API_KEY"));
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn supervised_quality_command_discovers_only_allowlisted_shell_tools() {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir("quality-shell-tool-discovery");
    let runtime_root = temp_root.join("run/8080");
    let tool_root = temp_root.join("shell-tools");
    fs::create_dir_all(&tool_root).unwrap();
    let tool = tool_root.join("refine-quality-tool");
    fs::write(&tool, "#!/bin/sh\nprintf 'tool-found:%s:%s:%s' \"${DOTNET_ROOT-unset}\" \"${OPENAI_API_KEY-unset}\" \"${DATABASE_URL-unset}\"\n").unwrap();
    let mut permissions = fs::metadata(&tool).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&tool, permissions).unwrap();

    let shell = BTreeMap::from([
        ("PATH".to_string(), tool_root.to_string_lossy().to_string()),
        ("DOTNET_ROOT".to_string(), "/shell/dotnet".to_string()),
        (
            "OPENAI_API_KEY".to_string(),
            "shell-provider-key".to_string(),
        ),
        (
            "DATABASE_URL".to_string(),
            "shell-database-secret".to_string(),
        ),
    ]);
    let environment = crate::infrastructure::process::launch_environment::EffectiveLaunchEnvironment::assemble_for_test(
        &ProcessOwner::Quality,
        &[],
        &shell,
        &[],
    )
    .unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let output = supervisor
        .run_to_completion_with_prepared_environment(
            ManagedProcessSpec {
                owner: ProcessOwner::Quality,
                command: "/bin/sh".to_string(),
                args: shell_args("refine-quality-tool"),
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: None,
                authorization_command: Some("refine-quality-tool".to_string()),
                sensitive: false,
                metadata: Default::default(),
            },
            &environment,
            |_, _| {},
        )
        .unwrap();

    assert_eq!(output.process.exit_code, Some(0));
    assert_eq!(output.stdout, "tool-found:/shell/dotnet:unset:unset");
    fs::remove_dir_all(temp_root).unwrap();
}

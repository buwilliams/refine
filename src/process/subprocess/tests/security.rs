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

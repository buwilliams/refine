use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn browser_terminal_redacts_echoed_credentials_in_events_and_transcript() {
    let root = std::env::temp_dir().join(format!("refine-terminal-credentials-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let binary = root.join("agent");
    fs::write(&binary, "#!/usr/bin/python3\nimport os,time\nfor c in os.environ['OPENAI_API_KEY']:\n print(c,end='',flush=True);time.sleep(.001)\nprint(' finished')\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let source = format!("TERMINAL_TEST_{}", Uuid::new_v4().simple());
    let secret = "terminal-credential-must-stay-local";
    unsafe {
        std::env::set_var(&source, secret);
    }
    let state = root.join("state");
    let runtime = root.join("run/8082");
    let mut catalog = crate::model::providers::defaults();
    let mut provider = crate::model::providers::ProviderDefinition::generic("terminal-credentials");
    provider.executable = binary.display().to_string();
    provider
        .credentials
        .insert("OPENAI_API_KEY".into(), source.clone());
    catalog.providers.push(provider);
    crate::application::agents::providers::save(&state, &json!(catalog)).unwrap();
    let service =
        crate::infrastructure::agents::invocation::HostAgentProviderService::with_runtime_root(
            &runtime,
        )
        .with_refine_dir(&state);
    let command = service
        .interactive_command("terminal-credentials", "context")
        .unwrap();
    let response = terminal_session_start_response(
        TerminalLaunchSpec {
            runtime_root: runtime.clone(),
            cwd: root.clone(),
            profile: "agent".into(),
            provider: Some("terminal-credentials".into()),
            command: command.binary,
            args: command.args,
            metadata: Map::new(),
            prompt_transport: Some(command.prompt_transport),
            prompt_artifact: command.prompt_artifact,
            authorization_command: Some(command.authorization_command),
            launch_environment: Some(command.launch_environment),
        },
        80,
        24,
    )
    .unwrap();
    let id = response["id"].as_str().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !terminal_status_response(&runtime, id).unwrap()["exited"]
        .as_bool()
        .unwrap()
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let events = terminal_events_since(&runtime, id, 0).unwrap();
    let output = events
        .iter()
        .filter_map(|e| e["data"].as_str())
        .collect::<String>();
    assert!(output.contains("[REDACTED] finished"), "{output}");
    assert!(!serde_json::to_string(&events).unwrap().contains(secret));
    let session = sessions().lock().unwrap().remove(id).unwrap();
    assert!(
        !fs::read_to_string(&session.stdout_path)
            .unwrap()
            .contains(secret)
    );
    unsafe {
        std::env::remove_var(&source);
    }
    fs::remove_dir_all(root).unwrap();
}

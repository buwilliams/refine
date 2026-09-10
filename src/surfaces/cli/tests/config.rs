use super::*;
use crate::error::RefineError;
use std::sync::{Mutex, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[test]
fn events_skills_and_runtime_command_trees_parse_and_retired_editors_are_rejected() {
    for args in [
        vec!["refine", "config", "show"],
        vec!["refine", "config", "show", "skills"],
        vec![
            "refine",
            "config",
            "settings",
            "set",
            "--set",
            "agent_cli=codex",
        ],
        vec!["refine", "skills", "triggers"],
        vec!["refine", "skills", "list", "--node-id", "default"],
        vec![
            "refine",
            "skills",
            "trigger",
            "deploy",
            "--param",
            "version=1",
            "--request-id",
            "request-1",
        ],
        vec![
            "refine",
            "skills",
            "clone",
            "check",
            "copy",
            "--trigger",
            "custom",
        ],
        vec!["refine", "skills", "runs"],
        vec!["refine", "skills", "cancel", "invocation-1"],
        vec![
            "refine",
            "skills",
            "save",
            "check",
            "--revision",
            "4",
            "--file",
            "skill.json",
        ],
        vec!["refine", "skills", "enable", "check"],
        vec!["refine", "skills", "remove", "check", "--revision", "5"],
    ] {
        Cli::try_parse_from(&args).unwrap_or_else(|error| panic!("failed {args:?}: {error}"));
    }
    assert!(Cli::try_parse_from(["refine", "events", "list"]).is_err());
    assert!(Cli::try_parse_from(["refine", "config", "show", "events"]).is_err());
    assert!(
        Cli::try_parse_from(["refine", "skills", "trigger", "check", "--goal-id", "GOAL1"])
            .is_err()
    );
    for retired in ["quality", "governance", "guidance"] {
        assert!(Cli::try_parse_from(["refine", "config", retired, "show"]).is_err());
    }
}

#[test]
fn config_help_documents_scope_boundary_and_catalogs_every_family() {
    use clap::CommandFactory;
    let mut command = Cli::command();
    let config = command.find_subcommand_mut("config").unwrap();
    let help = config.render_long_help().to_string();
    for word in [
        "Project",
        "workflow",
        "nodes",
        "fleet",
        "Reporters",
        "Todos",
        "agent",
    ] {
        assert!(help.contains(word), "config help missing {word}: {help}");
    }

    let catalog = crate::surfaces::cli::catalog::commands_catalog();
    let config = catalog["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["name"] == "config")
        .unwrap();
    let names = config["subcommands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["show", "settings"]);
}

#[test]
fn config_target_root_adapter_uses_shared_services_and_returns_saved_readback() {
    let root = unique_temp_dir("cli-config-target-root");
    std::fs::create_dir_all(&root).unwrap();
    let saved = dispatch_config(
        Cli::try_parse_from([
            "refine",
            "config",
            "settings",
            "set",
            "--set",
            "agent_cli=codex",
            "--target-root",
            root.to_str().unwrap(),
        ])
        .unwrap()
        .command
        .into_config(),
    )
    .unwrap();
    assert_eq!(saved["settings"]["agent_cli"], "codex");
    let all = dispatch_config(
        Cli::try_parse_from([
            "refine",
            "config",
            "show",
            "--target-root",
            root.to_str().unwrap(),
        ])
        .unwrap()
        .command
        .into_config(),
    )
    .unwrap();
    assert_eq!(all["skills"]["items"].as_array().unwrap().len(), 4);
    assert!(all.get("events").is_none());
    assert!(all.get("governance").is_none());
    fs::remove_dir_all(root).unwrap();
}

trait IntoConfigAction {
    fn into_config(self) -> ConfigAction;
}

impl IntoConfigAction for Commands {
    fn into_config(self) -> ConfigAction {
        let Commands::Config { action } = self else {
            panic!("expected config command")
        };
        action
    }
}

#[test]
fn config_rejects_unknown_malformed_and_overlapping_input_before_state_changes() {
    let root = unique_temp_dir("cli-config-invalid");
    fs::create_dir_all(&root).unwrap();
    for payload in [
        "{",
        r#"{"unknown_setting":true}"#,
        r#"{"max_automatic_round_retries":-1}"#,
    ] {
        let action = Cli::try_parse_from([
            "refine",
            "config",
            "settings",
            "set",
            "--json",
            payload,
            "--target-root",
            root.to_str().unwrap(),
        ])
        .unwrap()
        .command
        .into_config();
        assert!(matches!(
            dispatch_config(action),
            Err(RefineError::InvalidInput(_))
        ));
    }
    let action = Cli::try_parse_from([
        "refine",
        "config",
        "settings",
        "set",
        "--json",
        "{}",
        "--set",
        "agent_cli=codex",
        "--target-root",
        root.to_str().unwrap(),
    ])
    .unwrap()
    .command
    .into_config();
    assert!(matches!(
        dispatch_config(action),
        Err(RefineError::InvalidInput(_))
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn config_daemon_routing_uses_contract_and_idempotency_headers_and_saved_readback() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for response in [
            json!({"settings": {"agent_cli": "claude"}}),
            json!({"ok": true, "settings": {"agent_cli": "codex", "unrelated": "kept"}}),
        ] {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                bytes.extend_from_slice(&buffer[..read]);
                if read == 0
                    || bytes
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .is_some_and(|split| {
                            let head = String::from_utf8_lossy(&bytes[..split]);
                            let length = head
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase()
                                        .strip_prefix("content-length: ")
                                        .and_then(|value| value.parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            bytes.len() >= split + 4 + length
                        })
                {
                    break;
                }
            }
            requests.push(String::from_utf8(bytes).unwrap());
            let body = serde_json::to_vec(&response).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        }
        requests
    });
    let previous = std::env::var_os("REFINE_DAEMON_PORT");
    unsafe { std::env::set_var("REFINE_DAEMON_PORT", port.to_string()) };

    let read = dispatch_config(
        Cli::try_parse_from(["refine", "config", "settings", "show"])
            .unwrap()
            .command
            .into_config(),
    )
    .unwrap();
    assert_eq!(read["settings"]["agent_cli"], "claude");
    let saved = dispatch_config(
        Cli::try_parse_from([
            "refine",
            "config",
            "settings",
            "set",
            "--set",
            "agent_cli=codex",
        ])
        .unwrap()
        .command
        .into_config(),
    )
    .unwrap();
    assert_eq!(saved["settings"]["agent_cli"], "codex");
    assert_eq!(saved["settings"]["unrelated"], "kept");

    match previous {
        Some(value) => unsafe { std::env::set_var("REFINE_DAEMON_PORT", value) },
        None => unsafe { std::env::remove_var("REFINE_DAEMON_PORT") },
    }
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with("GET /settings HTTP/1.1"));
    assert!(requests[1].starts_with("PATCH /settings HTTP/1.1"));
    for request in &requests {
        assert!(request.contains(&format!(
            "X-Refine-API-Version: {}",
            crate::surfaces::web_server::API_CONTRACT_VERSION
        )));
        assert!(request.contains("Idempotency-Key: cli-"));
    }
    assert!(requests[1].ends_with(r#"{"agent_cli":"codex"}"#));
}

#[test]
fn config_rejects_invalid_domain_input_before_daemon_transport() {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let body = br#"{}"#;
                    write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                    stream.write_all(body).unwrap();
                    return true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if stop_rx.try_recv().is_ok() {
                        return false;
                    }
                    thread::yield_now();
                }
                Err(error) => panic!("unexpected listener error: {error}"),
            }
        }
    });
    let previous = std::env::var_os("REFINE_DAEMON_PORT");
    unsafe { std::env::set_var("REFINE_DAEMON_PORT", port.to_string()) };

    let result = dispatch_config(
        Cli::try_parse_from([
            "refine",
            "config",
            "settings",
            "set",
            "--json",
            r#"{"unknown_setting":true}"#,
        ])
        .unwrap()
        .command
        .into_config(),
    );

    match previous {
        Some(value) => unsafe { std::env::set_var("REFINE_DAEMON_PORT", value) },
        None => unsafe { std::env::remove_var("REFINE_DAEMON_PORT") },
    }
    assert!(matches!(result, Err(RefineError::InvalidInput(_))));
    stop_tx.send(()).unwrap();
    assert!(!server.join().unwrap(), "invalid input reached the daemon");
}

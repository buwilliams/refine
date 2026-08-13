use super::*;
use crate::process::supervisor::errors::RefineError;
use std::sync::{Mutex, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[test]
fn config_complete_command_tree_and_payload_forms_parse() {
    for args in [
        vec!["refine", "config", "show"],
        vec!["refine", "config", "show", "quality"],
        vec!["refine", "config", "settings", "show"],
        vec![
            "refine",
            "config",
            "settings",
            "set",
            "--set",
            "agent_cli=codex",
        ],
        vec![
            "refine",
            "config",
            "settings",
            "set",
            "--json",
            r#"{"agent_cli":"codex"}"#,
        ],
        vec!["refine", "config", "quality", "show"],
        vec![
            "refine",
            "config",
            "quality",
            "set",
            "--test",
            "Dashboard loads",
        ],
        vec![
            "refine",
            "config",
            "quality",
            "set",
            "--file",
            "quality.json",
        ],
        vec!["refine", "config", "governance", "show"],
        vec![
            "refine",
            "config",
            "governance",
            "set",
            "--rule",
            "No regressions",
        ],
        vec!["refine", "config", "governance", "generate-rules"],
        vec!["refine", "config", "governance", "set", "--stdin"],
        vec!["refine", "config", "guidance", "list"],
        vec![
            "refine",
            "config",
            "guidance",
            "add",
            "--json",
            r#"{"name":"A","rule":"B","instructions":"C"}"#,
        ],
        vec![
            "refine",
            "config",
            "guidance",
            "edit",
            "guidance-1",
            "--name",
            "New",
        ],
        vec!["refine", "config", "guidance", "enable", "guidance-1"],
        vec!["refine", "config", "guidance", "disable", "guidance-1"],
        vec!["refine", "config", "guidance", "remove", "guidance-1"],
    ] {
        Cli::try_parse_from(&args).unwrap_or_else(|error| panic!("failed {args:?}: {error}"));
    }
    assert!(
        Cli::try_parse_from([
            "refine", "config", "quality", "set", "--json", "{}", "--file", "q.json"
        ])
        .is_ok(),
        "the shared decoder owns structured overlapping-source errors"
    );
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
    assert_eq!(
        names,
        ["show", "settings", "quality", "governance", "guidance"]
    );
}

#[test]
fn config_target_root_adapter_uses_shared_services_and_returns_saved_readback() {
    let temp_root = unique_temp_dir("cli-config-target-root");
    let target = temp_root.join("app");
    fs::create_dir_all(&target).unwrap();

    let saved = dispatch_config(
        Cli::try_parse_from([
            "refine",
            "config",
            "quality",
            "set",
            "--business-requirements",
            "Dashboard works",
            "--test",
            "Dashboard loads",
            "--target-root",
            target.to_str().unwrap(),
        ])
        .unwrap()
        .command
        .into_config(),
    )
    .unwrap();
    assert_eq!(saved["business_requirements"], "Dashboard works");
    assert_eq!(saved["tests"], json!(["Dashboard loads"]));

    let all = dispatch_config(
        Cli::try_parse_from([
            "refine",
            "config",
            "show",
            "--target-root",
            target.to_str().unwrap(),
        ])
        .unwrap()
        .command
        .into_config(),
    )
    .unwrap();
    assert_eq!(all["quality"]["business_requirements"], "Dashboard works");
    assert!(all["settings"].is_object());
    assert_eq!(all["governance"]["rules_revision"], 0);
    assert_eq!(all["guidance"]["revision"], 0);

    fs::remove_dir_all(temp_root).unwrap();
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
    let temp_root = unique_temp_dir("cli-config-invalid");
    let target = temp_root.join("app");
    fs::create_dir_all(&target).unwrap();
    let target = target.to_str().unwrap();

    let malformed = Cli::try_parse_from([
        "refine",
        "config",
        "quality",
        "set",
        "--json",
        "{",
        "--target-root",
        target,
    ])
    .unwrap()
    .command
    .into_config();
    assert!(matches!(
        dispatch_config(malformed),
        Err(RefineError::InvalidInput(_))
    ));

    let unknown = Cli::try_parse_from([
        "refine",
        "config",
        "settings",
        "set",
        "--set",
        "unknown_setting=true",
        "--target-root",
        target,
    ])
    .unwrap()
    .command
    .into_config();
    assert!(matches!(
        dispatch_config(unknown),
        Err(RefineError::InvalidInput(_))
    ));

    let overlap = Cli::try_parse_from([
        "refine",
        "config",
        "quality",
        "set",
        "--instructions",
        "One",
        "--json",
        "{}",
        "--target-root",
        target,
    ])
    .unwrap()
    .command
    .into_config();
    assert!(matches!(
        dispatch_config(overlap),
        Err(RefineError::InvalidInput(_))
    ));

    let invalid_quality = Cli::try_parse_from([
        "refine",
        "config",
        "quality",
        "set",
        "--json",
        r#"{"tests":"not-a-list"}"#,
        "--target-root",
        target,
    ])
    .unwrap()
    .command
    .into_config();
    assert!(matches!(
        dispatch_config(invalid_quality),
        Err(RefineError::InvalidInput(_))
    ));

    let invalid_governance = Cli::try_parse_from([
        "refine",
        "config",
        "governance",
        "set",
        "--json",
        r#"{"max_automatic_round_retries":-1}"#,
        "--target-root",
        target,
    ])
    .unwrap()
    .command
    .into_config();
    assert!(matches!(
        dispatch_config(invalid_governance),
        Err(RefineError::InvalidInput(_))
    ));

    let missing_guidance = Cli::try_parse_from([
        "refine",
        "config",
        "guidance",
        "remove",
        "missing",
        "--target-root",
        target,
    ])
    .unwrap()
    .command
    .into_config();
    assert!(matches!(
        dispatch_config(missing_guidance),
        Err(RefineError::NotFound(_))
    ));

    fs::remove_dir_all(temp_root).unwrap();
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

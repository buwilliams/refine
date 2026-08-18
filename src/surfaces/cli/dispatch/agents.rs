use super::*;

pub(super) fn dispatch_command(command: Commands) -> RefineResult<()> {
    match command {
        Commands::Agent {
            action:
                AgentAction::Open {
                    goal_id,
                    profile,
                    prompt,
                },
        } => {
            let profile = if goal_id.is_some() && profile == CliAgentProfile::Agent {
                CliAgentProfile::Goal
            } else {
                profile
            };
            let response = daemon_json(
                "POST",
                "/terminal/session",
                Some(json!({
                    "profile": profile.as_str(),
                    "surface": "cli",
                    "goal_id": goal_id,
                    "initial_prompt": prompt
                })),
            )?;
            print_json(&response);
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Detect,
        } => {
            let providers = HostAgentProviderService::new().detect()?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"providers": providers})).unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Configure { provider },
        } => {
            HostAgentProviderService::new().configure(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "configured": true})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Auth { provider },
        } => {
            HostAgentProviderService::new().authenticate(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "authenticated": true})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action: AgentAction::Diagnose { provider },
        } => {
            let diagnostics = HostAgentProviderService::new().diagnose(&provider)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"ok": true, "provider": provider, "diagnostics": diagnostics})
                )
                .unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action:
                AgentAction::Invoke {
                    prompt,
                    provider,
                    cwd,
                },
        } => {
            let output = HostAgentProviderService::new().invoke(ProviderInvocation {
                stall_timeout_seconds: None,
                provider,
                prompt,
                session_id: None,
                cwd: cwd.map(|path| path.display().to_string()),
                process_metadata: Default::default(),
            })?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"ok": true, "output": output})).unwrap()
            );
            Ok(())
        }
        Commands::Agent {
            action:
                AgentAction::Resume {
                    session_id,
                    provider,
                },
        } => {
            let output = HostAgentProviderService::new().resume(&provider, &session_id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({"ok": true, "output": output})).unwrap()
            );
            Ok(())
        }
        _ => unreachable!("command family was routed incorrectly"),
    }
}

#[cfg(not(test))]
pub(super) fn dispatch_agent_daemon(action: AgentAction) -> RefineResult<()> {
    let action = match action {
        AgentAction::Open {
            goal_id,
            profile,
            prompt,
        } => return attach_agent(profile, goal_id.as_deref(), prompt.as_deref()),
        action => action,
    };
    let response = match action {
        AgentAction::Open { .. } => unreachable!("handled before daemon dispatch"),
        AgentAction::Detect => daemon_json("GET", "/agents", None)?,
        AgentAction::Configure { provider } => daemon_json(
            "POST",
            &format!("/agents/{}/configure", path_segment(&provider)),
            None,
        )?,
        AgentAction::Auth { provider } => daemon_json(
            "POST",
            &format!("/agents/{}/auth", path_segment(&provider)),
            None,
        )?,
        AgentAction::Diagnose { provider } => daemon_json(
            "GET",
            &format!("/agents/{}/diagnostics", path_segment(&provider)),
            None,
        )?,
        AgentAction::Invoke {
            prompt,
            provider,
            cwd,
        } => daemon_json(
            "POST",
            &format!("/agents/{}/invoke", path_segment(&provider)),
            Some(json!({
                "prompt": prompt,
                "cwd": cwd.map(|path| path.display().to_string())
            })),
        )?,
        AgentAction::Resume {
            session_id,
            provider,
        } => daemon_json(
            "POST",
            &format!("/agents/{}/resume", path_segment(&provider)),
            Some(json!({
                "session_id": session_id
            })),
        )?,
    };
    print_json(&response);
    Ok(())
}

#[cfg(not(test))]
pub(super) fn attach_agent(
    profile: CliAgentProfile,
    goal_id: Option<&str>,
    prompt: Option<&str>,
) -> RefineResult<()> {
    if profile == CliAgentProfile::Goal && goal_id.is_none() {
        return Err(RefineError::InvalidInput(
            "opening a Goal Agent requires GOAL_ID".to_string(),
        ));
    }
    if profile != CliAgentProfile::Goal && goal_id.is_some() {
        return Err(RefineError::InvalidInput(format!(
            "GOAL_ID is only valid with --profile goal, not --profile {}",
            profile.as_str()
        )));
    }
    if profile != CliAgentProfile::Plan && prompt.is_some() {
        return Err(RefineError::InvalidInput(
            "--prompt is only valid with --profile plan".to_string(),
        ));
    }
    let session = daemon_json(
        "POST",
        "/terminal/session",
        Some(json!({
            "profile": profile.as_str(),
            "surface": "cli",
            "goal_id": goal_id,
            "initial_prompt": prompt
        })),
    )?;
    let session_id = session
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            RefineError::Serialization(
                "Goal Agent response did not include a session id".to_string(),
            )
        })?
        .to_string();
    let provider = session
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let cwd = session.get("cwd").and_then(Value::as_str).unwrap_or("");
    let label = goal_id
        .map(|goal_id| format!("Goal {goal_id} Agent"))
        .unwrap_or_else(|| format!("{} agent", profile.as_str()));
    eprintln!(
        "Attached to {label} via {provider}{}.\r\nPress Ctrl-] to detach without stopping the agent.",
        if cwd.is_empty() {
            String::new()
        } else {
            format!(" in {cwd}")
        }
    );
    let mut last_attention = (
        session
            .get("attention_state")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        session
            .get("attention_message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    );
    if last_attention.0 == "needs_input" && !last_attention.1.is_empty() {
        eprintln!("\r\n{label} needs input: {}", last_attention.1);
    }

    let _terminal_mode = CliTerminalMode::enter();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buffer = [0_u8; 1024];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if tx.send(buffer[..count].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut after = 0_u64;
    let mut last_size = None;
    let mut next_terminal_check = Instant::now();
    loop {
        while let Ok(bytes) = rx.try_recv() {
            let detach = bytes.iter().position(|byte| *byte == 0x1d);
            let input = detach.map(|index| &bytes[..index]).unwrap_or(&bytes);
            if !input.is_empty() {
                let data = String::from_utf8_lossy(input).to_string();
                daemon_json(
                    "POST",
                    &format!("/terminal/{}/input", path_segment(&session_id)),
                    Some(json!({"data": data})),
                )?;
            }
            if detach.is_some() {
                eprintln!("\r\nDetached. {label} is still running.");
                return Ok(());
            }
        }

        match daemon_json(
            "GET",
            &format!(
                "/terminal/{}/events?snapshot=1&after={after}",
                path_segment(&session_id)
            ),
            None,
        ) {
            Ok(snapshot) => {
                for event in snapshot
                    .get("events")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(seq) = event.get("seq").and_then(Value::as_u64) {
                        after = after.max(seq);
                    }
                    if let Some(data) = event.get("data").and_then(Value::as_str) {
                        print!("{data}");
                    }
                }
                std::io::stdout().flush().map_err(|error| {
                    RefineError::Io(format!("failed to flush terminal output: {error}"))
                })?;
            }
            Err(RefineError::NotFound(_)) => {
                eprintln!("\r\n{label} session ended.");
                return Ok(());
            }
            Err(error) => return Err(error),
        }
        if Instant::now() >= next_terminal_check {
            match daemon_json(
                "GET",
                &format!("/terminal/{}/status", path_segment(&session_id)),
                None,
            ) {
                Ok(status) => {
                    let attention = (
                        status
                            .get("attention_state")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        status
                            .get("attention_message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    );
                    if attention != last_attention {
                        last_attention = attention;
                        if last_attention.0 == "needs_input" {
                            eprintln!("\r\n{label} needs input: {}", last_attention.1);
                        }
                    }
                }
                Err(RefineError::NotFound(_)) => {
                    eprintln!("\r\n{label} session ended.");
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
            if let Some(size) = cli_terminal_size()
                && last_size != Some(size)
            {
                daemon_json(
                    "POST",
                    &format!("/terminal/{}/resize", path_segment(&session_id)),
                    Some(json!({"cols": size.0, "rows": size.1})),
                )?;
                last_size = Some(size);
            }
            next_terminal_check = Instant::now() + Duration::from_millis(500);
        }
        thread::sleep(Duration::from_millis(60));
    }
}

#[cfg(not(test))]
pub(super) fn cli_terminal_size() -> Option<(u16, u16)> {
    let output = Command::new("stty").arg("size").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    let rows = parts.next()?.parse::<u16>().ok()?;
    let cols = parts.next()?.parse::<u16>().ok()?;
    Some((cols, rows))
}

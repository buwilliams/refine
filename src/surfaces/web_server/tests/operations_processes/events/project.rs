use super::*;

#[test]
fn web_server_serves_project_utility_upgrade_health_and_sse_routes() {
    let temp_root = unique_temp_dir("http-project-utils");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(temp_root.join("child")).unwrap();
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let path = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!(
            "/api/project/path?path={}",
            percent_encode_for_test(temp_root.to_str().unwrap())
        ),
        body: None,
    });
    assert_eq!(path.status, 200);
    assert_eq!(path.body["exists"], true);
    assert_eq!(path.body["is_dir"], true);

    let directories = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!(
            "/api/project/directories?path={}&max_entries=10",
            percent_encode_for_test(temp_root.to_str().unwrap())
        ),
        body: None,
    });
    assert_eq!(directories.status, 200);
    assert!(
        directories.body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "child")
    );

    let upgrade = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/upgrade".to_string(),
        body: None,
    });
    assert_eq!(upgrade.status, 200);
    assert_eq!(upgrade.body["upgrade"]["available"], false);
    assert_eq!(upgrade.body["upgrade"]["upgrade_available"], false);
    assert_eq!(upgrade.body["upgrade"]["local_development"], true);

    let install = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/install".to_string(),
        body: Some(json!({"target": "linux-cli-web", "version": "1.0.0"})),
    });
    assert_eq!(install.status, 200);
    assert_eq!(install.body["install"]["installed"], true);
    assert_eq!(install.body["install"]["target"], "linux_cli_web");
    assert_eq!(install.body["install"]["port"], 8080);
    assert!(
        install.body["install"]["backend"]["service_metadata_path"]
            .as_str()
            .unwrap()
            .ends_with("refine-8080.service")
    );
    assert!(
        !temp_root.join("run/install-state.json").exists(),
        "HTTP installation must not create an unscoped install record"
    );

    let install_status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/system/install".to_string(),
        body: None,
    });
    assert_eq!(install_status.status, 200);
    assert_eq!(install_status.body["install"]["installed"], true);
    assert_eq!(install_status.body["install"]["port"], 8080);

    let repair = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/repair".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(repair.status, 200);
    assert_eq!(repair.body["install"]["port"], 8080);
    assert!(
        repair.body["install"]["backend"]["service_metadata_path"]
            .as_str()
            .unwrap()
            .ends_with("refine-8080.service")
    );

    let update = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/update".to_string(),
        body: Some(json!({"version": "1.1.0"})),
    });
    assert_eq!(update.status, 501);
    assert!(
        update.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("./r system update")
    );

    let install_status_after_update = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/system/install".to_string(),
        body: None,
    });
    assert_eq!(install_status_after_update.status, 200);
    assert_eq!(
        install_status_after_update.body["install"]["version"],
        "1.0.0"
    );

    let uninstall = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/system/uninstall".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(uninstall.status, 200);
    assert_eq!(uninstall.body["uninstalled"], true);

    let health = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/target-app/health".to_string(),
        body: Some(json!({})),
    });
    assert_eq!(health.status, 200);
    assert_eq!(health.body["last_check_ok"], true);

    let operation_registry = FileOperationRegistry::new(&runtime_root);
    let operation = operation_registry.register("sse-operation").unwrap();
    operation_registry
        .append_log(
            &operation.id,
            LogEntry {
                datetime: String::new(),
                severity: "info".to_string(),
                category: "operation".to_string(),
                message: "SSE operation progress".to_string(),
                details: None,
                actions: Vec::new(),
                actor: None,
                goal_id: None,
            },
        )
        .unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let stdout_path = runtime_root.join("sse.stdout.log");
    fs::write(&stdout_path, "SSE process output\n").unwrap();
    supervisor
        .register(ManagedProcess {
            id: "sse-process".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("sse".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap();
    let chat = FileChatService::new(&refine_dir);
    let session = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();
    chat.interrupt(&session.id, "SSE chat event").unwrap();
    fs::write(
        runtime_root.join("source-promotion.json"),
        serde_json::to_vec_pretty(&json!({
            "id": "source-sse",
            "status": "running",
            "stage": "build_candidate",
            "message": "Building source candidate"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        runtime_root.join("source-update-check.json"),
        serde_json::to_vec_pretty(&json!({
            "freshness": "fresh",
            "in_flight": false,
            "last_successful_check_at": "2026-08-06T12:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let daemon = LocalHttpDaemon {
        server,
        static_root: None,
    };
    let sse = daemon.handle_wire_request(HttpRequest {
        method: "GET".to_string(),
        path: "/api/sse".to_string(),
        headers: BTreeMap::new(),
        body: None,
    });
    assert_eq!(sse.status, 200);
    assert_eq!(sse.content_type, "text/event-stream");
    let sse_body = String::from_utf8(sse.body).unwrap();
    assert!(sse_body.contains("event: ready"));
    assert!(sse_body.contains("event: project_updated"));
    assert!(sse_body.contains("event: status_change"));
    assert!(sse_body.contains("event: runtime_change"));
    assert!(sse_body.contains("event: process_output"));
    assert!(sse_body.contains("SSE process output"));
    assert!(sse_body.contains("event: operation_progress"));
    assert!(sse_body.contains("SSE operation progress"));
    assert!(sse_body.contains("event: source_update"));
    assert!(sse_body.contains("Building source candidate"));
    assert!(sse_body.contains("2026-08-06T12:00:00Z"));
    assert!(sse_body.contains("event: chat_event"));
    assert!(sse_body.contains("SSE chat event"));

    remove_temp_dir(&temp_root);
}

#[test]
fn http_lifecycle_routes_preserve_shared_status_and_evidence_for_every_action() {
    #[derive(Default)]
    struct RecordingLifecycle {
        actions: std::cell::RefCell<Vec<&'static str>>,
    }

    impl crate::tools::host::daemon_lifecycle::HostDaemonLifecycleService for RecordingLifecycle {
        fn start(
            &self,
            config: crate::process::supervisor::lifecycle::BackgroundDaemonConfig,
        ) -> RefineResult<crate::process::supervisor::lifecycle::DaemonStatus> {
            self.actions.borrow_mut().push("start");
            Ok(http_lifecycle_status(config.port, "start"))
        }

        fn stop(
            &self,
            port: u16,
        ) -> RefineResult<crate::process::supervisor::lifecycle::DaemonStatus> {
            self.actions.borrow_mut().push("stop");
            Ok(http_lifecycle_status(port, "stop"))
        }

        fn restart(
            &self,
            config: crate::process::supervisor::lifecycle::BackgroundDaemonConfig,
        ) -> RefineResult<crate::process::supervisor::lifecycle::DaemonStatus> {
            self.actions.borrow_mut().push("restart");
            Ok(http_lifecycle_status(config.port, "restart"))
        }
    }

    let server = server_with_projection();
    let lifecycle = RecordingLifecycle::default();
    for (action, expected) in [
        (
            crate::tools::host::daemon_lifecycle::DaemonLifecycleAction::Start,
            "start",
        ),
        (
            crate::tools::host::daemon_lifecycle::DaemonLifecycleAction::Stop,
            "stop",
        ),
        (
            crate::tools::host::daemon_lifecycle::DaemonLifecycleAction::Restart,
            "restart",
        ),
    ] {
        let response = server.handle_daemon_lifecycle_with(action, &lifecycle);
        assert_eq!(response.status, 200);
        assert_eq!(response.body["status"]["port"], 8080);
        assert_eq!(response.body["status"]["worker_state"], expected);
        assert_eq!(
            response.body["status"]["lifecycle_evidence"]["action"],
            expected
        );
        assert_eq!(
            response.body["status"]["lifecycle_evidence"]["outcome"],
            format!("{expected}_shared")
        );
    }
    assert_eq!(
        lifecycle.actions.into_inner(),
        vec!["start", "stop", "restart"]
    );
}

#[test]
fn http_stop_and_restart_return_durable_receipts_before_control_handoff() {
    #[derive(Default)]
    struct RecordingHandoff {
        actions: std::cell::RefCell<Vec<String>>,
    }

    impl crate::tools::host::daemon_lifecycle::RestartSafeHandoffLauncher for RecordingHandoff {
        fn launch(
            &self,
            handoff: &crate::tools::host::daemon_lifecycle::RestartSafeHandoff,
            service_manager: Option<&str>,
        ) -> RefineResult<()> {
            assert_eq!(service_manager, Some("systemd_user"));
            self.actions.borrow_mut().push(handoff.args[3].clone());
            Ok(())
        }
    }

    let temp_root = unique_temp_dir("http-lifecycle-handoff");
    let mut server = server_with_projection();
    server.runtime_root = Some(temp_root.join("run/8080"));
    let handoff = RecordingHandoff::default();

    for (action, expected) in [
        (
            crate::tools::host::daemon_lifecycle::DaemonLifecycleAction::Stop,
            "stop",
        ),
        (
            crate::tools::host::daemon_lifecycle::DaemonLifecycleAction::Restart,
            "restart",
        ),
    ] {
        let response = server.handle_daemon_lifecycle_handoff_with(
            action,
            Path::new("/mock/refine"),
            Some("systemd_user"),
            &handoff,
        );
        assert_eq!(response.status, 202);
        assert_eq!(response.body["operation"]["action"], expected);
        assert_eq!(response.body["operation"]["status"], "queued");
        let operation_id = response.body["operation"]["id"].as_str().unwrap();
        let reconciled = server.handle_daemon_lifecycle_operation(operation_id);
        assert_eq!(reconciled.status, 200);
        assert_eq!(reconciled.body["operation"], response.body["operation"]);
    }
    assert_eq!(handoff.actions.into_inner(), vec!["stop", "restart"]);

    remove_temp_dir(&temp_root);
}

fn http_lifecycle_status(
    port: u16,
    action: &str,
) -> crate::process::supervisor::lifecycle::DaemonStatus {
    crate::process::supervisor::lifecycle::DaemonStatus {
        port,
        daemon_healthy: action != "stop",
        web_available: action != "stop",
        worker_state: action.to_string(),
        target_app_state: "detached".to_string(),
        launch_mode: "test".to_string(),
        executable_path: None,
        active_operations: Vec::new(),
        degraded_integrations: Vec::new(),
        lifecycle_evidence: Some(
            crate::process::supervisor::lifecycle::DaemonLifecycleEvidence {
                action: action.to_string(),
                service_manager: "test".to_string(),
                outcome: format!("{action}_shared"),
                command_error: None,
                readiness_error: None,
                observed_reachable: Some(action != "stop"),
                recovery: None,
            },
        ),
    }
}

#[test]
fn web_server_lists_processes_and_updates_pause_controls() {
    let temp_root = unique_temp_dir("http-processes");
    let runtime_root = temp_root.join("run/8080");
    init_git_app(&temp_root);
    let refine_dir = refine_dir_for_target_root(&temp_root).unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime_root);
    let chat = FileChatService::with_runtime_root(&refine_dir, &runtime_root);
    let standalone_chat = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), None)
        .unwrap();
    let goal_chat = chat
        .start_with_options(
            ChatAttachment::Goal("GOALCHAT".to_string()),
            Some("smoke-ai"),
            Some("goal"),
        )
        .unwrap();
    let stopped_chat = chat
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), None)
        .unwrap();
    chat.stop(&stopped_chat.id).unwrap();
    supervisor
        .register(ManagedProcess {
            id: "supervisor-context".to_string(),
            owner: ProcessOwner::Daemon,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("setsid".to_string()),
            details: None,
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    // Launch a real, long-lived agent process so it stays alive (and counted)
    // through the assertions below. A short-lived command would exit before the
    // summary call and be pruned by liveness recovery, racing the agent count.
    let launched_agent = supervisor
        .launch(crate::process::subprocess::ManagedProcessSpec {
            owner: crate::process::subprocess::ProcessOwner::Agent,
            command: if cfg!(windows) { "cmd" } else { "sleep" }.to_string(),
            args: if cfg!(windows) {
                vec!["/C".to_string(), "ping -n 30 127.0.0.1 >NUL".to_string()]
            } else {
                vec!["30".to_string()]
            },
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: None,
            sensitive: false,
            metadata: Default::default(),
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "agent-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Agent context".to_string()),
            details: Some(json!({"goal_id": "GOALCTX", "round_idx": 1}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    FileProcessSupervisor::new(runtime_root.join("agents"))
        .register(ManagedProcess {
            id: "background-agent-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Background agent context".to_string()),
            details: Some(json!({"goal_id": "GOALBACKGROUND", "round_idx": 0}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "chat-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("Chat context".to_string()),
            details: Some(
                json!({"session_id": "chat-context-session", "mode": "standalone"}).to_string(),
            ),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "ui-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("UI context".to_string()),
            details: Some(json!({"kind": "ui"}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "exited-agent-context".to_string(),
            owner: crate::process::subprocess::ProcessOwner::Agent,
            pid: None,
            state: "exited".to_string(),
            label: Some("Exited agent context".to_string()),
            details: Some(json!({"goal_id": "DONECTX", "round_idx": 1}).to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap();
    fs::write(runtime_root.join("processes/empty-process.json"), "").unwrap();
    fs::write(
        runtime_root.join("processes/malformed-process.json"),
        "{not json",
    )
    .unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());

    let listed = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes".to_string(),
        body: None,
    });
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["processes"][0]["kind"], "agent");
    assert_eq!(listed.body["runner_reachable"], false);
    assert_eq!(
        listed.body["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .map(|work| work["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "merger",
            "plan_draft_extractor",
            "target_app_builder",
            "target_app_config_generator",
            "sqlite_cache_rebuild",
            "activity_log_cleanup"
        ]
    );
    assert!(
        listed.body["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .all(|work| work["status"] == "idle")
    );
    let listed_processes = listed.body["processes"].as_array().unwrap();
    let supervisor_context = listed_processes
        .iter()
        .find(|process| process["id"] == "supervisor-context")
        .unwrap();
    assert_eq!(supervisor_context["kind"], "daemon");
    assert_eq!(supervisor_context["actions"], json!(["terminate", "kill"]));
    assert_eq!(
        supervisor_context["management_actions"],
        json!(["pause_workflow"])
    );
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == "exited-agent-context")
    );
    let agent_context = listed_processes
        .iter()
        .find(|process| process["id"] == "agent-context")
        .unwrap();
    assert_eq!(agent_context["goal_id"], "GOALCTX");
    assert_eq!(agent_context["round_idx"], 1);
    assert_eq!(agent_context["management_actions"], json!(["stop_agent"]));
    let background_agent_context = listed_processes
        .iter()
        .find(|process| process["id"] == "background-agent-context")
        .unwrap();
    assert_eq!(background_agent_context["kind"], "agent");
    assert_eq!(background_agent_context["goal_id"], "GOALBACKGROUND");
    assert_eq!(background_agent_context["round_idx"], 0);
    assert_eq!(
        background_agent_context["management_actions"],
        json!(["stop_agent"])
    );
    let chat_context = listed_processes
        .iter()
        .find(|process| process["id"] == "chat-context")
        .unwrap();
    assert_eq!(chat_context["kind"], "chat");
    assert_eq!(chat_context["session_id"], "chat-context-session");
    assert_eq!(chat_context["mode"], "standalone");
    assert_eq!(chat_context["management_actions"], json!(["stop_agent"]));
    let standalone_context = listed_processes
        .iter()
        .find(|process| process["id"] == format!("chat-session-{}", standalone_chat.id))
        .unwrap();
    assert_eq!(standalone_context["kind"], "chat");
    assert_eq!(standalone_context["session_id"], standalone_chat.id);
    assert_eq!(standalone_context["mode"], "standalone");
    assert_eq!(
        standalone_context["management_actions"],
        json!(["stop_agent"])
    );
    let goal_chat_context = listed_processes
        .iter()
        .find(|process| process["id"] == format!("chat-session-{}", goal_chat.id))
        .unwrap();
    assert_eq!(goal_chat_context["kind"], "chat");
    assert_eq!(goal_chat_context["session_id"], goal_chat.id);
    assert_eq!(goal_chat_context["mode"], "goal");
    assert_eq!(goal_chat_context["goal_id"], "GOALCHAT");
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == format!("chat-session-{}", stopped_chat.id))
    );
    let ui_context = listed_processes
        .iter()
        .find(|process| process["id"] == "ui-context")
        .unwrap();
    assert_eq!(ui_context["kind"], "ui");
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == "empty-process")
    );
    assert!(
        !listed_processes
            .iter()
            .any(|process| process["id"] == "malformed-process")
    );

    supervisor
        .register(ManagedProcess {
            id: "exited-target-context".to_string(),
            owner: ProcessOwner::TargetApp,
            pid: None,
            state: "exited".to_string(),
            label: Some("sh".to_string()),
            details: Some("-c old-app-status".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap();
    supervisor
        .register(ManagedProcess {
            id: "dead-target-context".to_string(),
            owner: ProcessOwner::TargetApp,
            pid: Some(99_999_999),
            state: "running".to_string(),
            label: Some("sh".to_string()),
            details: Some("-c stale-app-status".to_string()),
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let listed_after_target_records = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes".to_string(),
        body: None,
    });
    assert_eq!(listed_after_target_records.status, 200);
    let listed_after_target_records = listed_after_target_records.body["processes"]
        .as_array()
        .unwrap();
    assert!(
        !listed_after_target_records
            .iter()
            .any(|process| process["id"] == "exited-target-context")
    );
    assert!(
        !listed_after_target_records
            .iter()
            .any(|process| process["id"] == "dead-target-context")
    );
    assert!(supervisor.inspect("dead-target-context").is_err());

    let summary = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes?summary=1".to_string(),
        body: None,
    });
    assert_eq!(summary.status, 200);
    assert_eq!(summary.body["agent_count"], 3);
    assert_eq!(summary.body["process_count"], 8);
    assert_eq!(summary.body["processes"].as_array().unwrap().len(), 0);
    let cached = server.current_runtime_projection().unwrap();
    assert!(
        cached
            .processes
            .iter()
            .any(|process| process["goal_id"] == "GOALCTX")
    );
    assert_eq!(cached.supervisor.unwrap()["runner_reachable"], json!(false));

    let stdout_path = runtime_root.join("stream.stdout.log");
    let stderr_path = runtime_root.join("stream.stderr.log");
    fs::write(&stdout_path, "hello stdout\n").unwrap();
    fs::write(&stderr_path, "warn stderr\n").unwrap();
    supervisor
        .register(crate::process::subprocess::ManagedProcess {
            id: "stream-test".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: Some(std::process::id()),
            state: "running".to_string(),
            label: Some("stream".to_string()),
            details: None,
            stdout_path: Some(stdout_path.display().to_string()),
            stderr_path: Some(stderr_path.display().to_string()),
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: Some(0),
        })
        .unwrap();
    let stream = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/processes/stream-test/stream".to_string(),
        body: None,
    });
    assert_eq!(stream.status, 200);
    assert_eq!(stream.body["process_id"], "stream-test");
    assert!(
        stream.body["output"]
            .as_str()
            .unwrap()
            .contains("hello stdout")
    );
    assert!(
        stream.body["output"]
            .as_str()
            .unwrap()
            .contains("warn stderr")
    );
    supervisor
        .register(crate::process::subprocess::ManagedProcess {
            id: "stop-test".to_string(),
            owner: crate::process::subprocess::ProcessOwner::UserHelper,
            pid: None,
            state: "running".to_string(),
            label: Some("stop".to_string()),
            details: None,
            stdout_path: None,
            stderr_path: None,
            stdin_path: None,
            limits: None,
            started_at: String::new(),
            exit_code: None,
        })
        .unwrap();
    let stopped = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/stop-test/stop".to_string(),
        body: None,
    });
    assert_eq!(stopped.status, 200);
    assert_eq!(stopped.body["stopped"], true);
    assert_eq!(stopped.body["process"]["id"], "stop-test");
    assert!(supervisor.inspect("stop-test").is_err());

    let legacy_background_pause = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/background".to_string(),
        body: Some(json!({"stopped": true})),
    });
    assert_eq!(legacy_background_pause.status, 200);
    assert_eq!(legacy_background_pause.body["workflow_paused"], true);
    let legacy_agent_resume = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/processes/agents".to_string(),
        body: Some(json!({"paused": false})),
    });
    assert_eq!(legacy_agent_resume.status, 200);
    assert_eq!(legacy_agent_resume.body["workflow_paused"], false);

    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Pause workflow drain", Some("GOAL-WORKFLOW"))
        .unwrap();
    work_items
        .transition_goal_status("GOAL-WORKFLOW", GoalStatus::Todo)
        .unwrap();
    work_items
        .advance_automated_goal_status("GOAL-WORKFLOW", GoalStatus::InProgress)
        .unwrap();
    let paused = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/workflow/pause".to_string(),
        body: Some(json!({"paused": true})),
    });
    assert_eq!(paused.status, 200);
    assert_eq!(paused.body["paused"], true);
    assert_eq!(paused.body["workflow_paused"], true);
    assert_eq!(paused.body["background_processes_stopped"], true);
    assert_eq!(paused.body["agents_paused"], true);
    let paused_supervisor = paused.body["processes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|process| process["id"] == "supervisor-context")
        .unwrap();
    assert_eq!(
        paused_supervisor["management_actions"],
        json!(["unpause_workflow"])
    );
    assert_eq!(
        work_items
            .show_goal_summary("GOAL-WORKFLOW")
            .unwrap()
            .goal
            .status,
        GoalStatus::InProgress
    );
    assert_eq!(
        supervisor.inspect(&launched_agent.id).unwrap().state,
        "running"
    );
    assert!(
        paused.body["runner_work"]
            .as_array()
            .unwrap()
            .iter()
            .all(|work| work["status"] == "paused")
    );

    let resumed = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/workflow/pause".to_string(),
        body: Some(json!({"paused": false})),
    });
    assert_eq!(resumed.status, 200);
    assert_eq!(resumed.body["paused"], false);
    assert_eq!(resumed.body["workflow_paused"], false);
    assert_eq!(resumed.body["background_processes_stopped"], false);
    assert_eq!(resumed.body["agents_paused"], false);
    assert!(runtime_root.join("process-control.json").exists());
    let cached = server.current_runtime_projection().unwrap();
    assert_eq!(cached.supervisor.unwrap()["workflow_paused"], json!(false));

    // Terminate the long-lived agent so the test leaves no orphaned process.
    let _ = supervisor.signal(&launched_agent.id, "terminate");

    remove_temp_dir(&temp_root);
}

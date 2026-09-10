use super::*;

#[test]
fn web_server_serves_source_file_tree_read_and_search() {
    let temp_root = unique_temp_dir("http-files");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(temp_root.join("src")).unwrap();
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(temp_root.join("README.md"), "hello\nworld\n").unwrap();
    fs::write(temp_root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        temp_root.join("pixel.png"),
        [
            0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0x00, 0x00, 0x00, 0x00,
        ],
    )
    .unwrap();
    fs::write(temp_root.join("artifact.bin"), [0x00, 0x01, 0x02]).unwrap();
    fs::write(refine_dir.join("refine.json"), "{}").unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());

    let tree = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/tree?path=&recursive=1&max_depth=2&max_entries=20".to_string(),
        body: None,
    });
    assert_eq!(tree.status, 200);
    let root_entries = tree.body["entries_by_path"][""].as_array().unwrap();
    assert!(
        root_entries
            .iter()
            .any(|entry| entry["path"] == "README.md")
    );
    assert!(root_entries.iter().any(|entry| entry["path"] == "src"));
    assert!(!root_entries.iter().any(|entry| entry["path"] == ".refine"));
    let src_index = root_entries
        .iter()
        .position(|entry| entry["path"] == "src")
        .unwrap();
    let readme_index = root_entries
        .iter()
        .position(|entry| entry["path"] == "README.md")
        .unwrap();
    assert!(src_index < readme_index);
    assert!(
        tree.body["entries_by_path"]["src"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "src/main.rs")
    );

    let read = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=README.md&offset=0&limit=6".to_string(),
        body: None,
    });
    assert_eq!(read.status, 200);
    assert_eq!(read.body["previewable"], true);
    assert_eq!(read.body["content"], "hello\n");
    assert_eq!(read.body["has_more"], true);
    assert_eq!(read.body["next_offset"], 6);

    let image = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=pixel.png".to_string(),
        body: None,
    });
    assert_eq!(image.status, 200);
    assert_eq!(image.body["previewable"], true);
    assert_eq!(image.body["kind"], "image");
    assert_eq!(image.body["mime_type"], "image/png");
    assert!(
        image.body["data_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );

    let binary = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=artifact.bin".to_string(),
        body: None,
    });
    assert_eq!(binary.status, 200);
    assert_eq!(binary.body["previewable"], false);
    assert_eq!(binary.body["kind"], "binary");

    let search = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/search?q=main&max_entries=5".to_string(),
        body: None,
    });
    assert_eq!(search.status, 200);
    assert_eq!(search.body["entries"][0]["path"], "src/main.rs");

    let traversal = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: "/api/files/read?path=../Cargo.toml".to_string(),
        body: None,
    });
    assert_eq!(traversal.status, 400);
    assert_eq!(traversal.body["error"]["code"], "invalid_input");

    remove_temp_dir(&temp_root);
}

#[test]
fn web_server_runs_interactive_terminal_session() {
    let temp_root = unique_temp_dir("http-terminal");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(&refine_dir).unwrap();
    fs::write(temp_root.join("README.md"), "terminal root\n").unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(refine_dir.parent().unwrap().to_path_buf());
    server.runtime_root = Some(temp_root.join("run/8080"));

    let start = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({"cols": 80, "rows": 20})),
    });
    assert_eq!(start.status, 200, "{}", start.body);
    assert_eq!(start.body["cwd"], temp_root.display().to_string());
    assert_eq!(start.body["profile"], "terminal");
    assert!(
        start.body["process_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("interactive-"))
    );
    let session_id = start.body["id"].as_str().unwrap().to_string();
    let process_id = start.body["process_id"].as_str().unwrap();
    let managed = FileProcessSupervisor::new(server.runtime_root.as_ref().unwrap())
        .list()
        .unwrap();
    assert!(managed.iter().any(|process| process.id == process_id));

    let status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/terminal/{session_id}/status"),
        body: None,
    });
    assert_eq!(status.status, 200, "{}", status.body);
    assert_eq!(status.body["id"], session_id);
    assert_eq!(status.body["process_id"], process_id);
    assert_eq!(status.body["alive"], true);
    assert_eq!(status.body["exited"], false);

    let resize = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/resize"),
        body: Some(json!({"cols": 120, "rows": 36})),
    });
    assert_eq!(resize.status, 200, "{}", resize.body);

    let input = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/input"),
        body: Some(json!({"data": "printf 'terminal:%s' \"$(cat README.md)\"\r"})),
    });
    assert_eq!(input.status, 200);

    let mut output = String::new();
    for _ in 0..40 {
        let events = server.handle(ApiRequest {
            method: "GET".to_string(),
            path: format!("/api/terminal/{session_id}/events"),
            body: None,
        });
        assert_eq!(events.status, 200);
        for event in events.body["events"].as_array().unwrap() {
            output.push_str(event["data"].as_str().unwrap_or(""));
        }
        if output.contains("terminal:terminal root") {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(output.contains("terminal:terminal root"), "{output:?}");

    let stop = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/terminal/{session_id}/stop"),
        body: None,
    });
    assert_eq!(stop.status, 200);

    let stopped_status = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/terminal/{session_id}/status"),
        body: None,
    });
    assert_eq!(stopped_status.status, 404);

    let second = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/terminal/session".to_string(),
        body: Some(json!({"cols": 80, "rows": 20})),
    });
    assert_eq!(second.status, 200, "{}", second.body);
    let second_process_id = second.body["process_id"].as_str().unwrap().to_string();
    let process_stop = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/processes/{second_process_id}/stop"),
        body: Some(json!({"signal": "terminate"})),
    });
    assert_eq!(process_stop.status, 200, "{}", process_stop.body);
    assert_eq!(process_stop.body["process"]["kind"], "interactive_session");
    for _ in 0..40 {
        if !FileProcessSupervisor::new(server.runtime_root.as_ref().unwrap())
            .list()
            .unwrap()
            .iter()
            .any(|process| process.id == second_process_id)
        {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !FileProcessSupervisor::new(server.runtime_root.as_ref().unwrap())
            .list()
            .unwrap()
            .iter()
            .any(|process| process.id == second_process_id)
    );

    remove_temp_dir(&temp_root);
}

#[test]
fn web_terminal_input_stays_with_its_managed_session() {
    let temp_root = unique_temp_dir("http-terminal-input-isolation");
    let refine_dir = temp_root.join(".refine");
    fs::create_dir_all(&refine_dir).unwrap();

    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(temp_root.join("run/8080"));

    let mut session_ids = Vec::new();
    for _ in 0..2 {
        let response = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/terminal/session".to_string(),
            body: Some(json!({"profile": "terminal", "cols": 80, "rows": 20})),
        });
        assert_eq!(response.status, 200, "{}", response.body);
        session_ids.push(response.body["id"].as_str().unwrap().to_string());
    }
    let first_id = session_ids.remove(0);
    let second_id = session_ids.remove(0);
    assert_ne!(first_id, second_id);

    for (session_id, marker) in [
        (&first_id, "__clipboard_first__"),
        (&second_id, "__clipboard_second__"),
    ] {
        let response = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/terminal/{session_id}/input"),
            body: Some(json!({"data": format!("printf '{marker}\\n'\r")})),
        });
        assert_eq!(response.status, 200, "{}", response.body);
    }

    let mut outputs = [String::new(), String::new()];
    for _ in 0..40 {
        for (index, session_id) in [&first_id, &second_id].into_iter().enumerate() {
            let response = server.handle(ApiRequest {
                method: "GET".to_string(),
                path: format!("/api/terminal/{session_id}/events"),
                body: None,
            });
            assert_eq!(response.status, 200, "{}", response.body);
            outputs[index] = response.body["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|event| event["data"].as_str())
                .collect();
        }
        if outputs[0].contains("__clipboard_first__") && outputs[1].contains("__clipboard_second__")
        {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    assert!(
        outputs[0].contains("__clipboard_first__"),
        "{:?}",
        outputs[0]
    );
    assert!(
        !outputs[0].contains("__clipboard_second__"),
        "{:?}",
        outputs[0]
    );
    assert!(
        outputs[1].contains("__clipboard_second__"),
        "{:?}",
        outputs[1]
    );
    assert!(
        !outputs[1].contains("__clipboard_first__"),
        "{:?}",
        outputs[1]
    );

    for session_id in [&first_id, &second_id] {
        let response = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: format!("/api/terminal/{session_id}/stop"),
            body: None,
        });
        assert_eq!(response.status, 200, "{}", response.body);
    }
    remove_temp_dir(&temp_root);
}

#[test]
fn operation_sse_keeps_all_active_operations_beyond_recent_terminal_limit() {
    let runtime_root = unique_temp_dir("operation-sse-active-window");
    let registry = FileOperationRegistry::new(&runtime_root);
    let active = registry.register("long-running-operation").unwrap();

    for index in 0..11 {
        let terminal = registry
            .register(&format!("completed-operation-{index}"))
            .unwrap();
        registry
            .succeed_with_result_and_progress(
                &terminal.id,
                json!({"stage": "complete"}),
                json!({"ok": true}),
            )
            .unwrap();
    }

    let events = recent_operation_sse_events(&runtime_root, 10).unwrap();
    assert_eq!(events.len(), 11);
    assert!(events.iter().any(|event| {
        event["operation"]["id"].as_str() == Some(active.id.as_str())
            && event["operation"]["status"] == "running"
    }));

    fs::remove_dir_all(runtime_root).unwrap();
}

#[cfg(unix)]
#[test]
fn custom_skill_terminal_uses_selected_instructions_and_managed_session_without_goal() {
    use std::os::unix::fs::PermissionsExt;
    let _env = crate::infrastructure::agents::invocation::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp_root = unique_temp_dir("http-skill-terminal");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    fs::create_dir_all(&refine_dir).unwrap();
    let provider = temp_root.join("skill-provider");
    fs::write(&provider, "#!/usr/bin/env python3\nimport pathlib,sys,time\npathlib.Path('skill-prompt.txt').write_text(sys.argv[-1])\nprint('Skill agent ready',flush=True)\ntime.sleep(30)\n").unwrap();
    fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe {
                if let Some(value) = &self.0 {
                    std::env::set_var("REFINE_SMOKE_AI_PATH", value);
                } else {
                    std::env::remove_var("REFINE_SMOKE_AI_PATH");
                }
            }
        }
    }
    let _restore = Restore(std::env::var_os("REFINE_SMOKE_AI_PATH"));
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }
    crate::infrastructure::process::supervisor::config::FileSettingsService::with_active_root(
        &refine_dir,
        &runtime_root,
    )
    .update(&json!({"agent_cli":"smoke-ai"}))
    .unwrap();
    let service =
        crate::application::events::FileEventService::with_runtime_root(&refine_dir, &runtime_root);
    let revision = service.config().unwrap().revision;
    service.save("skills", "release", json!({"revision":revision,"item":{"name":"Release check","prompt":"Inspect only the release notes.","parameters":[{"name":"count","kind":"number","required":true}]},"trigger":{"source":"custom"}})).unwrap();
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    for extra in [
        json!({"goal_id":"GOAL1"}),
        json!({"surface":"cli"}),
        json!({"parameters":{"count":"wrong"}}),
    ] {
        let mut body = json!({"profile":"skill","surface":"toolbar","skill_id":"release","parameters":{"count":4},"cols":80,"rows":20});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let response = server.handle(ApiRequest {
            method: "POST".into(),
            path: "/api/terminal/session".into(),
            body: Some(body),
        });
        assert_eq!(response.status, 400, "{}", response.body);
    }
    let start = server.handle(ApiRequest { method:"POST".into(),path:"/api/terminal/session".into(),body:Some(json!({"profile":"skill","surface":"toolbar","skill_id":"release","parameters":{"count":4},"cols":80,"rows":20})) });
    assert_eq!(start.status, 200, "{}", start.body);
    assert_eq!(start.body["profile"], "skill");
    let process_id = start.body["process_id"].as_str().unwrap();
    let records = FileProcessSupervisor::new(&runtime_root).list().unwrap();
    let record = records.iter().find(|p| p.id == process_id).unwrap();
    let metadata: serde_json::Value =
        serde_json::from_str(record.details.as_deref().unwrap()).unwrap();
    assert_eq!(metadata["skill_id"], "release");
    assert!(metadata.get("goal_id").is_none());
    for _ in 0..100 {
        if temp_root.join("skill-prompt.txt").exists() {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let prompt = fs::read_to_string(temp_root.join("skill-prompt.txt")).unwrap();
    assert!(prompt.contains("Inspect only the release notes."));
    assert!(prompt.contains("\"count\":4"));
    let stop = server.handle(ApiRequest {
        method: "POST".into(),
        path: format!("/api/terminal/{}/stop", start.body["id"].as_str().unwrap()),
        body: None,
    });
    assert_eq!(stop.status, 200, "{}", stop.body);
    remove_temp_dir(&temp_root);
}

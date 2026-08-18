use super::*;

#[test]
fn file_chat_service_streams_provider_output_into_progress() {
    let temp_root = unique_temp_dir("chat-provider-stream");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    write_fake_provider_script(
        &refine_dir,
        "claude",
        "#!/bin/sh\nprintf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"streamed activity line\"}}'\nsleep 1\nprintf '%s\\n' '{\"item\":{\"type\":\"agent_message\",\"text\":\"final response line\"}}'\n",
    );
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("claude"), Some("chat"))
        .unwrap();

    service.append_user_message(&session.id, "hello").unwrap();
    let streamed = wait_for_chat_read(&service, &session.id, |read| {
        read.in_flight
            && read
                .progress_lines
                .iter()
                .any(|line| line.contains("streamed activity line"))
    });
    assert!(
        streamed
            .progress_lines
            .iter()
            .any(|line| line.contains("streamed activity line"))
    );
    let completed = wait_for_chat_read(&service, &session.id, |read| !read.in_flight);
    assert!(!completed.in_flight);
    let record = wait_for_chat_record(&service, &session.id, |record| {
        record.transcript_events.iter().any(|event| {
            event
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|line| line.contains("final response line"))
        })
    });
    assert!(record.transcript_events.iter().any(|event| {
        event
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|line| line.contains("final response line"))
    }));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_persists_importable_artifacts_from_provider_output() {
    let temp_root = unique_temp_dir("chat-artifacts");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    write_fake_provider(
        &refine_dir,
        "smoke-ai",
        0,
        r#"{"importable_artifacts":[{"type":"round","round":{"reporter":"QA","prompt": "Fixed"}},{"type":"goal","goal":{"name":"Imported goal","prompt": "B"}}]}"#,
    );
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();

    service
        .append_user_message(&session.id, "draft follow-up")
        .unwrap();
    let resumed = wait_for_chat_record(&service, &session.id, |record| {
        record.importable_artifacts.len() == 2
    });
    assert_eq!(resumed.importable_artifacts.len(), 2);
    assert_eq!(resumed.importable_artifacts[0]["type"], "round");
    assert_eq!(resumed.importable_artifacts[1]["type"], "goal");
    assert!(resumed.transcript_events.iter().any(|event| {
        event
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .contains("Detected 2 importable artifact")
    }));
    let read = service.read(&session.id).unwrap();
    assert_eq!(read.importable_artifacts.len(), 2);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_persists_provider_failure() {
    let temp_root = unique_temp_dir("chat-failure");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    write_fake_provider(&refine_dir, "smoke-ai", 2, "provider failed");
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();

    service.append_user_message(&session.id, "hello").unwrap();
    let resumed = wait_for_chat_record(&service, &session.id, |record| record.interrupted);
    assert!(resumed.interrupted);
    assert!(
        resumed
            .interruption_detail
            .as_deref()
            .unwrap_or("")
            .contains("provider failed")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_persists_provider_session_id_and_in_flight_lifecycle() {
    let temp_root = unique_temp_dir("chat-provider-session");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    write_fake_provider(
        &refine_dir,
        "smoke-ai",
        0,
        r#"{"session_id":"prov-1","item":{"type":"agent_message","text":"provider says hello"}}"#,
    );
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();

    service.append_user_message(&session.id, "hello").unwrap();
    let resumed = wait_for_chat_record(&service, &session.id, |record| {
        record.provider_session_id.as_deref() == Some("prov-1")
    });
    assert_eq!(resumed.provider_session_id.as_deref(), Some("prov-1"));
    assert!(!resumed.in_flight);
    assert_eq!(resumed.last_turn_started_at, None);
    assert!(!resumed.interrupted);
    let persisted: Value = serde_json::from_str(
        &fs::read_to_string(refine_dir.join(format!("chat/sessions/{}.json", session.id))).unwrap(),
    )
    .unwrap();
    assert!(persisted.get("in_flight").is_none());
    assert!(persisted.get("last_turn_started_at").is_none());
    let operations = FileOperationRegistry::new(&service.runtime_root)
        .recover()
        .unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].owner, format!("chat:{}", session.id));
    assert_eq!(operations[0].state, OperationState::Succeeded);

    let read = service.read(&session.id).unwrap();
    assert!(!read.in_flight);
    assert_eq!(read.provider_session_id.as_deref(), Some("prov-1"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_recovers_stale_in_flight_turns() {
    let temp_root = unique_temp_dir("chat-recovery");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();
    let registry = FileOperationRegistry::new(&service.runtime_root);
    let operation = registry.register(&format!("chat:{}", session.id)).unwrap();

    let recovered = service
        .recover_interrupted_turns("daemon restarted during provider turn")
        .unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(
        registry.status(&operation.id).unwrap().state,
        OperationState::Interrupted
    );
    let resumed = service.resume(&session.id).unwrap();
    assert!(!resumed.in_flight);
    assert_eq!(resumed.last_turn_started_at, None);
    assert!(resumed.interrupted);
    assert_eq!(
        resumed.interruption_detail.as_deref(),
        Some("daemon restarted during provider turn")
    );
    assert!(resumed.transcript_events.iter().any(|event| {
        event_text(event)
            .as_deref()
            .unwrap_or("")
            .contains("daemon restarted")
    }));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_resumes_provider_session_when_supported() {
    let temp_root = unique_temp_dir("chat-provider-resume");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    write_fake_provider(
        &refine_dir,
        "claude",
        0,
        r#"{"session_id":"prov-2","item":{"type":"agent_message","text":"resumed ok"}}"#,
    );
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("claude"), Some("chat"))
        .unwrap();
    let mut record = service.load_record(&session.id).unwrap();
    record.provider_session_id = Some("prov-1".to_string());
    record.interrupted = true;
    record.interruption_detail = Some("daemon restarted".to_string());
    service.write_record(&record).unwrap();

    let resumed = service.resume_provider_turn(&session.id).unwrap();
    assert_eq!(resumed.provider_session_id.as_deref(), Some("prov-2"));
    assert!(!resumed.in_flight);
    assert!(!resumed.interrupted);
    assert!(
        resumed
            .transcript_events
            .iter()
            .any(|event| event_text(event).as_deref() == Some("resumed ok"))
    );

    fs::remove_dir_all(temp_root).unwrap();
}

use super::*;

#[test]
fn retired_supervisor_sessions_cannot_start_and_are_purged() {
    let temp_root = unique_temp_dir("retired-supervisor-chat");
    let service = FileChatService::new(temp_root.join(".refine"));
    let error = service
        .start_with_options(
            ChatAttachment::Supervisor,
            Some("smoke-ai"),
            Some("supervisor"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("retired"));

    let legacy = service
        .start_record_with_options(
            ChatAttachment::Supervisor,
            Some("smoke-ai"),
            Some("supervisor"),
        )
        .unwrap();
    assert!(service.session_path(&legacy.id).exists());
    assert_eq!(service.purge_supervisor_sessions().unwrap(), 1);
    assert!(!service.session_path(&legacy.id).exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_persists_session_transcript_and_stop() {
    let temp_root = unique_temp_dir("chat");
    let refine_dir = temp_root.join(".refine");
    write_fake_provider(&refine_dir, "smoke-ai", 0, "provider says hello");
    let service = FileChatService::new(&refine_dir);

    let session = service
        .start_with_options(
            ChatAttachment::Goal("GOAL1".to_string()),
            Some("smoke-ai"),
            Some("goal"),
        )
        .unwrap();
    assert_eq!(session.mode, "goal");
    assert_eq!(session.provider, "smoke-ai");

    service
        .append_user_message(&session.id, "What should I test?")
        .unwrap();
    let queued = service.read(&session.id).unwrap();
    assert!(queued.in_flight || !queued.queued_messages.is_empty());
    let streamed = wait_for_chat_line(&service, &session.id, "provider says hello");
    assert!(streamed.alive);
    assert!(
        streamed
            .lines
            .iter()
            .any(|line| line.contains("provider says hello"))
            || streamed
                .progress_lines
                .iter()
                .any(|line| line.contains("provider says hello"))
    );
    let record = wait_for_chat_record(&service, &session.id, |record| {
        record.transcript_events.iter().any(|event| {
            event
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|line| line.contains("Provider turn completed"))
        })
    });
    let read = service.read(&session.id).unwrap();
    assert!(read.alive);
    assert!(record.transcript_events.iter().any(|event| {
        event
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|line| line.contains("What should I test?"))
    }));
    assert!(record.transcript_events.iter().any(|event| {
        event
            .get("text")
            .and_then(Value::as_str)
            .is_some_and(|line| line.contains("Provider turn completed"))
    }));
    let stopped = service.stop(&session.id).unwrap();
    assert!(stopped.closed);
    assert!(!service.read(&session.id).unwrap().alive);

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_rebuilds_attached_goal_context_from_refine_records() {
    let temp_root = unique_temp_dir("chat-goal-context");
    let refine_dir = temp_root.join(".refine");
    FileWorkItemService::new(&refine_dir)
        .create_goal_summary("Checkout fails", Some("GOAL1"))
        .unwrap();
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(
            ChatAttachment::Goal("GOAL1".to_string()),
            Some("smoke-ai"),
            Some("goal"),
        )
        .unwrap();

    let prompt = service.chat_prompt(&session, "What changed?");
    assert!(prompt.contains("Context:"));
    assert!(prompt.contains("\"id\": \"GOAL1\""));
    assert!(prompt.contains("\"name\": \"Checkout fails\""));
    assert!(prompt.contains("What changed?"));

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn file_chat_service_ignores_orphaned_operations_for_purged_supervisor_sessions() {
    let temp_root = unique_temp_dir("chat-purged-supervisor-recovery");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_record_with_options(
            ChatAttachment::Supervisor,
            Some("smoke-ai"),
            Some("supervisor"),
        )
        .unwrap();
    let registry = FileOperationRegistry::new(&service.runtime_root);
    let operation = registry.register(&format!("chat:{}", session.id)).unwrap();

    assert_eq!(service.purge_supervisor_sessions().unwrap(), 1);
    assert!(
        service
            .recover_interrupted_turns("daemon restarted during provider turn")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        registry.status(&operation.id).unwrap().state,
        OperationState::Interrupted
    );

    fs::remove_dir_all(temp_root).unwrap();
}

use super::*;

#[test]
fn file_chat_service_edits_removes_and_batches_queued_messages() {
    let temp_root = unique_temp_dir("chat-queue");
    init_git_app(&temp_root);
    let refine_dir = temp_root.join(".refine");
    write_fake_provider(&refine_dir, "smoke-ai", 0, "queued provider response");
    let service = FileChatService::new(&refine_dir);
    let session = service
        .start_with_options(ChatAttachment::Standalone, Some("smoke-ai"), Some("chat"))
        .unwrap();
    let mut busy = service.resume(&session.id).unwrap();
    busy.queue_dispatching = true;
    service.write_record(&busy).unwrap();

    let queued = service.append_user_message(&session.id, "first").unwrap();
    let first_id = queued.queued_messages[0].id.clone();
    let queued = service.append_user_message(&session.id, "second").unwrap();
    let second_id = queued.queued_messages[1].id.clone();
    service
        .update_queued_message(&session.id, &first_id, "first edited")
        .unwrap();
    service
        .remove_queued_message(&session.id, &second_id)
        .unwrap();
    service.append_user_message(&session.id, "third").unwrap();

    let mut ready = service.resume(&session.id).unwrap();
    assert_eq!(ready.queued_messages.len(), 2);
    ready.queue_dispatching = false;
    service.write_record(&ready).unwrap();
    service.ensure_queue_dispatch(&mut ready).unwrap();
    wait_for_chat_line(&service, &session.id, "queued provider response");
    let record = service.resume(&session.id).unwrap();
    let user_events = record
        .transcript_events
        .iter()
        .filter(|event| event.get("role").and_then(|value| value.as_str()) == Some("user"))
        .collect::<Vec<_>>();
    assert_eq!(user_events.len(), 1);
    let user_text = user_events[0]
        .get("text")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    assert!(user_text.contains("first edited"));
    assert!(user_text.contains("third"));
    assert!(!user_text.contains("second"));
    assert!(record.queued_messages.is_empty());

    fs::remove_dir_all(temp_root).unwrap();
}

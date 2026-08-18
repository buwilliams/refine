use super::{ChatAttachment, ChatQueuedMessage};

pub(super) fn combined_queued_message(messages: &[ChatQueuedMessage]) -> String {
    if messages.len() == 1 {
        return messages[0].text.clone();
    }
    messages
        .iter()
        .enumerate()
        .map(|(idx, message)| format!("Message {}:\n{}", idx + 1, message.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn is_internal_queued_message(
    attachment: &ChatAttachment,
    message: &ChatQueuedMessage,
) -> bool {
    message.internal
        || (matches!(attachment, ChatAttachment::Supervisor)
            && message.text.starts_with(
                "Supervise until the queue is idle using the evidence and Refine's tools.",
            ))
}

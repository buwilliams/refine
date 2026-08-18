use mail_parser::{MessageParser, MimeHeaders};

use crate::error::{RefineError, RefineResult};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ParsedEmail {
    pub(super) message_id: Option<String>,
    pub(super) sender: String,
    pub(super) subject: String,
    pub(super) source_text: String,
}

pub(super) fn parse_email(raw: &[u8]) -> RefineResult<ParsedEmail> {
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| RefineError::Serialization("failed to parse RFC 5322 email".to_string()))?;
    let from = message
        .from()
        .and_then(|addresses| addresses.first())
        .ok_or_else(|| RefineError::InvalidInput("email has no From address".to_string()))?;
    let sender = from
        .address
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| RefineError::InvalidInput("email has no From address".to_string()))?;
    let from_source = from
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|name| format!("{name} <{sender}>"))
        .unwrap_or_else(|| sender.clone());
    let subject = message.subject().unwrap_or_default().trim().to_string();
    let body = message
        .body_text(0)
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let mut source = format!("From: {from_source}\nSubject: {subject}\n\nBody:\n{body}");

    for attachment in message.attachments() {
        let Some(name) = attachment
            .attachment_name()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let is_text = attachment
            .content_type()
            .is_some_and(|content_type| content_type.c_type.eq_ignore_ascii_case("text"))
            || name.to_ascii_lowercase().ends_with(".txt");
        if !is_text {
            continue;
        }
        let Some(text) = attachment
            .text_contents()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        source.push_str(&format!("\n\nText attachment: {name}\n{text}"));
    }

    Ok(ParsedEmail {
        message_id: message.message_id().map(str::to_string),
        sender,
        subject,
        source_text: source,
    })
}

pub(super) fn source_is_authoritative(record: &super::DevelopmentRequestRecord) -> bool {
    let mut lines = record.source_text.lines();
    let from = lines
        .next()
        .and_then(|line| line.strip_prefix("From: "))
        .unwrap_or_default();
    let subject = lines.next() == Some(format!("Subject: {}", record.subject).as_str());
    (from == record.sender || from.ends_with(&format!("<{}>", record.sender)))
        && subject
        && record.source_text.contains("\n\nBody:\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_source_preserves_named_text_parts_and_ignores_other_bodies() {
        let raw = concat!(
            "From: Buddy <Buddy@example.com>\r\n",
            "Subject: Add a useful feature\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=x\r\n\r\n",
            "--x\r\nContent-Type: text/plain\r\n\r\nPlease add it.\r\n",
            "--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=first.txt\r\n\r\nFirst.\r\n",
            "--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=second.txt\r\n\r\nSecond.\r\n",
            "--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment\r\n\r\nUnnamed.\r\n",
            "--x\r\nContent-Type: image/png\r\nContent-Disposition: attachment; filename=screen.png\r\n\r\nPNGDATA\r\n",
            "--x--\r\n"
        );
        let parsed = parse_email(raw.as_bytes()).unwrap();
        assert_eq!(parsed.sender, "buddy@example.com");
        assert_eq!(parsed.subject, "Add a useful feature");
        assert_eq!(
            parsed.source_text,
            concat!(
                "From: Buddy <buddy@example.com>\nSubject: Add a useful feature\n\n",
                "Body:\nPlease add it.\n\n",
                "Text attachment: first.txt\nFirst.\n\n",
                "Text attachment: second.txt\nSecond."
            )
        );
        assert!(!parsed.source_text.contains("Unnamed"));
        assert!(!parsed.source_text.contains("PNGDATA"));
    }

    #[test]
    fn html_only_and_empty_fields_are_explicit() {
        let html = parse_email(
            concat!(
                "From: buddy@example.com\r\nSubject: HTML\r\n",
                "Content-Type: text/html; charset=utf-8\r\n\r\n<p>Please <b>add</b> it.</p>"
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            html.source_text,
            "From: buddy@example.com\nSubject: HTML\n\nBody:\nPlease add it."
        );

        let empty = parse_email(b"From: buddy@example.com\r\nSubject:   \r\n\r\n").unwrap();
        assert_eq!(
            empty.source_text,
            "From: buddy@example.com\nSubject: \n\nBody:\n"
        );
    }
}

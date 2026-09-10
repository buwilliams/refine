use super::*;
use std::cell::{Cell, RefCell};

struct IntakeMail<'a> {
    service: &'a FileDevelopmentRequestService,
    messages: Vec<(&'static str, Vec<u8>)>,
    acknowledgements: RefCell<Vec<(String, Option<DevelopmentRequestRecord>)>>,
    fail_acknowledgement: Cell<bool>,
}

impl<'a> IntakeMail<'a> {
    fn new(service: &'a FileDevelopmentRequestService, senders: &[(&'static str, &str)]) -> Self {
        Self {
            service,
            messages: senders
                .iter()
                .map(|(id, sender)| {
                    (*id, format!(
                        "From: {sender}\r\nSubject: Request {id}\r\nContent-Type: text/plain\r\n\r\nImplement {id}."
                    ).into_bytes())
                })
                .collect(),
            acknowledgements: RefCell::new(Vec::new()),
            fail_acknowledgement: Cell::new(false),
        }
    }
}

impl MailSource for IntakeMail<'_> {
    fn pending_email_ids(&self, address: &str) -> RefineResult<Vec<String>> {
        assert_eq!(address, settings().address);
        Ok(self.messages.iter().map(|(id, _)| id.to_string()).collect())
    }

    fn raw_email(&self, email_id: &str) -> RefineResult<Vec<u8>> {
        self.messages
            .iter()
            .find(|(id, _)| *id == email_id)
            .map(|(_, raw)| raw.clone())
            .ok_or_else(|| RefineError::NotFound(format!("raw email {email_id} is unavailable")))
    }

    fn mark_processed(&self, email_id: &str) -> RefineResult<()> {
        let path = self.service.record_path(&request_id(email_id));
        let durable_record = path
            .exists()
            .then(|| self.service.read_record(&path).unwrap());
        self.acknowledgements
            .borrow_mut()
            .push((email_id.to_string(), durable_record));
        if self.fail_acknowledgement.replace(false) {
            return Err(RefineError::Io("remote acknowledgement interrupted".into()));
        }
        Ok(())
    }
}

fn fixture() -> (PathBuf, FileDevelopmentRequestService) {
    let root = std::env::temp_dir().join(format!("refine-email-intake-{}", uuid::Uuid::new_v4()));
    let service = FileDevelopmentRequestService::new(
        root.join("runtime"),
        root.join("state"),
        root.join("target"),
    );
    (root, service)
}

fn assert_backlog_goal(service: &FileDevelopmentRequestService, record: &DevelopmentRequestRecord) {
    let detail = FileWorkItemService::new(&service.refine_dir)
        .show_goal_detail(&record.id)
        .unwrap();
    assert_eq!(detail["id"], record.id);
    assert_eq!(detail["status"], "backlog");
    assert_eq!(detail["priority"], "low");
    assert_eq!(detail["reporter"], record.sender);
    assert_eq!(detail["assignee"], record.sender);
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(detail["rounds"][0]["prompt"], record.source_text);
}

#[test]
fn ingest_filters_senders_and_records_accepted_source_before_remote_acknowledgement() {
    let (root, service) = fixture();
    let mail = IntakeMail::new(
        &service,
        &[
            ("trusted", "Buddy <BUDDY@example.com>"),
            ("untrusted", "stranger@example.com"),
        ],
    );

    assert_eq!(service.ingest(&mail, &settings()).unwrap(), 2);
    let acknowledgements = mail.acknowledgements.borrow();
    assert_eq!(acknowledgements.len(), 2);
    assert_eq!(acknowledgements[0].0, "trusted");
    let record = acknowledgements[0].1.as_ref().unwrap();
    assert_eq!(record.status, DevelopmentRequestStatus::Received);
    assert_eq!(record.sender, "buddy@example.com");
    assert!(record.source_text.contains("Implement trusted."));
    assert_eq!(acknowledgements[1], ("untrusted".to_string(), None));
    assert_eq!(service.record_paths().unwrap().len(), 1);

    let result = service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(
        result,
        json!({"goal_ids": [record.id], "errors": [], "batch_limit": 25})
    );
    assert_backlog_goal(&service, record);
    let persisted = service
        .read_record(&service.record_path(&record.id))
        .unwrap();
    assert_eq!(persisted.review_seen_at, None);
    assert_eq!(persisted.notified_at, None);
    assert_eq!(persisted.status, DevelopmentRequestStatus::GoalCreated);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_durable_record_write_prevents_acknowledgement_and_can_be_retried() {
    let (root, service) = fixture();
    let mail = IntakeMail::new(&service, &[("write-retry", "buddy@example.com")]);
    service.fail_next_record_write.set(true);

    let error = service.ingest(&mail, &settings()).unwrap_err();
    assert!(error.to_string().contains("record write interruption"));
    assert!(mail.acknowledgements.borrow().is_empty());
    assert!(service.record_paths().unwrap().is_empty());

    assert_eq!(service.ingest(&mail, &settings()).unwrap(), 1);
    assert!(mail.acknowledgements.borrow()[0].1.is_some());
    let result = service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(result["goal_ids"], json!([request_id("write-retry")]));
    assert_eq!(result["errors"], json!([]));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_remote_acknowledgement_retains_source_and_repeated_fetches_do_not_duplicate() {
    let (root, service) = fixture();
    let mail = IntakeMail::new(&service, &[("ack-retry", "buddy@example.com")]);
    mail.fail_acknowledgement.set(true);

    let error = service.ingest(&mail, &settings()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("remote acknowledgement interrupted")
    );
    let path = service.record_path(&request_id("ack-retry"));
    let before = fs::read(&path).unwrap();
    let received = service.read_record(&path).unwrap();
    assert_eq!(received.status, DevelopmentRequestStatus::Received);
    assert_eq!(
        mail.acknowledgements.borrow()[0].1.as_ref(),
        Some(&received)
    );

    assert_eq!(service.ingest(&mail, &settings()).unwrap(), 1);
    assert_eq!(fs::read(&path).unwrap(), before);
    let result = service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(result["goal_ids"], json!([received.id]));
    let linked = fs::read(&path).unwrap();
    assert_eq!(service.ingest(&mail, &settings()).unwrap(), 1);
    let repeated = service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(
        repeated,
        json!({"goal_ids": [], "errors": [], "batch_limit": 25})
    );
    assert_eq!(fs::read(&path).unwrap(), linked);
    assert_eq!(
        FileWorkItemService::new(&service.refine_dir)
            .list_goal_summaries()
            .unwrap()
            .len(),
        1
    );
    assert_backlog_goal(&service, &received);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn per_record_failure_retains_successful_imports_and_durable_retry_evidence() {
    let (root, service) = fixture();
    let mail = IntakeMail::new(&service, &[("valid-batch", "buddy@example.com")]);
    let mut legacy = service.record_from_email(
        "unavailable-legacy",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".into(),
            subject: "Legacy".into(),
            source_text: "retained legacy source".into(),
        },
        &settings().address,
    );
    legacy.schema_version = 1;
    service.write_record(&legacy).unwrap();
    assert_eq!(service.ingest(&mail, &settings()).unwrap(), 1);

    let result = service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(result["goal_ids"], json!([request_id("valid-batch")]));
    assert_eq!(result["batch_limit"], 25);
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].as_str().unwrap().contains(&legacy.id));
    assert!(errors[0].as_str().unwrap().contains("unavailable"));
    let retained = service
        .read_record(&service.record_path(&legacy.id))
        .unwrap();
    assert_eq!(retained.status, DevelopmentRequestStatus::Received);
    assert_eq!(retained.schema_version, 1);
    assert_eq!(retained.source_text, legacy.source_text);
    assert_eq!(retained.attempts, 1);
    assert!(
        retained
            .last_error
            .as_ref()
            .unwrap()
            .contains("unavailable")
    );
    let valid = service
        .read_record(&service.record_path(&request_id("valid-batch")))
        .unwrap();
    assert_backlog_goal(&service, &valid);

    let retried = service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(retried["goal_ids"], json!([]));
    assert_eq!(retried["errors"].as_array().unwrap().len(), 1);
    assert_eq!(
        service
            .read_record(&service.record_path(&legacy.id))
            .unwrap()
            .attempts,
        2
    );
    assert_eq!(
        FileWorkItemService::new(&service.refine_dir)
            .list_goal_summaries()
            .unwrap()
            .len(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn interrupted_goal_link_is_recovered_by_deterministic_identity() {
    let (root, service) = fixture();
    let mail = IntakeMail::new(&service, &[("link-retry", "buddy@example.com")]);
    service.ingest(&mail, &settings()).unwrap();
    let path = service.record_path(&request_id("link-retry"));
    let before = fs::read(&path).unwrap();
    let mut record = service.read_record(&path).unwrap();
    service.fail_next_record_write.set(true);

    let error = service
        .recover_or_create_goal(&mut record, &mail, &settings())
        .unwrap_err();
    assert!(error.to_string().contains("record write interruption"));
    assert_eq!(fs::read(&path).unwrap(), before);
    let work_items = FileWorkItemService::new(&service.refine_dir);
    let original = work_items.show_goal_detail(&record.id).unwrap();
    let result = service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(
        result,
        json!({"goal_ids": [record.id], "errors": [], "batch_limit": 25})
    );
    assert_eq!(work_items.show_goal_detail(&record.id).unwrap(), original);
    assert_eq!(work_items.list_goal_summaries().unwrap().len(), 1);
    assert_eq!(
        service.read_record(&path).unwrap().status,
        DevelopmentRequestStatus::GoalCreated
    );
    assert_backlog_goal(&service, &record);
    fs::remove_dir_all(root).unwrap();
}

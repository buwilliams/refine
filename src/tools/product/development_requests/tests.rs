use super::*;
use std::cell::Cell;

struct FakeMail {
    raw: Option<Vec<u8>>,
    notifications: Cell<usize>,
}

impl MailSource for FakeMail {
    fn pending_email_ids(&self, _address: &str) -> RefineResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn raw_email(&self, email_id: &str) -> RefineResult<Vec<u8>> {
        self.raw
            .clone()
            .ok_or_else(|| RefineError::NotFound(format!("raw email {email_id} is unavailable")))
    }

    fn mark_processed(&self, _email_id: &str) -> RefineResult<()> {
        Ok(())
    }

    fn send_resolution(
        &self,
        _settings: &DevelopmentRequestSettings,
        _record: &DevelopmentRequestRecord,
    ) -> RefineResult<()> {
        self.notifications.set(self.notifications.get() + 1);
        Ok(())
    }
}

fn settings() -> DevelopmentRequestSettings {
    DevelopmentRequestSettings {
        address: "goal@getrefine.dev".to_string(),
        allowed_senders: BTreeSet::from(["buddy@example.com".to_string()]),
        auto_approve_after: Duration::ZERO,
    }
}

fn write_config(runtime_root: &Path, target_root: &Path, allowed_senders: &[&str]) {
    fs::create_dir_all(runtime_root).unwrap();
    fs::write(
        self_development_email_config_path(runtime_root),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "target_root": target_root,
            "address": " Goal@GetRefine.dev ",
            "allowed_senders": allowed_senders,
            "poll_seconds": 0,
            "auto_approve_after_seconds": 5
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn absent_local_contract_disables_email_intake() {
    let runtime_root = std::env::temp_dir().join(format!(
        "refine-development-request-config-{}",
        uuid::Uuid::new_v4()
    ));
    assert_eq!(
        load_self_development_email_config(&runtime_root).unwrap(),
        None
    );
}

#[test]
fn local_contract_is_normalized_reread_and_bound_to_one_target() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-config-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime_root = root.join("run/8082");
    let target_root = root.join("refine-next");
    let other_target = root.join("production-app");
    fs::create_dir_all(&target_root).unwrap();
    fs::create_dir_all(&other_target).unwrap();
    write_config(
        &runtime_root,
        &target_root,
        &[" Buddy@Example.com ", "BUDDY@example.com"],
    );

    let config = load_self_development_email_config(&runtime_root)
        .unwrap()
        .unwrap();
    assert_eq!(config.target_root, target_root.canonicalize().unwrap());
    assert_eq!(config.address, "goal@getrefine.dev");
    assert_eq!(config.poll_seconds, 1);
    assert_eq!(
        config.allowed_senders,
        BTreeSet::from(["buddy@example.com".to_string()])
    );
    assert!(self_development_email_target_is_active(&config, &target_root).unwrap());
    assert!(!self_development_email_target_is_active(&config, &other_target).unwrap());

    write_config(
        &runtime_root,
        &target_root,
        &["second@example.com", "THIRD@example.com"],
    );
    let updated = load_self_development_email_config(&runtime_root)
        .unwrap()
        .unwrap();
    assert_eq!(
        updated.allowed_senders,
        BTreeSet::from([
            "second@example.com".to_string(),
            "third@example.com".to_string()
        ])
    );
    let settings = DevelopmentRequestSettings::from_local_config(&updated);
    assert_eq!(settings.auto_approve_after, Duration::from_secs(5));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_agent_cli_config_is_tolerated_but_inactive_and_omitted() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-legacy-config-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime_root = root.join("run/8082");
    let target_root = root.join("target");
    fs::create_dir_all(&runtime_root).unwrap();
    fs::create_dir_all(&target_root).unwrap();
    fs::write(
        self_development_email_config_path(&runtime_root),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "target_root": target_root,
            "address": "goal@getrefine.dev",
            "allowed_senders": ["buddy@example.com"],
            "agent_cli": "obsolete-provider"
        }))
        .unwrap(),
    )
    .unwrap();
    let config = load_self_development_email_config(&runtime_root)
        .unwrap()
        .unwrap();
    assert!(
        serde_json::to_value(config)
            .unwrap()
            .get("agent_cli")
            .is_none()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_contract_rejects_a_relative_target() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-config-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime_root = root.join("run/8082");
    fs::create_dir_all(&runtime_root).unwrap();
    fs::write(
        self_development_email_config_path(&runtime_root),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "target_root": "../refine-next",
            "address": "goal@getrefine.dev",
            "allowed_senders": ["buddy@example.com"]
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(load_self_development_email_config(&runtime_root).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn mime_extraction_keeps_body_and_text_attachments_only() {
    let raw = concat!(
        "From: Buddy <Buddy@example.com>\r\n",
        "To: goal@getrefine.dev\r\n",
        "Subject: Add a useful feature\r\n",
        "Message-ID: <source@example.com>\r\n",
        "MIME-Version: 1.0\r\n",
        "Content-Type: multipart/mixed; boundary=x\r\n\r\n",
        "--x\r\nContent-Type: text/plain\r\n\r\nPlease add the feature.\r\n",
        "--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=request.txt\r\n\r\nAcceptance details.\r\n",
        "--x\r\nContent-Type: image/png\r\nContent-Disposition: attachment; filename=screen.png\r\n\r\nPNGDATA\r\n",
        "--x\r\nContent-Type: application/json\r\nContent-Disposition: attachment; filename=generated.json\r\n\r\n{}\r\n",
        "--x--\r\n"
    );
    let parsed = parse_email(raw.as_bytes()).unwrap();
    assert_eq!(parsed.sender, "buddy@example.com");
    assert!(parsed.source_text.contains("Please add the feature."));
    assert!(parsed.source_text.contains("Acceptance details."));
    assert!(!parsed.source_text.contains("PNGDATA"));
    assert!(!parsed.source_text.contains("generated.json"));
}

#[test]
fn request_identity_is_stable_and_goal_compatible() {
    let id = request_id("fastmail-email-id");
    assert_eq!(id, request_id("fastmail-email-id"));
    assert!(id.starts_with("DR"));
    assert_eq!(id.len(), 26);
}

#[test]
fn local_request_record_round_trips_as_the_durable_retry_queue() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-{}",
        uuid::Uuid::new_v4()
    ));
    let service = FileDevelopmentRequestService::new(
        root.join("run/8082"),
        root.join("refine-live-state"),
        root.join("target"),
    );
    let record = service.record_from_email(
        "provider-id",
        ParsedEmail {
            message_id: Some("source@example.com".to_string()),
            sender: "buddy@example.com".to_string(),
            subject: "Request".to_string(),
            source_text:
                "From: buddy@example.com\nSubject: Request\n\nBody:\nPlease implement this."
                    .to_string(),
        },
        "goal@getrefine.dev",
    );
    service.write_record(&record).unwrap();
    assert_eq!(
        service
            .read_record(&service.record_path(&record.id))
            .unwrap(),
        record
    );
    assert!(
        service
            .record_path(&record.id)
            .starts_with(root.join("run/8082/self-development-email/requests"))
    );
    assert!(!root.join("refine-live-state/development-requests").exists());
    assert!(record.notification_message_id.ends_with("@getrefine.dev"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn jmap_patch_uses_the_provider_email_id_as_the_dynamic_object_key() {
    let email_id = "fastmail-id";
    let patch = json!({
        "update": {email_id: {format!("keywords/{PROCESSED_KEYWORD}"): true}}
    });
    assert_eq!(
        patch["update"][email_id][format!("keywords/{PROCESSED_KEYWORD}")],
        true
    );
    assert!(patch["update"].get("email_id").is_none());
}

#[test]
fn pending_query_selects_the_recipient_address_without_a_mailbox() {
    let filter = pending_email_filter("goal@getrefine.dev");
    assert_eq!(filter["to"], "goal@getrefine.dev");
    assert_eq!(filter["notKeyword"], PROCESSED_KEYWORD);
    assert!(filter.get("inMailbox").is_none());
}

#[test]
fn email_goal_priority_and_source_are_capability_owned() {
    let service = FileDevelopmentRequestService::new("runtime", "state", "target");
    let record = service.record_from_email(
        "provider-id",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".to_string(),
            subject: "Request".to_string(),
            source_text:
                "From: buddy@example.com\nSubject: Request\n\nBody:\nPlease implement this."
                    .to_string(),
        },
        "goal@getrefine.dev",
    );
    let request = development_request_goal_authoring_request(&record, "Request".to_string());
    assert_eq!(request.priority, "low");
    assert_eq!(request.prompt, record.source_text);
}

#[test]
fn schema_one_received_is_upgraded_from_raw_mail_before_goal_authoring() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-migrate-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime_root = root.join("run/8082");
    let refine_dir = root.join("refine-live-state");
    let target_root = root.join("target");
    fs::create_dir_all(&target_root).unwrap();
    let service = FileDevelopmentRequestService::new(&runtime_root, &refine_dir, &target_root);
    let mut record = service.record_from_email(
        "legacy-provider-id",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".to_string(),
            subject: "Legacy".to_string(),
            source_text: "legacy body only".to_string(),
        },
        "goal@getrefine.dev",
    );
    record.schema_version = 1;
    service.write_record(&record).unwrap();
    let mail = FakeMail {
        raw: Some(concat!(
            "From: Buddy <buddy@example.com>\r\nSubject: Migrated request\r\n",
            "MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n",
            "--x\r\nContent-Type: text/plain\r\n\r\nBody source.\r\n",
            "--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=one.txt\r\n\r\nOne.\r\n",
            "--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=two.txt\r\n\r\nTwo.\r\n",
            "--x--\r\n"
        ).as_bytes().to_vec()),
        notifications: Cell::new(0),
    };
    service
        .migrate_received_record(&mut record, &mail, &settings())
        .unwrap();
    let migrated = service
        .read_record(&service.record_path(&record.id))
        .unwrap();
    assert_eq!(migrated.schema_version, 2);
    assert!(
        migrated
            .source_text
            .contains("From: Buddy <buddy@example.com>")
    );
    assert!(
        migrated
            .source_text
            .contains("Text attachment: one.txt\nOne.")
    );
    assert!(
        migrated
            .source_text
            .contains("Text attachment: two.txt\nTwo.")
    );
    service
        .recover_or_create_goal(&mut record, &mail, &settings())
        .unwrap();
    let detail = FileWorkItemService::new(&refine_dir)
        .show_goal_detail(&record.id)
        .unwrap();
    assert_eq!(detail["priority"], "low");
    assert_eq!(detail["reporter"], "buddy@example.com");
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(detail["rounds"][0]["prompt"], migrated.source_text);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_schema_one_raw_mail_is_retryable_and_creates_no_goal() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-unavailable-{}",
        uuid::Uuid::new_v4()
    ));
    let service = FileDevelopmentRequestService::new(
        root.join("run/8082"),
        root.join("state"),
        root.join("target"),
    );
    let mut record = service.record_from_email(
        "missing-provider-id",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".to_string(),
            subject: "Legacy".to_string(),
            source_text: "legacy body only".to_string(),
        },
        "goal@getrefine.dev",
    );
    record.schema_version = 1;
    service.write_record(&record).unwrap();
    let mail = FakeMail {
        raw: None,
        notifications: Cell::new(0),
    };
    service.process_local_records(&mail, &settings()).unwrap();
    let retried = service
        .read_record(&service.record_path(&record.id))
        .unwrap();
    assert_eq!(retried.schema_version, 1);
    assert_eq!(retried.source_text, "legacy body only");
    assert_eq!(retried.attempts, 1);
    assert!(retried.last_error.unwrap().contains("unavailable"));
    assert!(
        FileWorkItemService::new(root.join("state"))
            .list_goal_summaries()
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn interrupted_schema_one_migration_write_retains_retryable_legacy_record() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-migration-write-{}",
        uuid::Uuid::new_v4()
    ));
    let service = FileDevelopmentRequestService::new(
        root.join("run/8082"),
        root.join("state"),
        root.join("target"),
    );
    let mut record = service.record_from_email(
        "migration-write-id",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".to_string(),
            subject: "Legacy".to_string(),
            source_text: "legacy body only".to_string(),
        },
        "goal@getrefine.dev",
    );
    record.schema_version = 1;
    service.write_record(&record).unwrap();
    let mail = FakeMail {
        raw: Some(
            concat!(
                "From: buddy@example.com\r\nSubject: Complete source\r\n",
                "Content-Type: text/plain\r\n\r\nComplete body"
            )
            .as_bytes()
            .to_vec(),
        ),
        notifications: Cell::new(0),
    };
    service.fail_next_record_write.set(true);
    service.process_local_records(&mail, &settings()).unwrap();
    let retried = service
        .read_record(&service.record_path(&record.id))
        .unwrap();
    assert_eq!(retried.schema_version, 1);
    assert_eq!(retried.source_text, "legacy body only");
    assert_eq!(retried.attempts, 1);
    assert!(retried.last_error.unwrap().contains("write interruption"));
    assert!(
        FileWorkItemService::new(root.join("state"))
            .list_goal_summaries()
            .unwrap()
            .is_empty()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn schema_one_received_links_an_existing_legacy_goal_without_raw_mail_or_rewrite() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-legacy-link-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime_root = root.join("run/8082");
    let refine_dir = root.join("state");
    let target_root = root.join("target");
    fs::create_dir_all(&target_root).unwrap();
    let service = FileDevelopmentRequestService::new(&runtime_root, &refine_dir, &target_root);
    let mut record = service.record_from_email(
        "legacy-linked-id",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".to_string(),
            subject: "Legacy subject".to_string(),
            source_text: "legacy body only".to_string(),
        },
        "goal@getrefine.dev",
    );
    record.schema_version = 1;
    service.write_record(&record).unwrap();
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .author_goal(GoalAuthoringRequest {
            id: Some(record.id.clone()),
            name: Some("Reviewer-authored historical name".to_string()),
            prompt: "Reviewer-authored historical Round".to_string(),
            reporter: "Legacy reviewer".to_string(),
            priority: "medium".to_string(),
            duplicate_decision: "original".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap();
    let goal_path = work_items
        .show_goal_summary(&record.id)
        .unwrap()
        .goal
        .json_path;
    let goal_path = refine_dir.join(goal_path);
    let before = fs::read(&goal_path).unwrap();
    let mail = FakeMail {
        raw: None,
        notifications: Cell::new(0),
    };
    service
        .recover_or_create_goal(&mut record, &mail, &settings())
        .unwrap();
    assert_eq!(fs::read(&goal_path).unwrap(), before);
    assert_eq!(record.status, DevelopmentRequestStatus::GoalCreated);
    assert_eq!(
        record.goal_name.as_deref(),
        Some("Reviewer-authored historical name")
    );
    assert_eq!(work_items.list_goal_summaries().unwrap().len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unsupported_record_is_unchanged_and_does_not_starve_later_valid_record() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-isolation-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime_root = root.join("run/8082");
    let refine_dir = root.join("state");
    let service =
        FileDevelopmentRequestService::new(&runtime_root, &refine_dir, root.join("target"));
    let bad_path = runtime_root.join("self-development-email/requests/000-bad/request.json");
    fs::create_dir_all(bad_path.parent().unwrap()).unwrap();
    let bad_bytes = br#"{"schema_version":99,"provider_email_id":"future"}"#.to_vec();
    fs::write(&bad_path, &bad_bytes).unwrap();
    let valid = service.record_from_email(
        "valid-provider-id",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".to_string(),
            subject: "Valid".to_string(),
            source_text: "From: buddy@example.com\nSubject: Valid\n\nBody:\nContinue".to_string(),
        },
        "goal@getrefine.dev",
    );
    service.write_record(&valid).unwrap();
    let mail = FakeMail {
        raw: None,
        notifications: Cell::new(0),
    };
    service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(fs::read(&bad_path).unwrap(), bad_bytes);
    assert_eq!(
        service
            .read_record(&service.record_path(&valid.id))
            .unwrap()
            .status,
        DevelopmentRequestStatus::GoalCreated
    );
    assert_eq!(
        FileWorkItemService::new(refine_dir)
            .list_goal_summaries()
            .unwrap()
            .len(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn linked_and_terminal_schema_one_records_retry_without_duplication() {
    let root = std::env::temp_dir().join(format!(
        "refine-development-request-legacy-states-{}",
        uuid::Uuid::new_v4()
    ));
    let runtime_root = root.join("run/8082");
    let refine_dir = root.join("state");
    let service =
        FileDevelopmentRequestService::new(&runtime_root, &refine_dir, root.join("target"));
    let work_items = FileWorkItemService::new(&refine_dir);
    let mut linked = service.record_from_email(
        "linked-state",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".to_string(),
            subject: "Linked".to_string(),
            source_text: "legacy body".to_string(),
        },
        "goal@getrefine.dev",
    );
    linked.schema_version = 1;
    work_items
        .author_goal(GoalAuthoringRequest {
            id: Some(linked.id.clone()),
            name: Some("Historical".to_string()),
            prompt: "Historical Round".to_string(),
            reporter: "Reviewer".to_string(),
            priority: "low".to_string(),
            duplicate_decision: "original".to_string(),
            ..GoalAuthoringRequest::default()
        })
        .unwrap();
    linked.goal_id = Some(linked.id.clone());
    linked.goal_name = Some("Historical".to_string());
    linked.status = DevelopmentRequestStatus::GoalCreated;
    service.write_record(&linked).unwrap();

    let mut terminal = service.record_from_email(
        "terminal-state",
        ParsedEmail {
            message_id: None,
            sender: "buddy@example.com".to_string(),
            subject: "Terminal".to_string(),
            source_text: "legacy body".to_string(),
        },
        "goal@getrefine.dev",
    );
    terminal.schema_version = 1;
    terminal.goal_id = Some(terminal.id.clone());
    terminal.goal_name = Some("Historical terminal".to_string());
    terminal.status = DevelopmentRequestStatus::Notified;
    terminal.notified_at = Some(Utc::now().to_rfc3339());
    service.write_record(&terminal).unwrap();
    let terminal_path = service.record_path(&terminal.id);
    let terminal_before = fs::read(&terminal_path).unwrap();
    let mail = FakeMail {
        raw: None,
        notifications: Cell::new(0),
    };
    service.process_local_records(&mail, &settings()).unwrap();
    service.process_local_records(&mail, &settings()).unwrap();
    assert_eq!(work_items.list_goal_summaries().unwrap().len(), 1);
    assert_eq!(fs::read(terminal_path).unwrap(), terminal_before);
    assert_eq!(mail.notifications.get(), 0);
    fs::remove_dir_all(root).unwrap();
}

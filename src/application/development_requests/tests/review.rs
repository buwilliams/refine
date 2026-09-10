use super::*;

use crate::application::fleet::nodes::FileNodeRegistryService;
use crate::application::workflow::governance::integration::FileGovernanceIntegrationService;
use crate::infrastructure::process::supervisor::config::FileSettingsService;
use crate::model::workflow::GoalStatus;

struct ReviewFixture {
    root: PathBuf,
    service: FileDevelopmentRequestService,
    work_items: FileWorkItemService,
    record_id: String,
    mail: FakeMail,
}

impl ReviewFixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("refine-email-review-{}", uuid::Uuid::new_v4()));
        let runtime = root.join("run/8082");
        let state = root.join("state");
        let target = root.join("target");
        fs::create_dir_all(&target).unwrap();
        git(&target, &["init", "-b", "main"]);
        git(
            &target,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "integrated candidate",
            ],
        );
        let candidate = git(&target, &["rev-parse", "HEAD"]);
        let nodes = FileNodeRegistryService::with_active_root(&state, &runtime);
        nodes.create("worker").unwrap();
        nodes.activate("worker").unwrap();
        let service = FileDevelopmentRequestService::new(&runtime, &state, &target);
        let work_items =
            FileWorkItemService::with_projection_cache(&state, &runtime, runtime.join("cache"));
        let mail = FakeMail {
            raw: None,
            notifications: Cell::new(0),
        };
        let mut record = service.record_from_email(
            "review-request",
            parse_email(
                concat!(
                    "From: buddy@example.com\r\n",
                    "Subject: Review request\r\n\r\n",
                    "Test Review acceptance\r\n",
                )
                .as_bytes(),
            )
            .unwrap(),
            "goal@getrefine.dev",
        );
        service
            .recover_or_create_goal(&mut record, &mail, &settings())
            .unwrap();
        let id = &record.id;
        work_items
            .transition_goal_status(id, GoalStatus::Todo)
            .unwrap();
        work_items
            .advance_automated_goal_status(id, GoalStatus::Plan)
            .unwrap();
        work_items
            .update_goal_git_refs(id, "main", "main", &candidate, Some(&candidate))
            .unwrap();
        // Seed an already-integrated Review candidate backed by a real local commit.
        work_items.update_goal_round_evaluation_summary(id, 0, &json!({
            "workflow_integration": {
                "candidate_commit": candidate, "target_branch": "main", "target_commit": candidate,
                "remote": "origin", "pushed": false, "integrated_at": Utc::now().to_rfc3339(),
                "merge": {"ok": true, "conflicts": [], "message": null}
            }
        })).unwrap();
        for status in [
            GoalStatus::Implement,
            GoalStatus::Quality,
            GoalStatus::Governance,
            GoalStatus::Review,
        ] {
            work_items
                .advance_automated_goal_status(id, status)
                .unwrap();
        }
        Self {
            root,
            service,
            work_items,
            record_id: record.id,
            mail,
        }
    }

    fn poll(&self, delay: u64) {
        let mut config = settings();
        config.auto_approve_after = Duration::from_secs(delay);
        self.service
            .process_local_records(&self.mail, &config)
            .unwrap();
    }

    fn record(&self) -> DevelopmentRequestRecord {
        self.service
            .read_record(&self.service.record_path(&self.record_id))
            .unwrap()
    }

    fn status(&self) -> GoalStatus {
        self.work_items
            .show_goal_summary(&self.record_id)
            .unwrap()
            .goal
            .status
    }

    fn enable(&self, enabled: bool) {
        FileSettingsService::for_node(&self.service.refine_dir, "worker")
            .update(&json!({"auto_approve": enabled}))
            .unwrap();
    }

    fn corrupt_setting(&self) {
        let nodes = FileNodeRegistryService::new(&self.service.refine_dir);
        let mut registry = nodes.load_registry().unwrap();
        registry
            .nodes
            .iter_mut()
            .find(|node| node.id == "worker")
            .unwrap()
            .settings
            .insert("auto_approve".to_string(), json!("invalid"));
        nodes.save_registry(&registry).unwrap();
    }

    fn approve_manually(&self) {
        FileGovernanceIntegrationService::with_target_root(
            &self.service.runtime_root,
            &self.service.refine_dir,
            &self.service.target_root,
        )
        .approve_reviewed_goal(&self.record_id)
        .unwrap();
    }
}

impl Drop for ReviewFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn auto_approve_defaults_to_manual_and_reads_the_worker_node_each_attempt() {
    let f = ReviewFixture::new();
    FileSettingsService::for_node(&f.service.refine_dir, "default")
        .update(&json!({"auto_approve": true}))
        .unwrap();
    f.poll(0);
    let first_seen = f.record().review_seen_at;
    assert!(first_seen.is_some());
    f.poll(0);
    assert_eq!(f.status(), GoalStatus::Review);
    assert_eq!(f.record().review_seen_at, first_seen);
    assert_eq!(f.mail.notifications.get(), 0);

    let head = git(&f.service.target_root, &["rev-parse", "HEAD"]);
    f.enable(true);
    f.poll(0);
    assert_eq!(f.status(), GoalStatus::Done);
    assert_eq!(f.record().status, DevelopmentRequestStatus::Notified);
    assert_eq!(f.record().review_seen_at, first_seen);
    assert_eq!(git(&f.service.target_root, &["rev-parse", "HEAD"]), head);
    f.enable(false);
    let notified = f.record();
    f.poll(0);
    assert_eq!(f.record(), notified);
    assert_eq!(f.status(), GoalStatus::Done);
    assert_eq!(f.mail.notifications.get(), 1);
}

#[test]
fn auto_approve_honors_recorded_delay_and_live_disable_without_restart() {
    let f = ReviewFixture::new();
    f.poll(3600);
    let first_seen = f.record().review_seen_at;
    f.enable(true);
    f.poll(3600);
    assert_eq!(f.status(), GoalStatus::Review);
    assert_eq!(f.record().review_seen_at, first_seen);

    // Advance the persisted observation time instead of sleeping in the test.
    let mut record = f.record();
    record.review_seen_at = Some((Utc::now() - chrono::Duration::seconds(3601)).to_rfc3339());
    f.service.write_record(&record).unwrap();
    f.enable(false);
    f.poll(3600);
    assert_eq!(f.status(), GoalStatus::Review);
    f.enable(true);
    f.poll(3600);
    assert_eq!(f.status(), GoalStatus::Done);
    assert_eq!(f.record().review_seen_at, record.review_seen_at);
    assert_eq!(f.mail.notifications.get(), 1);
}

#[test]
fn auto_approve_invalid_settings_keep_review_observation_and_retry_evidence() {
    let f = ReviewFixture::new();
    f.corrupt_setting();
    f.poll(0);
    let observed = f.record();
    assert!(observed.review_seen_at.is_some());
    assert_eq!(f.status(), GoalStatus::Review);
    assert_eq!(observed.attempts, 1);
    assert!(
        observed
            .last_error
            .unwrap()
            .contains("auto_approve must be true or false")
    );
    f.poll(0);
    assert_eq!(f.record().review_seen_at, observed.review_seen_at);
    assert_eq!(f.record().attempts, 2);
    assert_eq!(f.mail.notifications.get(), 0);
}

#[test]
fn auto_approve_settings_read_failures_preserve_review_until_repaired() {
    for unreadable in [false, true] {
        let f = ReviewFixture::new();
        f.enable(true);
        let path = FileSettingsService::for_node(&f.service.refine_dir, "worker").path();
        let saved = fs::read(&path).unwrap();
        if unreadable {
            // A directory deterministically produces a read error even as root.
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();
        } else {
            fs::write(&path, b"{broken registry").unwrap();
        }
        f.poll(0);
        let observed = f.record();
        assert_eq!(f.status(), GoalStatus::Review);
        assert!(observed.review_seen_at.is_some());
        assert_eq!(observed.attempts, 1);
        let error = observed.last_error.as_deref().unwrap();
        assert!(error.contains(if unreadable {
            "failed to read node registry"
        } else {
            "failed to parse node registry"
        }));
        f.poll(0);
        assert_eq!(f.status(), GoalStatus::Review);
        assert_eq!(f.record().review_seen_at, observed.review_seen_at);
        assert_eq!(f.record().attempts, 2);
        assert_eq!(f.mail.notifications.get(), 0);

        if unreadable {
            fs::remove_dir(&path).unwrap();
        }
        fs::write(&path, saved).unwrap();
        f.poll(0);
        assert_eq!(f.status(), GoalStatus::Done);
        assert_eq!(f.record().review_seen_at, observed.review_seen_at);
        assert_eq!(f.record().status, DevelopmentRequestStatus::Notified);
        assert!(f.record().last_error.is_none());
        assert_eq!(f.mail.notifications.get(), 1);
    }
}

#[test]
fn resolution_retry_after_auto_approval_ignores_disabled_or_invalid_settings() {
    struct RejectResolution;

    impl MailSource for RejectResolution {
        fn pending_email_ids(&self, _: &str) -> RefineResult<Vec<String>> {
            panic!("local resolution retry must not query intake")
        }

        fn raw_email(&self, _: &str) -> RefineResult<Vec<u8>> {
            panic!("local resolution retry must not fetch raw mail")
        }

        fn mark_processed(&self, _: &str) -> RefineResult<()> {
            panic!("local resolution retry must not mark intake")
        }

        fn send_resolution(
            &self,
            _: &DevelopmentRequestSettings,
            _: &DevelopmentRequestRecord,
        ) -> RefineResult<()> {
            Err(RefineError::Io(
                "resolution temporarily unavailable".to_string(),
            ))
        }
    }

    for invalid in [false, true] {
        let f = ReviewFixture::new();
        f.enable(true);
        f.service
            .process_local_records(&RejectResolution, &settings())
            .unwrap();
        let pending = f.record();
        assert_eq!(f.status(), GoalStatus::Done);
        assert_eq!(pending.status, DevelopmentRequestStatus::Resolved);
        assert!(pending.notified_at.is_none());
        assert_eq!(pending.attempts, 1);
        assert!(
            pending
                .last_error
                .as_deref()
                .unwrap()
                .contains("resolution temporarily unavailable")
        );
        if invalid {
            f.corrupt_setting();
        } else {
            f.enable(false);
        }
        f.poll(0);
        let notified = f.record();
        assert_eq!(f.status(), GoalStatus::Done);
        assert_eq!(notified.status, DevelopmentRequestStatus::Notified);
        assert!(notified.notified_at.is_some());
        assert!(notified.last_error.is_none());
        assert_eq!(
            notified.notification_message_id,
            pending.notification_message_id
        );
        assert_eq!(notified.review_seen_at, pending.review_seen_at);
        f.poll(0);
        assert_eq!(f.record(), notified);
        assert_eq!(f.mail.notifications.get(), 1);
    }
}

#[test]
fn manual_approval_notifies_once_with_disabled_or_unreadable_settings() {
    for unreadable in [false, true] {
        let f = ReviewFixture::new();
        if unreadable {
            f.corrupt_setting();
        } else {
            f.enable(false);
        }
        f.poll(0);
        f.approve_manually();
        f.poll(0);
        assert_eq!(f.status(), GoalStatus::Done);
        assert_eq!(f.record().status, DevelopmentRequestStatus::Notified);
        assert!(f.record().last_error.is_none());
        f.poll(0);
        assert_eq!(f.mail.notifications.get(), 1);
    }
}

#[test]
fn auto_approve_requires_verified_integration_and_preserves_follow_up_rounds() {
    let f = ReviewFixture::new();
    f.enable(true);
    f.work_items
        .update_goal_round_evaluation_summary(
            &f.record_id,
            0,
            &json!({"workflow_integration": null}),
        )
        .unwrap();
    f.poll(0);
    assert_eq!(f.status(), GoalStatus::Review);
    assert!(
        f.record()
            .last_error
            .unwrap()
            .contains("without successful Governance evidence")
    );
    f.enable(false);
    f.work_items
        .append_goal_round_summary(&f.record_id, "QA", "Fix the remaining issue")
        .unwrap();
    f.poll(0);
    assert_eq!(f.status(), GoalStatus::Todo);
    f.enable(true);
    f.poll(0);
    assert_eq!(f.status(), GoalStatus::Todo);
    let detail = f.work_items.show_goal_detail(&f.record_id).unwrap();
    assert_eq!(detail["rounds"].as_array().unwrap().len(), 2);
    assert_eq!(detail["rounds"][1]["prompt"], "Fix the remaining issue");
    assert_eq!(f.mail.notifications.get(), 0);
}

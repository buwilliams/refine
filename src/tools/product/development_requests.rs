use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::security::{NativeSecretStore, SecretStore};
use crate::tools::product::merging::FileMergerService;
use crate::tools::product::work_items::{
    FeatureGoalPlacement, FileWorkItemService, GoalAuthoringRequest,
};

const JMAP_SESSION_URL: &str = "https://api.fastmail.com/jmap/session";
const TOKEN_SCOPE: &str = "email";
const TOKEN_NAME: &str = "fastmail_jmap_token";
const PROCESSED_KEYWORD: &str = "refine-processed";
mod email_source;
mod fastmail;
mod records;

use email_source::{ParsedEmail, parse_email};
use fastmail::FastmailClient;
#[cfg(test)]
use fastmail::pending_email_filter;
use records::{read_record, write_record};

const REQUEST_SCHEMA_VERSION: u64 = 2;
const CONFIG_SCHEMA_VERSION: u64 = 1;
const DEFAULT_POLL_SECONDS: u64 = 60;
const DEVELOPMENT_REQUEST_GOAL_PRIORITY: &str = "low";
pub const SELF_DEVELOPMENT_EMAIL_CONFIG_FILE: &str = "self-development-email.json";

fn default_poll_seconds() -> u64 {
    DEFAULT_POLL_SECONDS
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SelfDevelopmentEmailConfig {
    pub schema_version: u64,
    pub target_root: PathBuf,
    pub address: String,
    pub allowed_senders: BTreeSet<String>,
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: u64,
    #[serde(default)]
    pub auto_approve_after_seconds: u64,
}

pub fn self_development_email_config_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join(SELF_DEVELOPMENT_EMAIL_CONFIG_FILE)
}

pub fn load_self_development_email_config(
    runtime_root: &Path,
) -> RefineResult<Option<SelfDevelopmentEmailConfig>> {
    let path = self_development_email_config_path(runtime_root);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)
        .map_err(|error| RefineError::Io(format!("failed to read {}: {error}", path.display())))?;
    let mut config =
        serde_json::from_slice::<SelfDevelopmentEmailConfig>(&bytes).map_err(|error| {
            RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
        })?;
    if config.schema_version != CONFIG_SCHEMA_VERSION {
        return Err(RefineError::InvalidInput(format!(
            "{} schema_version must be {CONFIG_SCHEMA_VERSION}",
            path.display()
        )));
    }
    if !config.target_root.is_absolute() {
        return Err(RefineError::InvalidInput(format!(
            "{} target_root must be an absolute path",
            path.display()
        )));
    }
    config.target_root = config.target_root.canonicalize().map_err(|error| {
        RefineError::InvalidInput(format!(
            "{} target_root {} cannot be resolved: {error}",
            path.display(),
            config.target_root.display()
        ))
    })?;
    config.address = config.address.trim().to_ascii_lowercase();
    if config.address.is_empty() || !config.address.contains('@') {
        return Err(RefineError::InvalidInput(format!(
            "{} address must be a valid non-empty email address",
            path.display()
        )));
    }
    config.allowed_senders = config
        .allowed_senders
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    if config.allowed_senders.is_empty() {
        return Err(RefineError::InvalidInput(format!(
            "{} allowed_senders must contain at least one address",
            path.display()
        )));
    }
    config.poll_seconds = config.poll_seconds.max(1);
    Ok(Some(config))
}

pub fn self_development_email_target_is_active(
    config: &SelfDevelopmentEmailConfig,
    active_target_root: &Path,
) -> RefineResult<bool> {
    let active_target_root = active_target_root.canonicalize().map_err(|error| {
        RefineError::InvalidInput(format!(
            "active target_root {} cannot be resolved: {error}",
            active_target_root.display()
        ))
    })?;
    Ok(active_target_root == config.target_root)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevelopmentRequestSettings {
    pub address: String,
    pub allowed_senders: BTreeSet<String>,
    pub auto_approve_after: Duration,
}

impl DevelopmentRequestSettings {
    pub fn from_local_config(config: &SelfDevelopmentEmailConfig) -> Self {
        Self {
            address: config.address.clone(),
            allowed_senders: config.allowed_senders.clone(),
            auto_approve_after: Duration::from_secs(config.auto_approve_after_seconds),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentRequestStatus {
    Received,
    Ignored,
    GoalCreated,
    Resolved,
    Notified,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DevelopmentRequestRecord {
    pub schema_version: u64,
    pub id: String,
    pub provider_email_id: String,
    pub message_id: Option<String>,
    pub sender: String,
    pub subject: String,
    pub source_text: String,
    pub status: DevelopmentRequestStatus,
    pub received_at: String,
    pub updated_at: String,
    pub goal_id: Option<String>,
    pub goal_name: Option<String>,
    pub review_seen_at: Option<String>,
    pub notification_message_id: String,
    pub notified_at: Option<String>,
    pub last_error: Option<String>,
    pub attempts: u64,
}

trait MailSource {
    fn pending_email_ids(&self, address: &str) -> RefineResult<Vec<String>>;
    fn raw_email(&self, email_id: &str) -> RefineResult<Vec<u8>>;
    fn mark_processed(&self, email_id: &str) -> RefineResult<()>;
    fn send_resolution(
        &self,
        settings: &DevelopmentRequestSettings,
        record: &DevelopmentRequestRecord,
    ) -> RefineResult<()>;
}

#[derive(Clone, Debug)]
pub struct FileDevelopmentRequestService {
    runtime_root: PathBuf,
    refine_dir: PathBuf,
    target_root: PathBuf,
    #[cfg(test)]
    fail_next_record_write: std::cell::Cell<bool>,
}

impl FileDevelopmentRequestService {
    pub fn new(
        runtime_root: impl Into<PathBuf>,
        refine_dir: impl Into<PathBuf>,
        target_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            refine_dir: refine_dir.into(),
            target_root: target_root.into(),
            #[cfg(test)]
            fail_next_record_write: std::cell::Cell::new(false),
        }
    }

    pub fn process_once(&self, settings: &DevelopmentRequestSettings) -> RefineResult<()> {
        if settings.allowed_senders.is_empty() {
            return Err(RefineError::InvalidInput(
                "self-development email allowed_senders must contain at least one address"
                    .to_string(),
            ));
        }
        let token = NativeSecretStore::new(&self.runtime_root)
            .get_secret(TOKEN_SCOPE, TOKEN_NAME)?
            .value;
        let fastmail = FastmailClient::connect(token)?;
        self.ingest(&fastmail, settings)?;
        self.process_local_records(&fastmail, settings)
    }

    fn ingest(
        &self,
        fastmail: &dyn MailSource,
        settings: &DevelopmentRequestSettings,
    ) -> RefineResult<()> {
        for email_id in fastmail.pending_email_ids(&settings.address)? {
            let raw = fastmail.raw_email(&email_id)?;
            let parsed = parse_email(&raw)?;
            if !settings.allowed_senders.contains(&parsed.sender) {
                fastmail.mark_processed(&email_id)?;
                continue;
            }
            let record = self.record_from_email(&email_id, parsed, &settings.address);
            let path = self.record_path(&record.id);
            if !path.exists() {
                self.write_record(&record)?;
            }
            // A remote message is acknowledged only after its local retry record is durable.
            fastmail.mark_processed(&email_id)?;
        }
        Ok(())
    }

    fn record_from_email(
        &self,
        email_id: &str,
        parsed: ParsedEmail,
        address: &str,
    ) -> DevelopmentRequestRecord {
        let id = request_id(email_id);
        let now = Utc::now().to_rfc3339();
        let message_id_domain = address
            .split_once('@')
            .map_or(address, |(_, domain)| domain);
        DevelopmentRequestRecord {
            schema_version: REQUEST_SCHEMA_VERSION,
            id: id.clone(),
            provider_email_id: email_id.to_string(),
            message_id: parsed.message_id,
            sender: parsed.sender,
            subject: parsed.subject,
            source_text: parsed.source_text,
            status: DevelopmentRequestStatus::Received,
            received_at: now.clone(),
            updated_at: now,
            goal_id: None,
            goal_name: None,
            review_seen_at: None,
            notification_message_id: format!("refine-{id}@{message_id_domain}"),
            notified_at: None,
            last_error: None,
            attempts: 0,
        }
    }

    fn process_local_records(
        &self,
        fastmail: &dyn MailSource,
        settings: &DevelopmentRequestSettings,
    ) -> RefineResult<()> {
        for path in self.record_paths()? {
            let mut record = match self.read_record(&path) {
                Ok(record) => record,
                Err(error) => {
                    eprintln!(
                        "refine development request record {} was isolated: {error}",
                        path.display()
                    );
                    continue;
                }
            };
            let result = match record.status {
                DevelopmentRequestStatus::Received => {
                    self.recover_or_create_goal(&mut record, fastmail, settings)
                }
                DevelopmentRequestStatus::GoalCreated | DevelopmentRequestStatus::Resolved => {
                    self.advance_goal_and_notify(&mut record, fastmail, settings)
                }
                DevelopmentRequestStatus::Ignored | DevelopmentRequestStatus::Notified => Ok(()),
            };
            if let Err(error) = result {
                record.attempts = record.attempts.saturating_add(1);
                record.last_error = Some(error.to_string());
                record.updated_at = Utc::now().to_rfc3339();
                self.write_record(&record)?;
                eprintln!("refine development request {}: {error}", record.id);
            }
        }
        Ok(())
    }

    fn recover_or_create_goal(
        &self,
        record: &mut DevelopmentRequestRecord,
        mail: &dyn MailSource,
        settings: &DevelopmentRequestSettings,
    ) -> RefineResult<()> {
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        let name = development_request_goal_name(&record.subject);
        match work_items.show_goal_summary(&record.id) {
            Ok(goal) => {
                if record.schema_version == REQUEST_SCHEMA_VERSION {
                    validate_recovered_goal(&work_items, record, &name)?;
                } else {
                    self.validate_legacy_goal_ownership(&goal.goal)?;
                }
                record.goal_id = Some(goal.goal.id);
                record.goal_name = Some(goal.goal.name);
                record.status = DevelopmentRequestStatus::GoalCreated;
                record.last_error = None;
                record.updated_at = Utc::now().to_rfc3339();
                return self.write_record(record);
            }
            Err(RefineError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }

        if record.schema_version == 1 {
            self.migrate_received_record(record, mail, settings)?;
        }
        let goal = work_items
            .author_goal(development_request_goal_authoring_request(record, name))?
            .goal
            .ok_or_else(|| {
                RefineError::Conflict("development request did not produce a Goal".to_string())
            })?;
        record.goal_id = Some(goal.id);
        record.goal_name = Some(goal.name);
        record.status = DevelopmentRequestStatus::GoalCreated;
        record.last_error = None;
        record.updated_at = Utc::now().to_rfc3339();
        self.write_record(record)
    }

    fn migrate_received_record(
        &self,
        record: &mut DevelopmentRequestRecord,
        mail: &dyn MailSource,
        settings: &DevelopmentRequestSettings,
    ) -> RefineResult<()> {
        let parsed = parse_email(&mail.raw_email(&record.provider_email_id)?)?;
        if !sender_is_trusted(settings, &parsed.sender) {
            return Err(RefineError::InvalidInput(format!(
                "schema 1 request {} re-fetched an untrusted sender {}",
                record.id, parsed.sender
            )));
        }
        let mut upgraded = record.clone();
        upgraded.schema_version = REQUEST_SCHEMA_VERSION;
        upgraded.message_id = parsed.message_id;
        upgraded.sender = parsed.sender;
        upgraded.subject = parsed.subject;
        upgraded.source_text = parsed.source_text;
        upgraded.updated_at = Utc::now().to_rfc3339();
        upgraded.last_error = None;
        // The migration write must become durable before the authoring seam is entered.
        self.write_record(&upgraded)?;
        *record = upgraded;
        Ok(())
    }

    fn validate_legacy_goal_ownership(
        &self,
        goal: &crate::model::goal::GoalIndexProjection,
    ) -> RefineResult<()> {
        let active_node = crate::tools::product::nodes::FileNodeRegistryService::with_active_root(
            &self.refine_dir,
            &self.runtime_root,
        )
        .active_node_id()?;
        if goal.node_id.as_deref().unwrap_or("default") != active_node {
            return Err(RefineError::Conflict(format!(
                "legacy development-request Goal {} belongs to a different node",
                goal.id
            )));
        }
        Ok(())
    }

    fn advance_goal_and_notify(
        &self,
        record: &mut DevelopmentRequestRecord,
        fastmail: &dyn MailSource,
        settings: &DevelopmentRequestSettings,
    ) -> RefineResult<()> {
        let goal_id = record.goal_id.as_deref().ok_or_else(|| {
            RefineError::Serialization(format!("request {} has no linked Goal", record.id))
        })?;
        let work_items = FileWorkItemService::with_projection_cache(
            &self.refine_dir,
            &self.runtime_root,
            self.runtime_root.join("cache"),
        );
        let mut goal = work_items.show_goal_summary(goal_id)?;
        if goal.goal.status == GoalStatus::Review {
            let now = Utc::now();
            let first_seen = match &record.review_seen_at {
                Some(value) => DateTime::parse_from_rfc3339(value)
                    .map(|value| value.with_timezone(&Utc))
                    .unwrap_or(now),
                None => {
                    record.review_seen_at = Some(now.to_rfc3339());
                    self.write_record(record)?;
                    now
                }
            };
            if now.signed_duration_since(first_seen).num_seconds()
                >= settings.auto_approve_after.as_secs() as i64
            {
                FileMergerService::with_target_root(
                    &self.runtime_root,
                    &self.refine_dir,
                    &self.target_root,
                )
                .approve_reviewed_goal(goal_id)?;
                goal = work_items.show_goal_summary(goal_id)?;
            }
        }
        if goal.goal.status != GoalStatus::Done {
            return Ok(());
        }
        record.status = DevelopmentRequestStatus::Resolved;
        record.last_error = None;
        record.updated_at = Utc::now().to_rfc3339();
        self.write_record(record)?;
        fastmail.send_resolution(settings, record)?;
        record.status = DevelopmentRequestStatus::Notified;
        record.notified_at = Some(Utc::now().to_rfc3339());
        record.updated_at = record.notified_at.clone().unwrap_or_default();
        self.write_record(record)
    }

    fn records_dir(&self) -> PathBuf {
        self.runtime_root
            .join("self-development-email")
            .join("requests")
    }

    fn record_path(&self, request_id: &str) -> PathBuf {
        self.records_dir().join(request_id).join("request.json")
    }

    fn record_paths(&self) -> RefineResult<Vec<PathBuf>> {
        let root = self.records_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut paths = fs::read_dir(&root)
            .map_err(|error| {
                RefineError::Io(format!("failed to read {}: {error}", root.display()))
            })?
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("request.json"))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }

    fn read_record(&self, path: &Path) -> RefineResult<DevelopmentRequestRecord> {
        read_record(path)
    }

    fn write_record(&self, record: &DevelopmentRequestRecord) -> RefineResult<()> {
        #[cfg(test)]
        if self.fail_next_record_write.replace(false) {
            return Err(RefineError::Io(
                "injected development-request record write interruption".to_string(),
            ));
        }
        let path = self.record_path(&record.id);
        write_record(&path, record)
    }
}

fn request_id(provider_email_id: &str) -> String {
    let digest = Sha256::digest(provider_email_id.as_bytes());
    format!("DR{:X}", digest)[..26].to_string()
}

fn development_request_goal_authoring_request(
    record: &DevelopmentRequestRecord,
    name: String,
) -> GoalAuthoringRequest {
    GoalAuthoringRequest {
        id: Some(record.id.clone()),
        name: Some(name),
        prompt: record.source_text.clone(),
        reporter: record.sender.clone(),
        assignee: None,
        priority: DEVELOPMENT_REQUEST_GOAL_PRIORITY.to_string(),
        feature_id: None,
        placement: FeatureGoalPlacement::Unordered,
        duplicate_decision: "original".to_string(),
        ..GoalAuthoringRequest::default()
    }
}

fn development_request_goal_name(subject: &str) -> String {
    let subject = subject.trim();
    if subject.is_empty() {
        "Development request".to_string()
    } else {
        subject.to_string()
    }
}

fn sender_is_trusted(settings: &DevelopmentRequestSettings, sender: &str) -> bool {
    settings.allowed_senders.contains(sender)
}

fn validate_recovered_goal(
    work_items: &FileWorkItemService,
    record: &DevelopmentRequestRecord,
    expected_name: &str,
) -> RefineResult<()> {
    let detail = work_items.show_goal_detail(&record.id)?;
    let rounds = detail
        .get("rounds")
        .and_then(Value::as_array)
        .ok_or_else(|| RefineError::Conflict(format!("Goal {} has no Round array", record.id)))?;
    let expected_assignee = record.sender.as_str();
    let source_matches = rounds.len() == 1
        && rounds[0].get("prompt").and_then(Value::as_str) == Some(record.source_text.as_str())
        && rounds[0].get("assignee").and_then(Value::as_str) == Some(expected_assignee);
    let metadata_matches = detail.get("name").and_then(Value::as_str) == Some(expected_name)
        && detail.get("priority").and_then(Value::as_str)
            == Some(DEVELOPMENT_REQUEST_GOAL_PRIORITY)
        && detail.get("reporter").and_then(Value::as_str) == Some(record.sender.as_str());
    if !source_matches || !metadata_matches {
        return Err(RefineError::Conflict(format!(
            "development request {} found a Goal that does not match its authoritative source revision",
            record.id
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

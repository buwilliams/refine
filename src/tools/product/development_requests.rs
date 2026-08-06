use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use mail_parser::{MessageParser, MimeHeaders};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::model::JsonObject;
use crate::model::workflow::GoalStatus;
use crate::process::subprocess::write_json_atomically;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::security::{NativeSecretStore, SecretStore};
use crate::prompts::{PromptTemplate, render};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::product::merging::FileMergerService;
use crate::tools::product::work_items::{
    FeatureGoalPlacement, FileWorkItemService, GoalAuthoringRequest,
};

const JMAP_SESSION_URL: &str = "https://api.fastmail.com/jmap/session";
const TOKEN_SCOPE: &str = "email";
const TOKEN_NAME: &str = "fastmail_jmap_token";
const PROCESSED_KEYWORD: &str = "refine-processed";
const REQUEST_SCHEMA_VERSION: u64 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevelopmentRequestSettings {
    pub enabled: bool,
    pub address: String,
    pub mailbox: String,
    pub allowed_senders: BTreeSet<String>,
    pub poll_interval: Duration,
    pub auto_approve_after: Duration,
    pub provider: String,
}

impl DevelopmentRequestSettings {
    pub fn from_project_settings(settings: &JsonObject) -> Self {
        let text = |key: &str, fallback: &str| {
            settings
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(fallback)
                .to_string()
        };
        let seconds = |key: &str, fallback: u64| {
            settings
                .get(key)
                .and_then(Value::as_str)
                .and_then(|value| value.trim().parse::<u64>().ok())
                .unwrap_or(fallback)
        };
        let provider = text("development_request_agent_cli", "");
        Self {
            enabled: settings
                .get("development_request_email_enabled")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "1"),
            address: text("development_request_address", "goal@getrefine.dev")
                .to_ascii_lowercase(),
            mailbox: text("development_request_mailbox", "Development Requests"),
            allowed_senders: parse_allowed_senders(
                settings
                    .get("development_request_allowed_senders")
                    .and_then(Value::as_str)
                    .unwrap_or(
                        "bwilliams@nevo.com, ejacobson@nevo.com, ethan.jacobson@insurity.com, buddy.williams@insurity.com, buddywilliams@gmail.com",
                    ),
            ),
            poll_interval: Duration::from_secs(seconds("development_request_poll_seconds", 60).max(1)),
            auto_approve_after: Duration::from_secs(seconds(
                "development_request_auto_approve_after_seconds",
                0,
            )),
            provider: if provider.is_empty() {
                text("agent_cli", "claude")
            } else {
                provider
            },
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
enum ReviewDecision {
    CreateGoal {
        name: String,
        prompt: String,
        priority: String,
    },
    Ignore {
        reason: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ParsedEmail {
    message_id: Option<String>,
    sender: String,
    subject: String,
    source_text: String,
}

#[derive(Clone, Debug, Deserialize)]
struct JmapSession {
    #[serde(rename = "apiUrl")]
    api_url: String,
    #[serde(rename = "downloadUrl")]
    download_url: String,
    #[serde(rename = "primaryAccounts")]
    primary_accounts: Map<String, Value>,
}

#[derive(Clone, Debug)]
struct FastmailClient {
    http: Client,
    token: String,
    api_url: String,
    download_url: String,
    account_id: String,
}

impl FastmailClient {
    fn connect(token: String) -> RefineResult<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(http_error)?;
        let session = http
            .get(JMAP_SESSION_URL)
            .bearer_auth(&token)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(http_error)?
            .json::<JmapSession>()
            .map_err(http_error)?;
        let account_id = session
            .primary_accounts
            .get("urn:ietf:params:jmap:mail")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RefineError::Conflict(
                    "Fastmail JMAP session has no primary mail account".to_string(),
                )
            })?
            .to_string();
        Ok(Self {
            http,
            token,
            api_url: session.api_url,
            download_url: session.download_url,
            account_id,
        })
    }

    fn call(&self, using: &[&str], method_calls: Vec<Value>) -> RefineResult<Value> {
        let response = self
            .http
            .post(&self.api_url)
            .bearer_auth(&self.token)
            .json(&json!({"using": using, "methodCalls": method_calls}))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(http_error)?
            .json::<Value>()
            .map_err(http_error)?;
        if let Some(error) = response
            .get("methodResponses")
            .and_then(Value::as_array)
            .and_then(|responses| {
                responses
                    .iter()
                    .find(|response| response.get(0).and_then(Value::as_str) == Some("error"))
            })
        {
            return Err(RefineError::Conflict(format!(
                "Fastmail JMAP method failed: {error}"
            )));
        }
        Ok(response)
    }

    fn method_result<'a>(response: &'a Value, name: &str) -> RefineResult<&'a Value> {
        response
            .get("methodResponses")
            .and_then(Value::as_array)
            .and_then(|responses| {
                responses
                    .iter()
                    .find(|response| response.get(0).and_then(Value::as_str) == Some(name))
            })
            .and_then(|response| response.get(1))
            .ok_or_else(|| RefineError::Serialization(format!("Fastmail response omitted {name}")))
    }

    fn mailbox_id(&self, name: &str) -> RefineResult<String> {
        let response = self.call(
            &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            vec![json!(["Mailbox/get", {"accountId": self.account_id}, "mailboxes"])],
        )?;
        Self::method_result(&response, "Mailbox/get")?
            .get("list")
            .and_then(Value::as_array)
            .and_then(|mailboxes| {
                mailboxes
                    .iter()
                    .find(|mailbox| mailbox.get("name").and_then(Value::as_str) == Some(name))
            })
            .and_then(|mailbox| mailbox.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                RefineError::NotFound(format!("Fastmail mailbox {name:?} was not found"))
            })
    }

    fn mailbox_id_by_role(&self, role: &str) -> RefineResult<String> {
        let response = self.call(
            &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            vec![json!(["Mailbox/get", {"accountId": self.account_id}, "mailboxes"])],
        )?;
        Self::method_result(&response, "Mailbox/get")?
            .get("list")
            .and_then(Value::as_array)
            .and_then(|mailboxes| {
                mailboxes
                    .iter()
                    .find(|mailbox| mailbox.get("role").and_then(Value::as_str) == Some(role))
            })
            .and_then(|mailbox| mailbox.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| RefineError::NotFound(format!("Fastmail {role} mailbox was not found")))
    }

    fn pending_email_ids(&self, mailbox_id: &str) -> RefineResult<Vec<String>> {
        let response = self.call(
            &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            vec![json!(["Email/query", {
                "accountId": self.account_id,
                "filter": {"inMailbox": mailbox_id, "notKeyword": PROCESSED_KEYWORD},
                "sort": [{"property": "receivedAt", "isAscending": true}],
                "limit": 25
            }, "pending"])],
        )?;
        Ok(Self::method_result(&response, "Email/query")?
            .get("ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect())
    }

    fn raw_email(&self, email_id: &str) -> RefineResult<Vec<u8>> {
        let response = self.call(
            &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            vec![json!(["Email/get", {
                "accountId": self.account_id,
                "ids": [email_id],
                "properties": ["id", "blobId"]
            }, "email"])],
        )?;
        let blob_id = Self::method_result(&response, "Email/get")?
            .get("list")
            .and_then(Value::as_array)
            .and_then(|emails| emails.first())
            .and_then(|email| email.get("blobId"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RefineError::Serialization(format!("Fastmail email {email_id} has no blobId"))
            })?;
        let url = self
            .download_url
            .replace("{accountId}", &self.account_id)
            .replace("{blobId}", blob_id)
            .replace("{name}", "message.eml")
            .replace("{type}", "message%2Frfc822");
        Ok(self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(http_error)?
            .bytes()
            .map_err(http_error)?
            .to_vec())
    }

    fn mark_processed(&self, email_id: &str) -> RefineResult<()> {
        let response = self.call(
            &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            vec![json!(["Email/set", {
                "accountId": self.account_id,
                "update": {email_id: {format!("keywords/{PROCESSED_KEYWORD}"): true}}
            }, "processed"])],
        )?;
        ensure_set_succeeded(
            Self::method_result(&response, "Email/set")?,
            "mark email processed",
        )
    }

    fn identity_id(&self, address: &str) -> RefineResult<String> {
        let response = self.call(
            &[
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:mail",
                "urn:ietf:params:jmap:submission",
            ],
            vec![json!(["Identity/get", {"accountId": self.account_id}, "identities"])],
        )?;
        Self::method_result(&response, "Identity/get")?
            .get("list")
            .and_then(Value::as_array)
            .and_then(|identities| {
                identities.iter().find(|identity| {
                    identity
                        .get("email")
                        .and_then(Value::as_str)
                        .is_some_and(|email| email.eq_ignore_ascii_case(address))
                })
            })
            .and_then(|identity| identity.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                RefineError::NotFound(format!("Fastmail identity {address} was not found"))
            })
    }

    fn sent_contains_message_id(&self, sent_id: &str, message_id: &str) -> RefineResult<bool> {
        let response = self.call(
            &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            vec![json!(["Email/query", {
                "accountId": self.account_id,
                "filter": {"inMailbox": sent_id, "header": ["Message-ID", message_id]},
                "limit": 1
            }, "sent-query"])],
        )?;
        Ok(Self::method_result(&response, "Email/query")?
            .get("ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| !ids.is_empty()))
    }

    fn send_resolution(
        &self,
        settings: &DevelopmentRequestSettings,
        record: &DevelopmentRequestRecord,
    ) -> RefineResult<()> {
        let drafts_id = self.mailbox_id_by_role("drafts")?;
        let sent_id = self.mailbox_id_by_role("sent")?;
        if self
            .sent_contains_message_id(&sent_id, &format!("<{}>", record.notification_message_id))?
        {
            return Ok(());
        }
        let identity_id = self.identity_id(&settings.address)?;
        let subject = if record.subject.to_ascii_lowercase().starts_with("re:") {
            record.subject.clone()
        } else {
            format!("Re: {}", record.subject)
        };
        let goal_id = record.goal_id.as_deref().unwrap_or("unknown");
        let goal_name = record.goal_name.as_deref().unwrap_or("Development request");
        let body = format!(
            "Your development request has been resolved.\n\nGoal: {goal_name} ({goal_id})\n\nThis confirms the Refine Goal is done; it does not make a separate deployment claim.\n"
        );
        let mut draft = json!({
            "mailboxIds": {drafts_id.clone(): true},
            "keywords": {"$draft": true, "$seen": true},
            "from": [{"email": settings.address}],
            "to": [{"email": record.sender}],
            "subject": subject,
            "textBody": [{"partId": "body", "type": "text/plain"}],
            "bodyValues": {"body": {"value": body, "isTruncated": false}},
            "header:Message-ID:asMessageIds": [record.notification_message_id]
        });
        if let Some(message_id) = record.message_id.as_ref().filter(|value| !value.is_empty()) {
            draft["header:In-Reply-To:asMessageIds"] = json!([message_id]);
            draft["header:References:asMessageIds"] = json!([message_id]);
        }
        let response = self.call(
            &[
                "urn:ietf:params:jmap:core",
                "urn:ietf:params:jmap:mail",
                "urn:ietf:params:jmap:submission",
            ],
            vec![
                json!(["Email/set", {
                    "accountId": self.account_id,
                    "create": {"draft": draft}
                }, "draft"]),
                json!(["EmailSubmission/set", {
                    "accountId": self.account_id,
                    "create": {"submission": {"emailId": "#draft", "identityId": identity_id}},
                    "onSuccessUpdateEmail": {"#submission": {
                        format!("mailboxIds/{drafts_id}"): null,
                        format!("mailboxIds/{sent_id}"): true,
                        "keywords/$draft": null
                    }}
                }, "submit"]),
            ],
        )?;
        ensure_set_succeeded(
            Self::method_result(&response, "Email/set")?,
            "create resolution email",
        )?;
        ensure_set_succeeded(
            Self::method_result(&response, "EmailSubmission/set")?,
            "submit resolution email",
        )
    }
}

#[derive(Clone, Debug)]
pub struct FileDevelopmentRequestService {
    runtime_root: PathBuf,
    refine_dir: PathBuf,
    target_root: PathBuf,
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
        }
    }

    pub fn process_once(&self, settings: &DevelopmentRequestSettings) -> RefineResult<()> {
        if !settings.enabled {
            return Ok(());
        }
        if settings.allowed_senders.is_empty() {
            return Err(RefineError::InvalidInput(
                "development_request_allowed_senders must contain at least one address".to_string(),
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
        fastmail: &FastmailClient,
        settings: &DevelopmentRequestSettings,
    ) -> RefineResult<()> {
        let mailbox_id = fastmail.mailbox_id(&settings.mailbox)?;
        for email_id in fastmail.pending_email_ids(&mailbox_id)? {
            let raw = fastmail.raw_email(&email_id)?;
            let parsed = parse_email(&raw)?;
            if !settings.allowed_senders.contains(&parsed.sender) {
                fastmail.mark_processed(&email_id)?;
                continue;
            }
            let record = self.record_from_email(&email_id, parsed);
            let path = self.record_path(&record.id);
            if !path.exists() {
                self.write_record(&record)?;
            }
            // A remote message is acknowledged only after its local retry record is durable.
            fastmail.mark_processed(&email_id)?;
        }
        Ok(())
    }

    fn record_from_email(&self, email_id: &str, parsed: ParsedEmail) -> DevelopmentRequestRecord {
        let id = request_id(email_id);
        let now = Utc::now().to_rfc3339();
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
            notification_message_id: format!("refine-{id}@getrefine.dev"),
            notified_at: None,
            last_error: None,
            attempts: 0,
        }
    }

    fn process_local_records(
        &self,
        fastmail: &FastmailClient,
        settings: &DevelopmentRequestSettings,
    ) -> RefineResult<()> {
        for path in self.record_paths()? {
            let mut record = self.read_record(&path)?;
            let result = match record.status {
                DevelopmentRequestStatus::Received => {
                    self.review_and_create_goal(&mut record, settings)
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

    fn review_and_create_goal(
        &self,
        record: &mut DevelopmentRequestRecord,
        settings: &DevelopmentRequestSettings,
    ) -> RefineResult<()> {
        let email = format!(
            "From: {}\nSubject: {}\n\n{}",
            record.sender, record.subject, record.source_text
        );
        let output = HostAgentProviderService::with_runtime_root(&self.runtime_root).invoke(
            ProviderInvocation {
                provider: settings.provider.clone(),
                prompt: render(
                    PromptTemplate::DevelopmentRequestReview,
                    &[("email", &email)],
                ),
                session_id: None,
                cwd: Some(self.target_root.display().to_string()),
                process_metadata: Map::from_iter([
                    (
                        "kind".to_string(),
                        Value::String("development_request_review".to_string()),
                    ),
                    ("request_id".to_string(), Value::String(record.id.clone())),
                ]),
            },
        )?;
        match parse_review_decision(&output)? {
            ReviewDecision::Ignore { reason } => {
                record.status = DevelopmentRequestStatus::Ignored;
                record.last_error = reason;
            }
            ReviewDecision::CreateGoal {
                name,
                prompt,
                priority,
            } => {
                if !matches!(priority.as_str(), "low" | "medium" | "high") {
                    return Err(RefineError::InvalidInput(
                        "development request reviewer returned an invalid priority".to_string(),
                    ));
                }
                let work_items = FileWorkItemService::with_projection_cache(
                    &self.refine_dir,
                    &self.runtime_root,
                    self.runtime_root.join("cache"),
                );
                // The request ID is also the Goal ID. If the process stopped after the Goal write
                // but before the request-record write, recover that exact Goal on the next poll.
                if let Ok(goal) = work_items.show_goal_summary(&record.id) {
                    record.goal_id = Some(goal.goal.id);
                    record.goal_name = Some(goal.goal.name);
                    record.status = DevelopmentRequestStatus::GoalCreated;
                    record.last_error = None;
                    record.updated_at = Utc::now().to_rfc3339();
                    return self.write_record(record);
                }
                let result = work_items.author_goal(GoalAuthoringRequest {
                    id: Some(record.id.clone()),
                    name: Some(name.clone()),
                    prompt,
                    reporter: record.sender.clone(),
                    assignee: None,
                    priority,
                    feature_id: None,
                    placement: FeatureGoalPlacement::Unordered,
                    duplicate_decision: "original".to_string(),
                    ..GoalAuthoringRequest::default()
                })?;
                let goal = result.goal.ok_or_else(|| {
                    RefineError::Conflict("development request did not produce a Goal".to_string())
                })?;
                record.goal_id = Some(goal.id);
                record.goal_name = Some(name);
                record.status = DevelopmentRequestStatus::GoalCreated;
                record.last_error = None;
            }
        }
        record.updated_at = Utc::now().to_rfc3339();
        self.write_record(record)
    }

    fn advance_goal_and_notify(
        &self,
        record: &mut DevelopmentRequestRecord,
        fastmail: &FastmailClient,
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
        self.refine_dir.join("development-requests")
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
        let bytes = fs::read(path).map_err(|error| {
            RefineError::Io(format!("failed to read {}: {error}", path.display()))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            RefineError::Serialization(format!("failed to parse {}: {error}", path.display()))
        })
    }

    fn write_record(&self, record: &DevelopmentRequestRecord) -> RefineResult<()> {
        let path = self.record_path(&record.id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!("failed to create {}: {error}", parent.display()))
            })?;
        }
        let encoded = serde_json::to_vec_pretty(record).map_err(|error| {
            RefineError::Serialization(format!("failed to encode request {}: {error}", record.id))
        })?;
        write_json_atomically(&path, &encoded, "development request")
    }
}

fn http_error(error: reqwest::Error) -> RefineError {
    RefineError::Io(format!("Fastmail request failed: {error}"))
}

fn ensure_set_succeeded(result: &Value, action: &str) -> RefineResult<()> {
    if result
        .get("notCreated")
        .and_then(Value::as_object)
        .is_some_and(|value| !value.is_empty())
        || result
            .get("notUpdated")
            .and_then(Value::as_object)
            .is_some_and(|value| !value.is_empty())
    {
        return Err(RefineError::Conflict(format!(
            "Fastmail could not {action}: {result}"
        )));
    }
    Ok(())
}

fn parse_allowed_senders(value: &str) -> BTreeSet<String> {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn parse_email(raw: &[u8]) -> RefineResult<ParsedEmail> {
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| RefineError::Serialization("failed to parse RFC 5322 email".to_string()))?;
    let sender = message
        .from()
        .and_then(|address| address.first())
        .and_then(|address| address.address.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| RefineError::InvalidInput("email has no From address".to_string()))?;
    let mut sections = Vec::new();
    if let Some(body) = message.body_text(0) {
        let body = body.trim();
        if !body.is_empty() {
            sections.push(body.to_string());
        }
    }
    for attachment in message.attachments() {
        if attachment
            .attachment_name()
            .is_some_and(|name| name.to_ascii_lowercase().ends_with(".txt"))
            && let Some(text) = attachment
                .text_contents()
                .map(str::trim)
                .filter(|text| !text.is_empty())
        {
            sections.push(format!("Text attachment:\n{text}"));
        }
    }
    Ok(ParsedEmail {
        message_id: message.message_id().map(str::to_string),
        sender,
        subject: message
            .subject()
            .unwrap_or("Development request")
            .trim()
            .to_string(),
        source_text: sections.join("\n\n"),
    })
}

fn request_id(provider_email_id: &str) -> String {
    let digest = Sha256::digest(provider_email_id.as_bytes());
    format!("DR{:X}", digest)[..26].to_string()
}

fn parse_review_decision(output: &str) -> RefineResult<ReviewDecision> {
    let trimmed = output.trim();
    let candidate = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    serde_json::from_str(candidate).map_err(|error| {
        RefineError::Serialization(format!(
            "development request reviewer returned invalid JSON: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_allowlist_is_case_insensitive_and_deduplicated() {
        assert_eq!(
            parse_allowed_senders(" Buddy@example.com,\nBUDDY@example.com\nteam@example.com "),
            BTreeSet::from([
                "buddy@example.com".to_string(),
                "team@example.com".to_string()
            ])
        );
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
                source_text: "Please implement this.".to_string(),
            },
        );
        service.write_record(&record).unwrap();
        assert_eq!(
            service
                .read_record(&service.record_path(&record.id))
                .unwrap(),
            record
        );
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
    fn reviewer_accepts_plain_and_fenced_json() {
        let plain = r#"{"decision":"create_goal","name":"N","prompt":"P","priority":"high"}"#;
        assert!(matches!(
            parse_review_decision(plain).unwrap(),
            ReviewDecision::CreateGoal { .. }
        ));
        let fenced = "```json\n{\"decision\":\"ignore\",\"reason\":\"noise\"}\n```";
        assert_eq!(
            parse_review_decision(fenced).unwrap(),
            ReviewDecision::Ignore {
                reason: Some("noise".to_string())
            }
        );
    }

    #[test]
    fn settings_default_to_goal_address_and_inherit_agent_cli() {
        let settings = JsonObject::from_iter([
            (
                "development_request_email_enabled".to_string(),
                Value::String("1".to_string()),
            ),
            ("agent_cli".to_string(), Value::String("codex".to_string())),
        ]);
        let parsed = DevelopmentRequestSettings::from_project_settings(&settings);
        assert!(parsed.enabled);
        assert_eq!(parsed.address, "goal@getrefine.dev");
        assert_eq!(parsed.provider, "codex");
        assert!(parsed.allowed_senders.contains("bwilliams@nevo.com"));
        assert!(parsed.allowed_senders.contains("buddywilliams@gmail.com"));
        assert!(
            parsed
                .allowed_senders
                .contains("buddy.williams@insurity.com")
        );
    }
}

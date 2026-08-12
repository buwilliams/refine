use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{
    DevelopmentRequestRecord, DevelopmentRequestSettings, JMAP_SESSION_URL, MailSource,
    PROCESSED_KEYWORD,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};

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
pub(super) struct FastmailClient {
    http: Client,
    token: String,
    api_url: String,
    download_url: String,
    account_id: String,
}

impl FastmailClient {
    pub(super) fn connect(token: String) -> RefineResult<Self> {
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

    fn pending_email_ids(&self, address: &str) -> RefineResult<Vec<String>> {
        let response = self.call(
            &["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail"],
            vec![json!(["Email/query", {
                "accountId": self.account_id,
                "filter": pending_email_filter(address),
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

impl MailSource for FastmailClient {
    fn pending_email_ids(&self, address: &str) -> RefineResult<Vec<String>> {
        Self::pending_email_ids(self, address)
    }

    fn raw_email(&self, email_id: &str) -> RefineResult<Vec<u8>> {
        Self::raw_email(self, email_id)
    }

    fn mark_processed(&self, email_id: &str) -> RefineResult<()> {
        Self::mark_processed(self, email_id)
    }

    fn send_resolution(
        &self,
        settings: &DevelopmentRequestSettings,
        record: &DevelopmentRequestRecord,
    ) -> RefineResult<()> {
        Self::send_resolution(self, settings, record)
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

pub(super) fn pending_email_filter(address: &str) -> Value {
    json!({"to": address, "notKeyword": PROCESSED_KEYWORD})
}

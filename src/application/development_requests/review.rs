use super::*;

use chrono::DateTime;

use crate::application::workflow::governance::integration::FileGovernanceIntegrationService;
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::model::workflow::GoalStatus;

impl FileDevelopmentRequestService {
    pub(super) fn advance_goal_and_notify(
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
            // Read the worker node's saved setting for every decision. The Review
            // timestamp is durable even while approval is disabled or settings fail
            // to load. Done notifications below do not depend on this setting.
            let node_settings =
                FileSettingsService::with_active_root(&self.refine_dir, &self.runtime_root)
                    .load()?;
            if node_settings.get("auto_approve").and_then(Value::as_str) == Some("true")
                && now
                    .signed_duration_since(first_seen)
                    .to_std()
                    .is_ok_and(|elapsed| elapsed >= settings.auto_approve_after)
            {
                FileGovernanceIntegrationService::with_target_root(
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
}

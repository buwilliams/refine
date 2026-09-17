//! Planning authoring uses the same Goal record and ownership boundary.
use super::*;
use serde_json::json;
impl FileWorkItemService {
    /// Publish ownership and a non-executable release commitment in one Goal write.
    pub fn handoff_planning_goal(
        &self,
        id: &str,
        target: &str,
        request_id: &str,
        invocations: &Value,
    ) -> RefineResult<()> {
        let _lock = self.acquire_goal_mutation_lock(id)?;
        let current = self.show_goal_summary(id)?;
        let (path, mut goal) = self.read_goal_value_unchecked_locked(&current)?;
        if goal["planning_release"]["request_id"] == request_id {
            if goal["node_id"] != target {
                return Err(RefineError::Conflict(
                    "Planning handoff was superseded by an explicit transfer".into(),
                ));
            }
            return Ok(());
        }
        self.ensure_goal_owned(&current)?;
        if current.goal.status != GoalStatus::Backlog {
            return Err(RefineError::Conflict(
                "Planning handoff requires Backlog".into(),
            ));
        }
        self.validate_transfer_target_node(target)?;
        if current.goal.feature_id.is_some() && current.goal.node_id.as_deref() != Some(target) {
            return Err(RefineError::Conflict(
                "Transfer the Feature before routing its card".into(),
            ));
        }
        goal["planning_release"] = json!({"request_id":request_id,"source_node":self.active_node_id()?,"target_node":target,"invocations":invocations,"at":now_timestamp()});
        goal["node_id"] = json!(target);
        write_json_atomically(&path, &goal)
    }
    pub fn edit_planning_goal(&self, id: &str, data: &Value, request_id: &str) -> RefineResult<()> {
        let reporter = data["reporter"]
            .as_str()
            .map(Self::validate_goal_reporter)
            .transpose()?;
        let mutate = || {
            let (_lock, path, mut goal) = self.read_goal_value(id)?;
            if goal["planning_edit_request"] == request_id {
                return Ok(());
            }
            if data["expected_goal_revision"].as_u64()
                != Some(super::record_persistence::workflow_revision(&goal))
            {
                return Err(RefineError::Conflict(
                    "Goal changed; refresh before editing".into(),
                ));
            }
            let status = GoalStatus::parse_wire(goal["status"].as_str().unwrap_or_default())
                .ok_or_else(|| RefineError::InvalidInput("Invalid Goal status".into()))?;
            validate_goal_operation(&status, &GoalOperation::EditMetadata)?;
            if let Some(description) = data.get("description") {
                if !matches!(status, GoalStatus::Draft | GoalStatus::Backlog) {
                    return Err(RefineError::Conflict(
                        "Edit the current Round explicitly after release".into(),
                    ));
                }
                let text = description
                    .as_str()
                    .ok_or_else(|| RefineError::InvalidInput("description must be text".into()))?;
                goal["description"] = json!(text);
            }
            if let Some(name) = data.get("name") {
                let name = name
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| RefineError::InvalidInput("name is required".into()))?;
                goal["name"] = json!(name);
            }
            if let Some(priority) = data.get("priority") {
                let priority = priority
                    .as_str()
                    .and_then(GoalPriority::parse_wire)
                    .ok_or_else(|| RefineError::InvalidInput("Invalid priority".into()))?;
                goal["priority"] = json!(priority);
            }
            if let Some(reporter) = reporter {
                goal["reporter"] = json!(reporter);
            }
            goal["planning_edit_request"] = json!(request_id);
            goal["updated"] = json!(now_timestamp());
            write_json_atomically(&path, &goal)
        };
        if let Some(reporter) = reporter {
            self.with_goal_reporter_registered(reporter, mutate)
        } else {
            mutate()
        }
    }
    pub fn import_planning_goal(
        &self,
        id: &str,
        name: &str,
        reporter: Option<&str>,
        legacy: &Value,
        list_id: &str,
        item_id: &str,
    ) -> RefineResult<()> {
        crate::application::events::transitions::without_dispatch(|| {
            let origin =
                json!({"todo_list_id":list_id,"todo_item_id":item_id,"completed":legacy["done"]});
            match self.show_goal_detail(id) {
                Ok(goal) if goal["planning_origin"] == origin => return Ok(()),
                Ok(_) => {
                    return Err(RefineError::Conflict(
                        "Migration Goal ID already exists with different provenance".into(),
                    ));
                }
                Err(RefineError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
            self.create_goal_record(name, Some(id), GoalStatus::Draft, None, reporter, None,
                Some(&json!({"created":legacy["created"],"updated":legacy["updated"],"planning_origin":origin})))?;
            Ok(())
        })
    }
}

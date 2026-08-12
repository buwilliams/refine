use std::path::PathBuf;

use serde_json::json;

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::prompts::{PromptEngine, PromptTemplate, render};
use crate::tools::product::project_projection::FileProjectProjectionStore;

use super::{ChatAttachment, ChatSessionRecord, FileChatService};

impl FileChatService {
    pub(super) fn project_root(&self) -> PathBuf {
        self.refine_dir
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.refine_dir.clone())
    }

    pub(super) fn chat_cwd(&self, record: &ChatSessionRecord) -> PathBuf {
        match &record.attachment {
            ChatAttachment::Standalone => record
                .worktree
                .as_ref()
                .map(|worktree| PathBuf::from(&worktree.path))
                .unwrap_or_else(|| self.project_root()),
            _ => self.project_root(),
        }
    }

    pub(super) fn provider_path_override(&self) -> Option<String> {
        let mut paths = Vec::new();
        paths.push(self.refine_dir.join("provider-bin"));
        paths.push(self.project_root().join("node_modules/.bin"));
        if let Some(path) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&path));
        }
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            paths.push(home.join(".local/bin"));
            paths.push(home.join(".npm-global/bin"));
            paths.push(home.join(".cargo/bin"));
        }
        let joined = std::env::join_paths(paths).ok()?;
        Some(joined.to_string_lossy().to_string())
    }

    pub(super) fn chat_prompt(&self, record: &ChatSessionRecord, message: &str) -> String {
        let attachment = match &record.attachment {
            ChatAttachment::Goal(id) => format!("Goal {id}"),
            ChatAttachment::Feature(id) => format!("Feature {id}"),
            ChatAttachment::Supervisor => "supervisor agent".to_string(),
            ChatAttachment::Standalone => "standalone chat".to_string(),
        };
        let instructions = chat_mode_instructions(record);
        let context = self
            .attached_product_context(record)
            .unwrap_or_else(|error| {
                format!("Attachment context could not be rebuilt from refine records: {error}")
            });
        render(
            PromptTemplate::Chat,
            &[
                ("mode", &record.mode),
                ("attachment", &attachment),
                ("instructions", instructions),
                ("context", &context),
                ("message", message),
            ],
        )
    }

    fn attached_product_context(&self, record: &ChatSessionRecord) -> RefineResult<String> {
        let store =
            FileProjectProjectionStore::with_runtime_root(&self.refine_dir, &self.runtime_root);
        let snapshot = store.load_or_refresh_projection(&self.runtime_root.join("cache"))?;
        match &record.attachment {
            ChatAttachment::Goal(id) => {
                let Some(goal) = snapshot.goals.get(id) else {
                    return Err(RefineError::NotFound(format!("Goal {id} was not found")));
                };
                serde_json::to_string_pretty(&json!({
                    "type": "goal",
                    "id": &goal.goal.id,
                    "name": &goal.goal.name,
                    "status": &goal.goal.status,
                    "priority": &goal.goal.priority,
                    "reporter": &goal.goal.reporter,
                    "round_count": goal.goal.round_count,
                    "feature_id": &goal.goal.feature_id,
                    "node_id": &goal.goal.node_id,
                    "updated": &goal.goal.updated
                }))
            }
            ChatAttachment::Feature(id) => {
                let Some(feature) = snapshot.features.get(id) else {
                    return Err(RefineError::NotFound(format!("Feature {id} was not found")));
                };
                serde_json::to_string_pretty(&json!({
                    "type": "feature",
                    "id": &feature.feature.id,
                    "name": &feature.feature.name,
                    "status": &feature.status,
                    "goal_ids": &feature.goal_ids,
                    "rollup": &feature.rollup,
                    "updated": &feature.feature.updated
                }))
            }
            ChatAttachment::Supervisor => {
                return Err(RefineError::Conflict(
                    "Supervisor Agent sessions are retired".to_string(),
                ));
            }
            ChatAttachment::Standalone => {
                let mut context = json!({
                    "type": "standalone",
                    "description": "standalone chat; no attached product record"
                });
                if let Some(worktree) = &record.worktree {
                    context["worktree"] = json!(worktree);
                }
                serde_json::to_string_pretty(&context)
            }
        }
        .map_err(|error| {
            RefineError::Serialization(format!("failed to encode chat attachment context: {error}"))
        })
    }
}

fn chat_mode_instructions(record: &ChatSessionRecord) -> &'static str {
    if record.mode.eq_ignore_ascii_case("plan") {
        return PromptEngine::load(PromptTemplate::ChatPlan);
    }
    PromptEngine::load(match &record.attachment {
        ChatAttachment::Goal(_) => PromptTemplate::ChatGoal,
        ChatAttachment::Feature(_) => PromptTemplate::ChatFeature,
        ChatAttachment::Supervisor => PromptTemplate::ChatAgent,
        ChatAttachment::Standalone => PromptTemplate::ChatStandalone,
    })
}

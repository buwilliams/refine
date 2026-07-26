use std::path::Path;

use serde::Serialize;

use crate::model::workflow::GoalStatus;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::product::work_items::FileWorkItemService;

use super::{ImportDraft, order_feature_dependency_drafts};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ImportDuplicateActions {
    pub moved_to_backlog: usize,
    pub move_noop: usize,
    pub updated_original: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportDuplicateOutcome {
    pub draft_name: String,
    pub decision: String,
    pub original_goal_id: Option<String>,
    pub outcome: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportFeatureDestination {
    None,
    Existing(String),
    New {
        name: String,
        description: Option<String>,
        reporter: Option<String>,
        assignee: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ImportFeatureIdentity {
    pub id: String,
    pub name: String,
    pub created: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportPersistProgress {
    pub completed: usize,
    pub total: usize,
}

pub trait ImportPersistObserver {
    fn is_cancelled(&self) -> bool {
        false
    }

    fn on_progress(&mut self, _progress: ImportPersistProgress) {}

    fn on_rollback_start(&mut self, _rollback: &ImportRollbackEvidence) {}
}

impl ImportPersistObserver for () {}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ImportRollbackEvidence {
    pub created_goal_ids: Vec<String>,
    pub rolled_back_goal_ids: Vec<String>,
    pub created_feature_id: Option<String>,
    pub rolled_back_feature_id: Option<String>,
    pub rollback_failures: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportPersistFailureKind {
    Cancelled,
    Failed,
}

#[derive(Debug)]
pub struct ImportPersistFailure {
    pub kind: ImportPersistFailureKind,
    pub error: Option<RefineError>,
    /// Zero-based index into the capability input drafts. Surface adapters must
    /// convert this exactly once when their public contract uses another base.
    pub failed_draft_index_zero_based: Option<usize>,
    pub failed_name: Option<String>,
    pub duplicate_actions: ImportDuplicateActions,
    pub duplicate_outcomes: Vec<ImportDuplicateOutcome>,
    pub rollback: ImportRollbackEvidence,
}

impl ImportPersistFailure {
    pub fn into_refine_error(self) -> RefineError {
        self.error.unwrap_or_else(|| {
            RefineError::InvalidInput("import persistence was cancelled".to_string())
        })
    }
}

pub fn persist_import_drafts(
    refine_dir: &Path,
    drafts: Vec<ImportDraft>,
    destination: ImportFeatureDestination,
    observer: &mut impl ImportPersistObserver,
) -> Result<super::ImportPersistResult, ImportPersistFailure> {
    let service = FileWorkItemService::new(refine_dir);
    persist_with_service(&service, drafts, destination, observer)
}

fn persist_with_service(
    service: &FileWorkItemService,
    drafts: Vec<ImportDraft>,
    destination: ImportFeatureDestination,
    observer: &mut impl ImportPersistObserver,
) -> Result<super::ImportPersistResult, ImportPersistFailure> {
    if let Err(error) = validate_duplicate_decisions(&drafts) {
        return Err(failure_without_mutation(error, None, None));
    }
    if observer.is_cancelled() {
        return Err(cancelled_without_mutation());
    }

    let mut transaction = ImportTransaction::default();
    let feature = match resolve_feature_destination(service, destination, &mut transaction) {
        Ok(feature) => feature,
        Err(error) => {
            return Err(failure_without_mutation(
                error,
                None,
                Some("feature".to_string()),
            ));
        }
    };
    let feature_id = feature.as_ref().map(|feature| feature.id.as_str());
    let total = drafts.len();
    let mut created_drafts = Vec::new();
    let mut duplicate_actions = ImportDuplicateActions::default();
    let mut duplicate_outcomes = Vec::new();

    for (index, draft) in drafts.into_iter().enumerate() {
        if observer.is_cancelled() {
            return Err(rollback_failure(
                service,
                transaction,
                observer,
                ImportFailureContext {
                    kind: ImportPersistFailureKind::Cancelled,
                    error: None,
                    failed_draft_index_zero_based: None,
                    failed_name: None,
                    duplicate_actions,
                    duplicate_outcomes,
                },
            ));
        }
        if let Err(error) = persist_draft(
            service,
            &draft,
            feature_id,
            &mut duplicate_actions,
            &mut duplicate_outcomes,
            &mut transaction,
            &mut created_drafts,
        ) {
            return Err(rollback_failure(
                service,
                transaction,
                observer,
                ImportFailureContext {
                    kind: ImportPersistFailureKind::Failed,
                    error: Some(error),
                    failed_draft_index_zero_based: Some(index),
                    failed_name: Some(draft.name),
                    duplicate_actions,
                    duplicate_outcomes,
                },
            ));
        }
        observer.on_progress(ImportPersistProgress {
            completed: index + 1,
            total,
        });
    }

    if observer.is_cancelled() {
        return Err(rollback_failure(
            service,
            transaction,
            observer,
            ImportFailureContext {
                kind: ImportPersistFailureKind::Cancelled,
                error: None,
                failed_draft_index_zero_based: None,
                failed_name: None,
                duplicate_actions,
                duplicate_outcomes,
            },
        ));
    }
    if let Some(feature_id) = feature_id
        && let Err(error) = order_feature_dependency_drafts(service, feature_id, &created_drafts)
    {
        return Err(rollback_failure(
            service,
            transaction,
            observer,
            ImportFailureContext {
                kind: ImportPersistFailureKind::Failed,
                error: Some(error),
                failed_draft_index_zero_based: None,
                failed_name: Some("dependency ordering".to_string()),
                duplicate_actions,
                duplicate_outcomes,
            },
        ));
    }

    let goal_ids = transaction.created_goal_ids;
    Ok(super::ImportPersistResult {
        created: goal_ids.len(),
        goal_ids,
        feature_id: feature.as_ref().map(|feature| feature.id.clone()),
        feature,
        duplicate_actions,
        duplicate_outcomes,
    })
}

#[derive(Default)]
struct ImportTransaction {
    created_goal_ids: Vec<String>,
    created_feature_id: Option<String>,
}

fn resolve_feature_destination(
    service: &FileWorkItemService,
    destination: ImportFeatureDestination,
    transaction: &mut ImportTransaction,
) -> RefineResult<Option<ImportFeatureIdentity>> {
    match destination {
        ImportFeatureDestination::None => Ok(None),
        ImportFeatureDestination::Existing(feature_id) => {
            let feature = service.show_feature_summary(feature_id.trim())?;
            Ok(Some(ImportFeatureIdentity {
                id: feature.feature.id,
                name: feature.feature.name,
                created: false,
            }))
        }
        ImportFeatureDestination::New {
            name,
            description,
            reporter,
            assignee,
        } => {
            let feature = service.create_feature_summary(
                &name,
                None,
                description.as_deref(),
                reporter.as_deref(),
                assignee.as_deref(),
            )?;
            transaction.created_feature_id = Some(feature.feature.id.clone());
            Ok(Some(ImportFeatureIdentity {
                id: feature.feature.id,
                name: feature.feature.name,
                created: true,
            }))
        }
    }
}

fn validate_duplicate_decisions(drafts: &[ImportDraft]) -> RefineResult<()> {
    for draft in drafts {
        match draft.duplicate_decision.trim() {
            ""
            | "original"
            | "duplicate"
            | "move_original_to_backlog"
            | "update_original_prompt"
            | "update_original_reporter"
            | "update_original_priority" => {}
            other => {
                return Err(RefineError::InvalidInput(format!(
                    "unknown duplicate_decision: {other}"
                )));
            }
        }
    }
    Ok(())
}

fn persist_draft(
    service: &FileWorkItemService,
    draft: &ImportDraft,
    feature_id: Option<&str>,
    actions: &mut ImportDuplicateActions,
    outcomes: &mut Vec<ImportDuplicateOutcome>,
    transaction: &mut ImportTransaction,
    created_drafts: &mut Vec<(ImportDraft, String)>,
) -> RefineResult<()> {
    let decision = draft.duplicate_decision.trim();
    if !decision.is_empty()
        && decision != "original"
        && let Some(duplicate) = service.latest_round_duplicate(draft.prompt.trim())?
    {
        let duplicate_id = duplicate.id;
        let outcome = match decision {
            "duplicate" => "skipped_duplicate",
            "move_original_to_backlog" => {
                if duplicate.status == GoalStatus::Backlog || duplicate_id.is_empty() {
                    actions.move_noop += 1;
                    "move_noop"
                } else if service
                    .transition_goal_status(&duplicate_id, GoalStatus::Backlog)
                    .is_ok()
                {
                    actions.moved_to_backlog += 1;
                    "moved_to_backlog"
                } else {
                    actions.move_noop += 1;
                    "move_noop"
                }
            }
            "update_original_prompt" => {
                if !duplicate_id.is_empty() {
                    service.edit_latest_goal_round_summary(
                        &duplicate_id,
                        None,
                        None,
                        Some(&draft.prompt),
                    )?;
                    actions.updated_original += 1;
                }
                "updated_original_prompt"
            }
            "update_original_reporter" => {
                if !duplicate_id.is_empty() {
                    if let Some(reporter) = nonempty_option(&draft.reporter) {
                        service.update_goal_reporter_summary(&duplicate_id, reporter)?;
                    }
                    actions.updated_original += 1;
                }
                "updated_original_reporter"
            }
            "update_original_priority" => {
                if !duplicate_id.is_empty() {
                    service.update_goal_metadata_summary(
                        &duplicate_id,
                        None,
                        Some(&draft.priority),
                        None,
                        None,
                    )?;
                    actions.updated_original += 1;
                }
                "updated_original_priority"
            }
            _ => unreachable!("duplicate decisions were validated before persistence"),
        };
        outcomes.push(ImportDuplicateOutcome {
            draft_name: draft.name.clone(),
            decision: decision.to_string(),
            original_goal_id: (!duplicate_id.is_empty()).then_some(duplicate_id),
            outcome: outcome.to_string(),
        });
        return Ok(());
    }

    let goal = service.create_goal_summary(&draft.name, None)?;
    transaction.created_goal_ids.push(goal.goal.id.clone());
    if !draft.prompt.trim().is_empty() {
        service.append_goal_round_summary_with_assignee(
            &goal.goal.id,
            nonempty_or(&draft.reporter, "Imported"),
            draft.assignee.as_deref(),
            &draft.prompt,
        )?;
    }
    if goal.goal.priority.as_str() != draft.priority || !draft.reporter.trim().is_empty() {
        service.update_goal_metadata_summary(
            &goal.goal.id,
            None,
            (goal.goal.priority.as_str() != draft.priority).then_some(draft.priority.as_str()),
            nonempty_option(&draft.reporter),
            None,
        )?;
    }
    if let Some(feature_id) = feature_id {
        service.assign_goal_to_feature(feature_id, &goal.goal.id)?;
    }
    created_drafts.push((draft.clone(), goal.goal.id.clone()));
    if decision == "original" {
        outcomes.push(ImportDuplicateOutcome {
            draft_name: draft.name.clone(),
            decision: decision.to_string(),
            original_goal_id: None,
            outcome: "created_original".to_string(),
        });
    }
    Ok(())
}

struct ImportFailureContext {
    kind: ImportPersistFailureKind,
    error: Option<RefineError>,
    failed_draft_index_zero_based: Option<usize>,
    failed_name: Option<String>,
    duplicate_actions: ImportDuplicateActions,
    duplicate_outcomes: Vec<ImportDuplicateOutcome>,
}

fn rollback_failure(
    service: &FileWorkItemService,
    transaction: ImportTransaction,
    observer: &mut impl ImportPersistObserver,
    context: ImportFailureContext,
) -> ImportPersistFailure {
    let mut rollback = ImportRollbackEvidence {
        created_goal_ids: transaction.created_goal_ids.clone(),
        created_feature_id: transaction.created_feature_id.clone(),
        ..ImportRollbackEvidence::default()
    };
    observer.on_rollback_start(&rollback);
    for goal_id in transaction.created_goal_ids.iter().rev() {
        match service.delete_goal_record(goal_id) {
            Ok(()) => rollback.rolled_back_goal_ids.push(goal_id.clone()),
            Err(error) => rollback
                .rollback_failures
                .push(format!("Goal {goal_id}: {error}")),
        }
    }
    if let Some(feature_id) = transaction.created_feature_id {
        match service.delete_feature_record(&feature_id) {
            Ok(()) => rollback.rolled_back_feature_id = Some(feature_id),
            Err(error) => rollback
                .rollback_failures
                .push(format!("Feature {feature_id}: {error}")),
        }
    }
    ImportPersistFailure {
        kind: context.kind,
        error: context.error,
        failed_draft_index_zero_based: context.failed_draft_index_zero_based,
        failed_name: context.failed_name,
        duplicate_actions: context.duplicate_actions,
        duplicate_outcomes: context.duplicate_outcomes,
        rollback,
    }
}

fn failure_without_mutation(
    error: RefineError,
    failed_draft_index_zero_based: Option<usize>,
    failed_name: Option<String>,
) -> ImportPersistFailure {
    ImportPersistFailure {
        kind: ImportPersistFailureKind::Failed,
        error: Some(error),
        failed_draft_index_zero_based,
        failed_name,
        duplicate_actions: ImportDuplicateActions::default(),
        duplicate_outcomes: Vec::new(),
        rollback: ImportRollbackEvidence::default(),
    }
}

fn cancelled_without_mutation() -> ImportPersistFailure {
    ImportPersistFailure {
        kind: ImportPersistFailureKind::Cancelled,
        error: None,
        failed_draft_index_zero_based: None,
        failed_name: None,
        duplicate_actions: ImportDuplicateActions::default(),
        duplicate_outcomes: Vec::new(),
        rollback: ImportRollbackEvidence::default(),
    }
}

fn nonempty_option(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    let value = value.trim();
    if value.is_empty() { fallback } else { value }
}

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    FileImportService, ImportDraft, ImportFeatureDestination, ImportPersistFailureKind,
    ImportPersistObserver, ImportPersistProgress,
};
use crate::application::work_items::FileWorkItemService;
use crate::model::workflow::GoalStatus;

#[cfg(unix)]
mod rollback_failures;

#[test]
fn shared_persistence_creates_goal_with_round_and_metadata() {
    let temp_root = unique_temp_dir("import-persist-metadata");
    let refine_dir = temp_root.join(".refine");
    let result = FileImportService::new(&refine_dir)
        .persist(vec![draft("Created", "Durable prompt")], None)
        .unwrap();

    let service = FileWorkItemService::new(&refine_dir);
    let goal = service.show_goal_summary(&result.goal_ids[0]).unwrap();
    let detail = service.show_goal_detail(&result.goal_ids[0]).unwrap();
    assert_eq!(goal.goal.name, "Created");
    assert_eq!(goal.goal.priority.as_str(), "high");
    assert_eq!(goal.goal.reporter.as_deref(), Some("Reporter"));
    assert_eq!(goal.goal.assignee.as_deref(), Some("Assignee"));
    assert_eq!(goal.goal.round_count, 1);
    assert_eq!(detail["rounds"][0]["prompt"], "Durable prompt");
    assert_eq!(detail["rounds"][0]["reporter"], "Reporter");
    assert_eq!(detail["rounds"][0]["assignee"], "Assignee");
    let wire_result = serde_json::to_value(&result).unwrap();
    assert_eq!(
        wire_result
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec!["created", "feature_id", "goal_ids"]
    );

    if temp_root.exists() {
        std::fs::remove_dir_all(temp_root).unwrap();
    }
}

#[test]
fn shared_persistence_resolves_existing_and_creates_new_feature_destinations() {
    let temp_root = unique_temp_dir("import-persist-features");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Existing", Some("FEA1"), None, None, None)
        .unwrap();
    let imports = FileImportService::new(&refine_dir);

    let existing = imports
        .persist_with_destination(
            vec![draft("Existing Goal", "Existing feature prompt")],
            ImportFeatureDestination::Existing("FEA1".to_string()),
            &mut (),
        )
        .unwrap();
    assert_eq!(existing.feature_id.as_deref(), Some("FEA1"));
    assert_eq!(existing.feature.as_ref().unwrap().name, "Existing");
    assert!(!existing.feature.as_ref().unwrap().created);

    let new = imports
        .persist_with_destination(
            vec![draft("New Goal", "New feature prompt")],
            ImportFeatureDestination::New {
                name: "Created Feature".to_string(),
                description: Some("Created for this import".to_string()),
                reporter: Some("Reporter".to_string()),
                assignee: Some("Assignee".to_string()),
            },
            &mut (),
        )
        .unwrap();
    let new_feature = new.feature.unwrap();
    assert!(new_feature.created);
    assert_eq!(new_feature.name, "Created Feature");
    assert_eq!(
        service
            .show_goal_summary(&new.goal_ids[0])
            .unwrap()
            .goal
            .feature_id
            .as_deref(),
        Some(new_feature.id.as_str())
    );

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_persistence_orders_dependency_connected_goals() {
    let temp_root = unique_temp_dir("import-persist-ordering");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Feature", Some("FEA1"), None, None, None)
        .unwrap();
    let first = draft("Foundation", "Create the foundation");
    let mut second = draft("Surface", "Create the surface");
    second.dependency_names = vec!["Foundation".to_string()];
    let third = draft("Independent", "Independent work");

    let result = FileImportService::new(&refine_dir)
        .persist(vec![second, third, first], Some("FEA1"))
        .unwrap();
    let goals = result
        .goal_ids
        .iter()
        .map(|id| service.show_goal_summary(id).unwrap().goal)
        .collect::<Vec<_>>();
    let foundation = goals.iter().find(|goal| goal.name == "Foundation").unwrap();
    let surface = goals.iter().find(|goal| goal.name == "Surface").unwrap();
    let independent = goals
        .iter()
        .find(|goal| goal.name == "Independent")
        .unwrap();
    assert_eq!(foundation.feature_order, Some(1));
    assert_eq!(surface.feature_order, Some(2));
    assert_eq!(independent.feature_order, None);

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_persistence_executes_every_duplicate_decision() {
    let temp_root = unique_temp_dir("import-persist-duplicates");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    let imports = FileImportService::new(&refine_dir);

    let original_create = create_original(&service, "Original create");
    let mut create = draft("Create despite duplicate", "Original create");
    create.duplicate_decision = "original".to_string();
    let created = imports.persist(vec![create], None).unwrap();
    assert_eq!(created.created, 1);
    assert_eq!(created.duplicate_outcomes[0].outcome, "created_original");
    assert_ne!(created.goal_ids[0], original_create);

    let original_skip = create_original(&service, "Original skip");
    let mut skip = draft("Skip duplicate", "Original skip");
    skip.duplicate_decision = "duplicate".to_string();
    let skipped = imports.persist(vec![skip], None).unwrap();
    assert_eq!(skipped.created, 0);
    assert_eq!(
        skipped.duplicate_outcomes[0].original_goal_id.as_deref(),
        Some(original_skip.as_str())
    );
    assert_eq!(skipped.duplicate_outcomes[0].outcome, "skipped_duplicate");

    let original_move = create_original(&service, "Original move");
    service
        .transition_goal_status(&original_move, GoalStatus::Todo)
        .unwrap();
    let mut move_original = draft("Move duplicate", "Original move");
    move_original.duplicate_decision = "move_original_to_backlog".to_string();
    let moved = imports.persist(vec![move_original], None).unwrap();
    assert_eq!(moved.duplicate_actions.moved_to_backlog, 1);
    assert_eq!(
        service
            .show_goal_summary(&original_move)
            .unwrap()
            .goal
            .status,
        GoalStatus::Backlog
    );

    let original_prompt = create_original(&service, "Original prompt");
    let mut update_prompt = draft("Update prompt", "Original prompt");
    update_prompt.duplicate_decision = "update_original_prompt".to_string();
    let prompt_result = imports.persist(vec![update_prompt], None).unwrap();
    assert_eq!(prompt_result.duplicate_actions.updated_original, 1);
    assert_eq!(
        prompt_result.duplicate_outcomes[0]
            .original_goal_id
            .as_deref(),
        Some(original_prompt.as_str())
    );

    let original_reporter = create_original(&service, "Original reporter");
    let mut update_reporter = draft("Update reporter", "Original reporter");
    update_reporter.reporter = "New Reporter".to_string();
    update_reporter.duplicate_decision = "update_original_reporter".to_string();
    imports.persist(vec![update_reporter], None).unwrap();
    assert_eq!(
        service
            .show_goal_summary(&original_reporter)
            .unwrap()
            .goal
            .reporter
            .as_deref(),
        Some("New Reporter")
    );

    let original_priority = create_original(&service, "Original priority");
    let mut update_priority = draft("Update priority", "Original priority");
    update_priority.priority = "medium".to_string();
    update_priority.duplicate_decision = "update_original_priority".to_string();
    imports.persist(vec![update_priority], None).unwrap();
    assert_eq!(
        service
            .show_goal_summary(&original_priority)
            .unwrap()
            .goal
            .priority
            .as_str(),
        "medium"
    );

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_persistence_rejects_unknown_duplicate_decision_before_mutation() {
    let temp_root = unique_temp_dir("import-persist-unknown");
    let refine_dir = temp_root.join(".refine");
    let mut unknown = draft("Unknown", "No duplicate exists");
    unknown.duplicate_decision = "invented".to_string();

    let failure = FileImportService::new(&refine_dir)
        .persist_with_destination(vec![unknown], ImportFeatureDestination::None, &mut ())
        .unwrap_err();
    assert_eq!(failure.kind, ImportPersistFailureKind::Failed);
    assert_eq!(
        failure.error.unwrap().to_string(),
        "unknown duplicate_decision: invented"
    );
    assert!(
        FileWorkItemService::new(&refine_dir)
            .list_goal_summaries()
            .unwrap()
            .is_empty()
    );

    if temp_root.exists() {
        std::fs::remove_dir_all(temp_root).unwrap();
    }
}

#[test]
fn shared_persistence_accounts_for_failure_and_rolls_back_created_work() {
    let temp_root = unique_temp_dir("import-persist-failure");
    let refine_dir = temp_root.join(".refine");
    let valid = draft("First", "First prompt");
    let mut invalid = draft("Second", "Second prompt");
    invalid.reporter = "invalid\nreporter".to_string();

    let failure = FileImportService::new(&refine_dir)
        .persist_with_destination(
            vec![valid, invalid],
            ImportFeatureDestination::New {
                name: "Transient Feature".to_string(),
                description: None,
                reporter: None,
                assignee: None,
            },
            &mut (),
        )
        .unwrap_err();
    assert_eq!(failure.kind, ImportPersistFailureKind::Failed);
    assert_eq!(failure.failed_draft_index_zero_based, Some(1));
    assert_eq!(failure.failed_name.as_deref(), Some("Second"));
    assert_eq!(failure.rollback.created_goal_ids.len(), 2);
    assert_eq!(failure.rollback.rolled_back_goal_ids.len(), 2);
    assert_eq!(
        failure.rollback.created_feature_id,
        failure.rollback.rolled_back_feature_id
    );
    assert!(failure.rollback.rollback_failures.is_empty());
    let service = FileWorkItemService::new(&refine_dir);
    assert!(service.list_goal_summaries().unwrap().is_empty());
    assert!(service.list_feature_summaries().unwrap().is_empty());

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn shared_persistence_cancellation_rolls_back_only_import_created_records() {
    let temp_root = unique_temp_dir("import-persist-cancel");
    let refine_dir = temp_root.join(".refine");
    let service = FileWorkItemService::new(&refine_dir);
    service
        .create_feature_summary("Existing", Some("FEA1"), None, None, None)
        .unwrap();
    let preexisting_goal = service
        .create_goal_summary("Pre-existing", Some("GOAL1"))
        .unwrap()
        .goal
        .id;
    let mut observer = CancelAfterFirst::default();

    let failure = FileImportService::new(&refine_dir)
        .persist_with_destination(
            vec![
                draft("First", "First cancellation prompt"),
                draft("Second", "Second cancellation prompt"),
            ],
            ImportFeatureDestination::Existing("FEA1".to_string()),
            &mut observer,
        )
        .unwrap_err();
    assert_eq!(failure.kind, ImportPersistFailureKind::Cancelled);
    assert_eq!(failure.rollback.created_goal_ids.len(), 1);
    assert_eq!(failure.rollback.rolled_back_goal_ids.len(), 1);
    assert!(failure.rollback.created_feature_id.is_none());
    assert!(service.show_goal_summary(&preexisting_goal).is_ok());
    assert!(service.show_feature_summary("FEA1").is_ok());
    assert_eq!(service.list_goal_summaries().unwrap().len(), 1);
    assert_eq!(
        observer.progress,
        vec![ImportPersistProgress {
            completed: 1,
            total: 2
        }]
    );

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[derive(Default)]
struct CancelAfterFirst {
    progress: Vec<ImportPersistProgress>,
    cancelled: bool,
}

impl ImportPersistObserver for CancelAfterFirst {
    fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    fn on_progress(&mut self, progress: ImportPersistProgress) {
        self.progress.push(progress);
        self.cancelled = true;
    }
}

fn create_original(service: &FileWorkItemService, prompt: &str) -> String {
    let goal = service.create_goal_summary(prompt, None).unwrap();
    service
        .append_goal_round_summary(&goal.goal.id, "Original Reporter", prompt)
        .unwrap();
    goal.goal.id
}

fn draft(name: &str, prompt: &str) -> ImportDraft {
    ImportDraft {
        name: name.to_string(),
        prompt: prompt.to_string(),
        reporter: "Reporter".to_string(),
        assignee: Some("Assignee".to_string()),
        priority: "high".to_string(),
        duplicate_decision: String::new(),
        dependency_names: Vec::new(),
    }
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

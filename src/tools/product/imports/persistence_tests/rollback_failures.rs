use super::*;
use crate::tools::product::imports::ImportRollbackEvidence;

#[cfg(unix)]
#[test]
fn shared_persistence_never_drops_injected_goal_deletion_rollback_failure() {
    let temp_root = unique_temp_dir("import-persist-goal-rollback-failure");
    let refine_dir = temp_root.join(".refine");
    let mut observer = FailRollbackAfterFirst::new(refine_dir.clone(), RollbackFailureTarget::Goal);

    let failure = FileImportService::new(&refine_dir)
        .persist_with_destination(
            vec![
                draft("First", "First cancellation prompt"),
                draft("Second", "Second cancellation prompt"),
            ],
            ImportFeatureDestination::None,
            &mut observer,
        )
        .unwrap_err();
    observer.restore_permissions();

    assert_eq!(failure.kind, ImportPersistFailureKind::Cancelled);
    assert_eq!(failure.rollback.created_goal_ids.len(), 1);
    assert!(failure.rollback.rolled_back_goal_ids.is_empty());
    assert_eq!(failure.rollback.rollback_failures.len(), 1);
    assert!(
        failure.rollback.rollback_failures[0].contains("Goal"),
        "{:?}",
        failure.rollback
    );
    assert!(
        FileWorkItemService::new(&refine_dir)
            .show_goal_summary(&failure.rollback.created_goal_ids[0])
            .is_ok(),
        "a failed rollback must retain an inspectable unrecovered Goal"
    );

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn shared_persistence_never_drops_injected_feature_deletion_rollback_failure() {
    let temp_root = unique_temp_dir("import-persist-feature-rollback-failure");
    let refine_dir = temp_root.join(".refine");
    let mut observer =
        FailRollbackAfterFirst::new(refine_dir.clone(), RollbackFailureTarget::Feature);

    let failure = FileImportService::new(&refine_dir)
        .persist_with_destination(
            vec![
                draft("First", "First cancellation prompt"),
                draft("Second", "Second cancellation prompt"),
            ],
            ImportFeatureDestination::New {
                name: "Unrecovered Feature".to_string(),
                description: None,
                reporter: None,
                assignee: None,
            },
            &mut observer,
        )
        .unwrap_err();
    observer.restore_permissions();

    assert_eq!(failure.kind, ImportPersistFailureKind::Cancelled);
    assert_eq!(failure.rollback.created_goal_ids.len(), 1);
    assert_eq!(failure.rollback.rolled_back_goal_ids.len(), 1);
    assert_eq!(failure.rollback.rollback_failures.len(), 1);
    assert!(
        failure.rollback.rollback_failures[0].contains("Feature"),
        "{:?}",
        failure.rollback
    );
    let feature_id = failure.rollback.created_feature_id.as_deref().unwrap();
    assert!(failure.rollback.rolled_back_feature_id.is_none());
    assert!(
        FileWorkItemService::new(&refine_dir)
            .show_feature_summary(feature_id)
            .is_ok(),
        "a failed rollback must retain an inspectable unrecovered Feature"
    );

    std::fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum RollbackFailureTarget {
    Goal,
    Feature,
}

#[cfg(unix)]
struct FailRollbackAfterFirst {
    refine_dir: PathBuf,
    target: RollbackFailureTarget,
    cancelled: bool,
    locked_directories: Vec<PathBuf>,
}

#[cfg(unix)]
impl FailRollbackAfterFirst {
    fn new(refine_dir: PathBuf, target: RollbackFailureTarget) -> Self {
        Self {
            refine_dir,
            target,
            cancelled: false,
            locked_directories: Vec::new(),
        }
    }

    fn restore_permissions(&self) {
        use std::os::unix::fs::PermissionsExt;

        for path in &self.locked_directories {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

#[cfg(unix)]
impl ImportPersistObserver for FailRollbackAfterFirst {
    fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    fn on_progress(&mut self, _progress: ImportPersistProgress) {
        self.cancelled = true;
    }

    fn on_rollback_start(&mut self, rollback: &ImportRollbackEvidence) {
        use std::os::unix::fs::PermissionsExt;

        let record_path = match self.target {
            RollbackFailureTarget::Goal => {
                let goal_id = &rollback.created_goal_ids[0];
                self.refine_dir
                    .join("goals")
                    .join(&goal_id[..2])
                    .join(&goal_id[2..])
                    .join("goal.json")
            }
            RollbackFailureTarget::Feature => {
                let feature_id = rollback.created_feature_id.as_deref().unwrap();
                self.refine_dir
                    .join("features")
                    .join(&feature_id[..2])
                    .join(&feature_id[2..])
                    .join("feature.json")
            }
        };
        let directory = record_path.parent().unwrap().to_path_buf();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555)).unwrap();
        self.locked_directories.push(directory);
    }
}

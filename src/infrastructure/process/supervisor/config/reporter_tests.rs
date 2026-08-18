use super::reporters::ReporterCascadeTestStage;
use super::*;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::application::projects::projection::FileProjectProjectionStore;
use crate::application::work_items::{FileWorkItemService, GoalAuthoringRequest};

#[test]
fn goal_creation_and_reporter_rename_are_one_linearizable_operation() {
    let temp_root = unique_temp_dir("reporter-create-rename-race");
    let refine_dir = temp_root.join(".refine");
    let reporters = FileReporterService::new(&refine_dir);
    let old_id = reporters.create("Original").unwrap()["reporter"]["id"]
        .as_u64()
        .unwrap();
    let (registered_tx, registered_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let release_rx = Arc::new(Mutex::new(release_rx));
    let paused = Arc::new(AtomicBool::new(false));
    let paused_for_hook = Arc::clone(&paused);
    let snapshot = FileProjectProjectionStore::new(&refine_dir)
        .rebuild_projection()
        .unwrap();
    let create_service = FileWorkItemService::new(&refine_dir)
        .with_after_reporter_registration_hook(move || {
            if !paused_for_hook.swap(true, Ordering::SeqCst) {
                registered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        });
    let creator = std::thread::spawn(move || {
        create_service.author_goal_from_projection(
            GoalAuthoringRequest {
                id: Some("GOALCREATE".to_string()),
                name: Some("Created concurrently".to_string()),
                prompt: "Create it".to_string(),
                reporter: "Original".to_string(),
                priority: "low".to_string(),
                ..GoalAuthoringRequest::default()
            },
            &snapshot,
        )
    });
    registered_rx.recv().unwrap();

    let (renamed_tx, renamed_rx) = mpsc::channel();
    let rename_dir = refine_dir.clone();
    let renamer = std::thread::spawn(move || {
        let result = FileReporterService::new(rename_dir).rename(old_id, "Renamed");
        renamed_tx.send(result).unwrap();
    });
    assert!(
        renamed_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "rename must wait until the dependent Goal creation is durable"
    );
    release_tx.send(()).unwrap();
    creator.join().unwrap().unwrap();
    renamed_rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    renamer.join().unwrap();

    let goal = read_goal(&refine_dir, "GOALCREATE");
    assert_eq!(goal["reporter"], "Renamed");
    assert_eq!(goal["rounds"][0]["reporter"], "Renamed");
    assert_eq!(
        reporter_names(&reporters),
        BTreeSet::from(["Renamed".to_string()])
    );
    assert!(!refine_dir.join(REPORTER_CASCADE_FILE).exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn goal_update_and_reporter_removal_are_linearizable_and_removal_stays_explicit() {
    let temp_root = unique_temp_dir("reporter-update-remove-race");
    let refine_dir = temp_root.join(".refine");
    let base_service = FileWorkItemService::new(&refine_dir);
    base_service
        .create_goal_summary("Updated concurrently", Some("GOALUPDATE"))
        .unwrap();
    let reporters = FileReporterService::new(&refine_dir);
    let removed_id = reporters.create("Removed").unwrap()["reporter"]["id"]
        .as_u64()
        .unwrap();
    let (registered_tx, registered_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let release_rx = Arc::new(Mutex::new(release_rx));
    let paused = Arc::new(AtomicBool::new(false));
    let paused_for_hook = Arc::clone(&paused);
    let update_service = FileWorkItemService::new(&refine_dir)
        .with_after_reporter_registration_hook(move || {
            if !paused_for_hook.swap(true, Ordering::SeqCst) {
                registered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        });
    let updater = std::thread::spawn(move || {
        update_service.update_goal_reporter_summary("GOALUPDATE", "Removed")
    });
    registered_rx.recv().unwrap();

    let (removed_tx, removed_rx) = mpsc::channel();
    let remove_dir = refine_dir.clone();
    let remover = std::thread::spawn(move || {
        let result = FileReporterService::new(remove_dir).delete(removed_id);
        removed_tx.send(result).unwrap();
    });
    assert!(
        removed_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "removal must wait until the dependent Goal update is durable"
    );
    release_tx.send(()).unwrap();
    updater.join().unwrap().unwrap();
    removed_rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    remover.join().unwrap();

    assert_eq!(read_goal(&refine_dir, "GOALUPDATE")["reporter"], "Removed");
    assert!(
        reporter_names(&reporters).is_empty(),
        "an intentionally empty dropdown must not be reseeded from history"
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn reporter_rename_and_unrelated_goal_mutation_share_the_canonical_goal_lock() {
    let temp_root = unique_temp_dir("reporter-rename-goal-mutation-race");
    let refine_dir = temp_root.join(".refine");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Original name", Some("GOALLOCK"))
        .unwrap();
    work_items
        .update_goal_reporter_summary("GOALLOCK", "Original reporter")
        .unwrap();
    let reporters = FileReporterService::new(&refine_dir);
    let old_id = reporter_id(&reporters, "Original reporter");
    let (locked_tx, locked_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);
    let release_rx = Arc::new(Mutex::new(release_rx));
    let paused = Arc::new(AtomicBool::new(false));
    let paused_for_hook = Arc::clone(&paused);
    let hooked = FileReporterService::with_cascade_hook(
        &refine_dir,
        Arc::new(move |stage| {
            if matches!(stage, ReporterCascadeTestStage::RecordLocked(_))
                && !paused_for_hook.swap(true, Ordering::SeqCst)
            {
                locked_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        }),
    );
    let renamer = std::thread::spawn(move || hooked.rename(old_id, "Renamed reporter"));
    locked_rx.recv().unwrap();

    let (mutation_tx, mutation_rx) = mpsc::channel();
    let mutation_dir = refine_dir.clone();
    let mutation = std::thread::spawn(move || {
        let result = FileWorkItemService::new(mutation_dir).update_goal_metadata_summary(
            "GOALLOCK",
            Some("Unrelated name mutation"),
            None,
            None,
            None,
        );
        mutation_tx.send(result).unwrap();
    });
    assert!(
        mutation_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "the unrelated mutation must wait while rename owns the Goal record"
    );
    release_tx.send(()).unwrap();
    renamer.join().unwrap().unwrap();
    mutation_rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    mutation.join().unwrap();

    let goal = read_goal(&refine_dir, "GOALLOCK");
    assert_eq!(goal["name"], "Unrelated name mutation");
    assert_eq!(goal["reporter"], "Renamed reporter");
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn reporter_write_failure_does_not_publish_the_goal_reference() {
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir("reporter-write-failure");
    let refine_dir = temp_root.join(".refine");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Write failure", Some("GOALREPORTERFAIL"))
        .unwrap();
    FileReporterService::new(&refine_dir)
        .create("Existing")
        .unwrap();
    let original_mode = fs::metadata(&refine_dir).unwrap().permissions().mode();
    fs::set_permissions(&refine_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = work_items.update_goal_reporter_summary("GOALREPORTERFAIL", "Not persisted");
    fs::set_permissions(&refine_dir, fs::Permissions::from_mode(original_mode)).unwrap();

    assert!(result.is_err());
    assert!(read_goal(&refine_dir, "GOALREPORTERFAIL")["reporter"].is_null());
    assert_eq!(
        reporter_names(&FileReporterService::new(&refine_dir)),
        BTreeSet::from(["Existing".to_string()])
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn goal_write_failure_retains_the_registered_reporter_for_retry() {
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir("goal-write-failure");
    let refine_dir = temp_root.join(".refine");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Write failure", Some("GOALWRITEFAIL"))
        .unwrap();
    let goal_dir = refine_dir.join("goals/GO/ALWRITEFAIL");
    let original_mode = fs::metadata(&goal_dir).unwrap().permissions().mode();
    fs::set_permissions(&goal_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = work_items.update_goal_reporter_summary("GOALWRITEFAIL", "Recoverable");
    fs::set_permissions(&goal_dir, fs::Permissions::from_mode(original_mode)).unwrap();

    assert!(result.is_err());
    assert!(read_goal(&refine_dir, "GOALWRITEFAIL")["reporter"].is_null());
    assert_eq!(
        reporter_names(&FileReporterService::new(&refine_dir)),
        BTreeSet::from(["Recoverable".to_string()])
    );
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn rename_reporter_write_failure_keeps_every_published_name_recoverable() {
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir("rename-reporter-write-failure");
    let refine_dir = temp_root.join(".refine");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Rename write failure", Some("GOALRENAMEFAIL"))
        .unwrap();
    work_items
        .update_goal_reporter_summary("GOALRENAMEFAIL", "Original")
        .unwrap();
    let reporters = FileReporterService::new(&refine_dir);
    let old_id = reporter_id(&reporters, "Original");
    let original_mode = fs::metadata(&refine_dir).unwrap().permissions().mode();
    let changed_permissions = Arc::new(AtomicBool::new(false));
    let changed_for_hook = Arc::clone(&changed_permissions);
    let rename_dir = refine_dir.clone();
    let hooked = FileReporterService::with_cascade_hook(
        &refine_dir,
        Arc::new(move |stage| {
            if matches!(stage, ReporterCascadeTestStage::AfterRecord(_))
                && !changed_for_hook.swap(true, Ordering::SeqCst)
            {
                fs::set_permissions(&rename_dir, fs::Permissions::from_mode(0o555)).unwrap();
            }
        }),
    );
    let result = hooked.rename(old_id, "Recovered");
    fs::set_permissions(&refine_dir, fs::Permissions::from_mode(original_mode)).unwrap();

    assert!(result.is_err());
    assert_eq!(
        read_goal(&refine_dir, "GOALRENAMEFAIL")["reporter"],
        "Recovered"
    );
    assert_eq!(
        reporter_names_from_disk(&refine_dir),
        BTreeSet::from(["Original".to_string(), "Recovered".to_string()])
    );
    assert!(refine_dir.join(REPORTER_CASCADE_FILE).exists());
    assert_eq!(
        reporter_names(&reporters),
        BTreeSet::from(["Recovered".to_string()])
    );
    assert!(!refine_dir.join(REPORTER_CASCADE_FILE).exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[cfg(unix)]
#[test]
fn merge_goal_write_failure_retains_source_and_target_until_recovery() {
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir("merge-goal-write-failure");
    let refine_dir = temp_root.join(".refine");
    let work_items = FileWorkItemService::new(&refine_dir);
    work_items
        .create_goal_summary("Merge write failure", Some("GOALMERGEFAIL"))
        .unwrap();
    work_items
        .update_goal_reporter_summary("GOALMERGEFAIL", "Source")
        .unwrap();
    let reporters = FileReporterService::new(&refine_dir);
    let source_id = reporter_id(&reporters, "Source");
    let target_id = reporters.create("Target").unwrap()["reporter"]["id"]
        .as_u64()
        .unwrap();
    let goal_dir = refine_dir.join("goals/GO/ALMERGEFAIL");
    let original_mode = fs::metadata(&goal_dir).unwrap().permissions().mode();
    fs::set_permissions(&goal_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = reporters.merge(source_id, target_id);
    fs::set_permissions(&goal_dir, fs::Permissions::from_mode(original_mode)).unwrap();

    assert!(result.is_err());
    assert_eq!(
        read_goal(&refine_dir, "GOALMERGEFAIL")["reporter"],
        "Source"
    );
    assert_eq!(
        reporter_names_from_disk(&refine_dir),
        BTreeSet::from(["Source".to_string(), "Target".to_string()])
    );
    assert!(refine_dir.join(REPORTER_CASCADE_FILE).exists());
    assert_eq!(
        reporter_names(&reporters),
        BTreeSet::from(["Target".to_string()])
    );
    assert_eq!(
        read_goal(&refine_dir, "GOALMERGEFAIL")["reporter"],
        "Target"
    );
    assert!(!refine_dir.join(REPORTER_CASCADE_FILE).exists());
    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn interrupted_rename_keeps_both_names_registered_and_recovers_idempotently() {
    let temp_root = unique_temp_dir("reporter-interrupted-cascade");
    let refine_dir = temp_root.join(".refine");
    let work_items = FileWorkItemService::new(&refine_dir);
    for id in ["GOALINTERRUPT1", "GOALINTERRUPT2"] {
        work_items
            .create_goal_summary("Interrupted", Some(id))
            .unwrap();
        work_items
            .update_goal_reporter_summary(id, "Original")
            .unwrap();
    }
    let reporters = FileReporterService::new(&refine_dir);
    let old_id = reporter_id(&reporters, "Original");
    let writes = Arc::new(AtomicUsize::new(0));
    let writes_for_hook = Arc::clone(&writes);
    let hooked = FileReporterService::with_cascade_hook(
        &refine_dir,
        Arc::new(move |stage| {
            if matches!(stage, ReporterCascadeTestStage::AfterRecord(_))
                && writes_for_hook.fetch_add(1, Ordering::SeqCst) == 0
            {
                panic!("injected interruption after the first cascade write");
            }
        }),
    );
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        hooked.rename(old_id, "Recovered")
    }));
    assert!(interrupted.is_err());
    assert!(refine_dir.join(REPORTER_CASCADE_FILE).exists());
    assert_eq!(
        reporter_names_from_disk(&refine_dir),
        BTreeSet::from(["Original".to_string(), "Recovered".to_string()])
    );
    let partial_names = ["GOALINTERRUPT1", "GOALINTERRUPT2"]
        .into_iter()
        .map(|id| {
            read_goal(&refine_dir, id)["reporter"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        partial_names,
        BTreeSet::from(["Original".to_string(), "Recovered".to_string()])
    );

    let recovered = reporters.list().unwrap();
    assert_eq!(
        recovered["reporters"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|reporter| reporter["name"].as_str())
            .collect::<Vec<_>>(),
        vec!["Recovered"]
    );
    assert!(!refine_dir.join(REPORTER_CASCADE_FILE).exists());
    for id in ["GOALINTERRUPT1", "GOALINTERRUPT2"] {
        assert_eq!(read_goal(&refine_dir, id)["reporter"], "Recovered");
    }
    fs::remove_dir_all(temp_root).unwrap();
}

fn reporter_id(service: &FileReporterService, name: &str) -> u64 {
    service.list().unwrap()["reporters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|reporter| reporter["name"] == name)
        .and_then(|reporter| reporter["id"].as_u64())
        .unwrap()
}

fn reporter_names(service: &FileReporterService) -> BTreeSet<String> {
    service.list().unwrap()["reporters"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|reporter| reporter["name"].as_str().map(str::to_string))
        .collect()
}

fn reporter_names_from_disk(refine_dir: &Path) -> BTreeSet<String> {
    serde_json::from_slice::<Value>(&fs::read(refine_dir.join(REPORTERS_FILE)).unwrap())
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|reporter| reporter["name"].as_str().map(str::to_string))
        .collect()
}

fn read_goal(refine_dir: &Path, id: &str) -> Value {
    let id = id.to_uppercase();
    let path = refine_dir
        .join("goals")
        .join(&id[..2])
        .join(&id[2..])
        .join("goal.json");
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

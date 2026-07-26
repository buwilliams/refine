use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn reporter_scoped_todo_lists_persist_edit_done_undo_and_delete() {
    let temp_root = unique_temp_dir("todo-service");
    let refine_dir = temp_root.join(".refine");
    let service = FileTodoService::new(&refine_dir);

    let created = service.create_list("Buddy", "Release").unwrap();
    let list_id = created["list"]["id"].as_str().unwrap().to_string();
    service.create_list("Alex", "Release").unwrap();
    let added = service
        .add_item("Buddy", &list_id, "Verify the candidate")
        .unwrap();
    let item_id = added["item"]["id"].as_str().unwrap().to_string();

    let done = service
        .update_item("Buddy", &list_id, &item_id, None, Some(true))
        .unwrap();
    assert_eq!(done["item"]["done"], true);
    let edited = service
        .update_item(
            "Buddy",
            &list_id,
            &item_id,
            Some("Verify exact results"),
            Some(false),
        )
        .unwrap();
    assert_eq!(edited["item"]["text"], "Verify exact results");
    assert_eq!(edited["item"]["done"], false);

    let buddy = service.list("Buddy").unwrap();
    assert_eq!(buddy["lists"].as_array().unwrap().len(), 1);
    assert_eq!(buddy["lists"][0]["items"][0]["done"], false);
    assert_eq!(
        service.list("Alex").unwrap()["lists"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(refine_dir.join(TODO_LISTS_FILE).exists());

    service.delete_item("Buddy", &list_id, &item_id).unwrap();
    assert!(
        service.list("Buddy").unwrap()["lists"][0]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    service.delete_list("Buddy", &list_id).unwrap();
    assert!(
        service.list("Buddy").unwrap()["lists"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

#[test]
fn todo_mutations_enforce_reporter_ownership_and_valid_input() {
    let temp_root = unique_temp_dir("todo-service-validation");
    let service = FileTodoService::new(temp_root.join(".refine"));
    let created = service.create_list("Buddy", "Personal").unwrap();
    let list_id = created["list"]["id"].as_str().unwrap();

    assert!(service.add_item("Alex", list_id, "No access").is_err());
    assert!(service.create_list("Buddy", " personal ").is_err());
    assert!(service.create_list("", "Missing reporter").is_err());
    assert!(
        service
            .update_item("Buddy", list_id, "missing", None, None)
            .is_err()
    );

    service.create_list("Alex", "Personal").unwrap();
    service.reassign_reporter("Buddy", "Alex").unwrap();
    let merged = service.list("Alex").unwrap();
    assert_eq!(merged["lists"].as_array().unwrap().len(), 2);
    assert_eq!(merged["lists"][0]["name"], "Personal (2)");

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{label}-{stamp}"))
}

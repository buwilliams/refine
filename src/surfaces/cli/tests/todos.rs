use super::*;

#[test]
fn todo_commands_dispatch_through_the_shared_file_todo_service() {
    let temp_root = unique_temp_dir("cli-todos");
    fs::create_dir_all(&temp_root).unwrap();
    let target_root = temp_root.to_str().unwrap();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "todo",
            "create-list",
            "Release",
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ])
        .unwrap(),
    )
    .unwrap();

    let service = FileTodoService::new(temp_root.join(".refine"));
    let initial = service.list("Buddy").unwrap();
    let list_id = initial["lists"][0]["id"].as_str().unwrap().to_string();

    dispatch(
        Cli::try_parse_from([
            "refine",
            "todo",
            "add",
            &list_id,
            "Verify candidate",
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ])
        .unwrap(),
    )
    .unwrap();
    let added = service.list("Buddy").unwrap();
    let item_id = added["lists"][0]["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    for argv in [
        vec![
            "refine",
            "todo",
            "edit",
            &list_id,
            &item_id,
            "Verify exact results",
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ],
        vec![
            "refine",
            "todo",
            "done",
            &list_id,
            &item_id,
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ],
        vec![
            "refine",
            "todo",
            "undo",
            &list_id,
            &item_id,
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ],
        vec![
            "refine",
            "todo",
            "rename-list",
            &list_id,
            "Ready for review",
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ],
        vec![
            "refine",
            "todo",
            "list",
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ],
    ] {
        dispatch(Cli::try_parse_from(argv).unwrap()).unwrap();
    }

    let updated = service.list("Buddy").unwrap();
    assert_eq!(updated["lists"][0]["name"], "Ready for review");
    assert_eq!(
        updated["lists"][0]["items"][0]["text"],
        "Verify exact results"
    );
    assert_eq!(updated["lists"][0]["items"][0]["done"], false);

    dispatch(
        Cli::try_parse_from([
            "refine",
            "todo",
            "delete",
            &list_id,
            &item_id,
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ])
        .unwrap(),
    )
    .unwrap();
    dispatch(
        Cli::try_parse_from([
            "refine",
            "todo",
            "delete-list",
            &list_id,
            "--reporter",
            "Buddy",
            "--target-root",
            target_root,
        ])
        .unwrap(),
    )
    .unwrap();
    assert!(
        service.list("Buddy").unwrap()["lists"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    fs::remove_dir_all(temp_root).unwrap();
}

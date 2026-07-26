use super::super::*;

pub(crate) fn todo_commands_share_reporter_scoped_api_capability(fixture: &IntegrationFixture) {
    let created = fixture.run_refine(&[
        "todo",
        "create-list",
        "Release",
        "--reporter",
        "CLI Reporter",
    ]);
    fixture.assert_success("todo create-list", &created);
    let created = fixture.json_stdout(&created);
    let list_id = created["list"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["list"]["reporter"], "CLI Reporter");

    let added = fixture.run_refine(&[
        "todo",
        "add",
        &list_id,
        "Verify candidate",
        "--reporter",
        "CLI Reporter",
    ]);
    fixture.assert_success("todo add", &added);
    let added = fixture.json_stdout(&added);
    let item_id = added["item"]["id"].as_str().unwrap().to_string();

    for (label, args, expected_done) in [
        (
            "todo done",
            vec![
                "todo",
                "done",
                &list_id,
                &item_id,
                "--reporter",
                "CLI Reporter",
            ],
            true,
        ),
        (
            "todo undo",
            vec![
                "todo",
                "undo",
                &list_id,
                &item_id,
                "--reporter",
                "CLI Reporter",
            ],
            false,
        ),
    ] {
        let output = fixture.run_refine(&args);
        fixture.assert_success(label, &output);
        assert_eq!(
            fixture.json_stdout(&output)["item"]["done"],
            expected_done,
            "{label}"
        );
    }

    let edited = fixture.run_refine(&[
        "todo",
        "edit",
        &list_id,
        &item_id,
        "Verify exact results",
        "--reporter",
        "CLI Reporter",
    ]);
    fixture.assert_success("todo edit", &edited);
    assert_eq!(
        fixture.json_stdout(&edited)["item"]["text"],
        "Verify exact results"
    );

    let renamed = fixture.run_refine(&[
        "todo",
        "rename-list",
        &list_id,
        "Ready for review",
        "--reporter",
        "CLI Reporter",
    ]);
    fixture.assert_success("todo rename-list", &renamed);
    assert_eq!(
        fixture.json_stdout(&renamed)["list"]["name"],
        "Ready for review"
    );

    let listed = fixture.run_refine(&["todo", "list", "--reporter", "CLI Reporter"]);
    fixture.assert_success("todo list", &listed);
    let listed = fixture.json_stdout(&listed);
    assert_eq!(listed["lists"][0]["id"], list_id);
    assert_eq!(listed["lists"][0]["items"][0]["id"], item_id);
    let api_listed = fixture.api_json("GET", "/api/todos?reporter=CLI%20Reporter", json!({}));
    assert_eq!(listed, api_listed);

    let other = fixture.run_refine(&["todo", "list", "--reporter", "Other Reporter"]);
    fixture.assert_success("todo list reporter isolation", &other);
    assert!(
        fixture.json_stdout(&other)["lists"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let deleted = fixture.run_refine(&[
        "todo",
        "delete",
        &list_id,
        &item_id,
        "--reporter",
        "CLI Reporter",
    ]);
    fixture.assert_success("todo delete", &deleted);
    assert!(
        fixture.json_stdout(&deleted)["lists"][0]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let deleted = fixture.run_refine(&[
        "todo",
        "delete-list",
        &list_id,
        "--reporter",
        "CLI Reporter",
    ]);
    fixture.assert_success("todo delete-list", &deleted);
    assert!(
        fixture.json_stdout(&deleted)["lists"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

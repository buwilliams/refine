use super::super::*;

pub(crate) fn feature_create_membership_rollup_and_delete(fixture: &IntegrationFixture) {
    let feature = fixture.run_refine(&["feature", "create", "cli surface feature"]);
    fixture.assert_success("feature create", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let goal_id = fixture.create_goal("feature member goal");

    let list = fixture.run_refine(&["feature", "list"]);
    fixture.assert_success("feature list", &list);
    assert!(feature_entry(&fixture.json_stdout(&list), &feature_id).is_some());

    let add = fixture.run_refine(&["feature", "add-goal", &feature_id, &goal_id]);
    fixture.assert_success("feature add-goal", &add);
    let list = fixture.run_refine(&["feature", "list"]);
    let payload = fixture.json_stdout(&list);
    let entry = feature_entry(&payload, &feature_id).expect("feature should be listed");
    assert!(
        entry["goal_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String(goal_id.clone()))
    );
    assert_eq!(entry["rollup"]["goal_count"], 1);

    let remove = fixture.run_refine(&["feature", "remove-goal", &feature_id, &goal_id]);
    fixture.assert_success("feature remove-goal", &remove);
    let list = fixture.run_refine(&["feature", "list"]);
    let payload = fixture.json_stdout(&list);
    let entry = feature_entry(&payload, &feature_id).expect("feature should be listed");
    assert!(
        !entry["goal_ids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::Value::String(goal_id.clone()))
    );

    let delete_goal = fixture.run_refine(&["goal", "delete", &goal_id]);
    fixture.assert_success("goal delete feature member", &delete_goal);
    let delete_feature = fixture.run_refine(&["feature", "delete", &feature_id]);
    fixture.assert_success("feature delete", &delete_feature);
}

pub(crate) fn feature_show_edit_reorder_move_cancel_and_import(fixture: &IntegrationFixture) {
    let feature = fixture.run_refine(&[
        "feature",
        "create",
        "cli extended feature",
        "--description",
        "Initial description",
        "--reporter",
        "refine-smoke",
    ]);
    fixture.assert_success("feature create extended", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let first_goal = fixture.create_goal("feature reorder first goal");
    let second_goal = fixture.create_goal("feature reorder second goal");

    let show = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show", &show);
    assert_eq!(fixture.json_stdout(&show)["feature"]["id"], feature_id);

    let edit = fixture.run_refine(&[
        "feature",
        "edit",
        &feature_id,
        "--name",
        "cli extended feature renamed",
        "--description",
        "Edited description",
        "--reporter",
        "refine-smoke",
    ]);
    fixture.assert_success("feature edit", &edit);
    let shown = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after edit", &shown);
    let shown_payload = fixture.json_stdout(&shown);
    assert_eq!(
        shown_payload["feature"]["name"],
        "cli extended feature renamed"
    );
    assert_eq!(
        shown_payload["feature"]["description"],
        "Edited description"
    );

    fixture.assert_success(
        "feature add first goal",
        &fixture.run_refine(&["feature", "add-goal", &feature_id, &first_goal]),
    );
    fixture.assert_success(
        "feature add second goal",
        &fixture.run_refine(&["feature", "add-goal", &feature_id, &second_goal]),
    );
    fixture.assert_success(
        "feature order first goal",
        &fixture.run_refine(&["feature", "order-goal", &feature_id, &first_goal]),
    );
    fixture.assert_success(
        "feature order second goal",
        &fixture.run_refine(&["feature", "order-goal", &feature_id, &second_goal]),
    );
    let reorder = fixture.run_refine(&["feature", "reorder-goal", &feature_id, &second_goal, "1"]);
    fixture.assert_success("feature reorder-goal", &reorder);
    let reordered = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after reorder", &reordered);
    let goal_ids = reordered_goal_ids(&fixture.json_stdout(&reordered));
    assert_eq!(
        goal_ids.first().map(String::as_str),
        Some(second_goal.as_str())
    );

    let move_todo = fixture.run_refine(&["feature", "move", &feature_id, "todo"]);
    fixture.assert_success("feature move todo", &move_todo);
    let moved = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after move", &moved);
    assert_eq!(fixture.json_stdout(&moved)["feature"]["status"], "todo");

    let cancel = fixture.run_refine(&["feature", "cancel", &feature_id]);
    fixture.assert_success("feature cancel", &cancel);
    let cancelled = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show after cancel", &cancelled);
    assert_eq!(fixture.json_stdout(&cancelled)["feature"]["status"], "done");

    let import = fixture.run_refine(&[
        "feature",
        "import",
        "--csv",
        "--text",
        "prompt,priority\nimplement imported goal,low\n",
        "--reporter",
        "refine-smoke",
        "--feature-id",
        &feature_id,
    ]);
    fixture.assert_success("feature import csv", &import);
    let import_payload = fixture.json_stdout(&import);
    assert_eq!(import_payload["count"], 1, "{import_payload:#}");

    for goal_id in [first_goal, second_goal] {
        let _ = fixture.run_refine(&["goal", "delete", &goal_id]);
    }
    let imported_ids = import_payload["goals"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|goal| goal["id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    for goal_id in imported_ids {
        let _ = fixture.run_refine(&["goal", "delete", &goal_id]);
    }
    fixture.assert_success(
        "feature delete extended",
        &fixture.run_refine(&["feature", "delete", &feature_id]),
    );
}

pub(crate) fn reordered_goal_ids(payload: &serde_json::Value) -> Vec<String> {
    let Some(goal_ids) = payload["goal_ids"]
        .as_array()
        .or_else(|| payload["feature"]["goal_ids"].as_array())
    else {
        return Vec::new();
    };
    goal_ids
        .iter()
        .filter_map(|goal_id| goal_id.as_str().map(str::to_string))
        .collect()
}

pub(crate) fn feature_entry<'a>(
    payload: &'a serde_json::Value,
    feature_id: &str,
) -> Option<&'a serde_json::Value> {
    payload["features"]
        .as_array()?
        .iter()
        .find(|entry| entry["feature"]["id"].as_str() == Some(feature_id))
}

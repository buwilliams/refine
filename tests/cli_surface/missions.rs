use super::super::*;

pub(crate) fn mission_create_list_show_start_cancel(fixture: &IntegrationFixture) {
    let create = fixture.run_refine(&[
        "mission",
        "create",
        "cli surface mission",
        "--intent",
        "Modernize the authentication flow",
        "--reporter",
        "refine-smoke",
    ]);
    fixture.assert_success("mission create", &create);
    let mission_id = fixture.json_stdout(&create)["mission"]["id"]
        .as_str()
        .expect("mission create should return mission.id")
        .to_string();
    assert_eq!(fixture.json_stdout(&create)["mission"]["status"], "draft");

    let list = fixture.run_refine(&["mission", "list"]);
    fixture.assert_success("mission list", &list);
    assert!(
        fixture.json_stdout(&list)["missions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mission| mission["id"].as_str() == Some(mission_id.as_str()))
    );

    let show = fixture.run_refine(&["mission", "show", &mission_id]);
    fixture.assert_success("mission show", &show);
    assert_eq!(fixture.json_stdout(&show)["mission"]["id"], mission_id);

    let start = fixture.run_refine(&["mission", "start", &mission_id]);
    fixture.assert_success("mission start", &start);
    assert_eq!(
        fixture.json_stdout(&start)["mission"]["status"],
        "investigate"
    );

    // The context projection reads back over the daemon route.
    let context = fixture.run_refine(&["mission", "context", &mission_id]);
    fixture.assert_success("mission context", &context);
    assert_eq!(
        fixture.json_stdout(&context)["context"]["mission_id"],
        mission_id
    );

    // Transfer validates the coordinator node and changes only coordination.
    let node_create = fixture.run_refine(&["node", "create", "mission-smoke-node"]);
    fixture.assert_success("node create", &node_create);
    let transfer = fixture.run_refine(&["mission", "transfer", &mission_id, "mission-smoke-node"]);
    fixture.assert_success("mission transfer", &transfer);
    assert_eq!(
        fixture.json_stdout(&transfer)["mission"]["coordinator_node_id"],
        "mission-smoke-node"
    );

    // Retry and decide fail closed without recorded stage failures or
    // decision requests rather than inventing work.
    let retry = fixture.run_refine(&["mission", "retry", &mission_id, "--stage", "quality"]);
    assert!(
        !retry.status.success(),
        "mission retry without a stage failure should fail closed"
    );
    let decide = fixture.run_refine(&[
        "mission",
        "decide",
        &mission_id,
        "dec-none",
        "--choice",
        "x",
        "--rationale",
        "none",
    ]);
    assert!(
        !decide.status.success(),
        "mission decide without a decision request should fail closed"
    );

    // Goal adoption edits the Draft plan and reports the new digest.
    let goal_id = fixture.create_goal("Mission adoption smoke");
    let add_goal =
        fixture.run_refine(&["mission", "add-goal", &mission_id, &goal_id, "--wave", "1"]);
    fixture.assert_success("mission add-goal", &add_goal);
    let remove_goal = fixture.run_refine(&["mission", "remove-goal", &mission_id, &goal_id]);
    fixture.assert_success("mission remove-goal", &remove_goal);

    let cancel = fixture.run_refine(&["mission", "cancel", &mission_id]);
    fixture.assert_success("mission cancel", &cancel);
    assert_eq!(
        fixture.json_stdout(&cancel)["mission"]["status"],
        "cancelled"
    );
}

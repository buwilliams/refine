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

    let cancel = fixture.run_refine(&["mission", "cancel", &mission_id]);
    fixture.assert_success("mission cancel", &cancel);
    assert_eq!(
        fixture.json_stdout(&cancel)["mission"]["status"],
        "cancelled"
    );
}

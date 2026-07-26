use super::super::*;
use crate::cli_surface::agents::node_ids;

pub(crate) fn node_create_activate_archive(fixture: &IntegrationFixture) {
    let list = fixture.run_refine(&["node", "list"]);
    fixture.assert_success("node list", &list);
    let payload = fixture.json_stdout(&list);
    assert_eq!(payload["active_node_id"], "default");
    assert!(node_ids(&payload).contains(&"default".to_string()));

    let create = fixture.run_refine(&["node", "create", "smoke-node"]);
    fixture.assert_success("node create", &create);
    let list = fixture.run_refine(&["node", "list"]);
    assert!(node_ids(&fixture.json_stdout(&list)).contains(&"smoke-node".to_string()));

    let activate = fixture.run_refine(&["node", "activate", "smoke-node"]);
    fixture.assert_success("node activate", &activate);
    let list = fixture.run_refine(&["node", "list"]);
    assert_eq!(fixture.json_stdout(&list)["active_node_id"], "smoke-node");

    let restore = fixture.run_refine(&["node", "activate", "default"]);
    fixture.assert_success("node restore", &restore);
    let archive = fixture.run_refine(&["node", "archive", "smoke-node"]);
    fixture.assert_success("node archive", &archive);
    let list = fixture.run_refine(&["node", "list"]);
    assert_eq!(fixture.json_stdout(&list)["active_node_id"], "default");
}

pub(crate) fn node_show_rename_settings_and_transfer(fixture: &IntegrationFixture) {
    let create = fixture.run_refine(&["node", "create", "transfer-node"]);
    fixture.assert_success("node create transfer", &create);

    let show = fixture.run_refine(&["node", "show", "transfer-node"]);
    fixture.assert_success("node show", &show);
    assert_eq!(fixture.json_stdout(&show)["node"]["id"], "transfer-node");

    let rename = fixture.run_refine(&["node", "rename", "transfer-node", "Transfer Node"]);
    fixture.assert_success("node rename", &rename);
    let shown = fixture.run_refine(&["node", "show", "transfer-node"]);
    fixture.assert_success("node show after rename", &shown);
    assert_eq!(
        fixture.json_stdout(&shown)["node"]["display_name"],
        "Transfer Node"
    );

    let settings = fixture.run_refine(&["node", "settings", "transfer-node"]);
    fixture.assert_success("node settings", &settings);
    let settings_payload = fixture.json_stdout(&settings);
    assert_eq!(settings_payload["node_id"], "transfer-node");
    assert!(
        settings_payload["settings"].is_object(),
        "{settings_payload:#}"
    );

    let goal_id = fixture.create_goal("node transfer goal");
    let transfer = fixture.run_refine(&["node", "transfer", "transfer-node", &goal_id]);
    fixture.assert_success("node transfer", &transfer);
    assert_eq!(fixture.goal_field(&goal_id, "node_id"), "transfer-node");

    let feature = fixture.run_refine(&["feature", "create", "cli node transfer feature"]);
    fixture.assert_success("feature create node transfer", &feature);
    let feature_id = fixture.json_stdout(&feature)["feature"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let feature_goal_id = fixture.create_goal("node transfer feature member goal");
    fixture.assert_success(
        "feature add node transfer goal",
        &fixture.run_refine(&["feature", "add-goal", &feature_id, &feature_goal_id]),
    );
    let direct_feature_goal_transfer =
        fixture.run_refine(&["node", "transfer", "transfer-node", &feature_goal_id]);
    assert!(
        !direct_feature_goal_transfer.status.success(),
        "Feature-owned Goal transfer unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&direct_feature_goal_transfer.stderr)
            .contains("transfer the Feature instead"),
        "stderr:\n{}",
        String::from_utf8_lossy(&direct_feature_goal_transfer.stderr)
    );
    fixture.assert_success(
        "feature transfer node",
        &fixture.run_refine(&["feature", "transfer", &feature_id, "transfer-node"]),
    );
    let transferred_feature = fixture.run_refine(&["feature", "show", &feature_id]);
    fixture.assert_success("feature show transferred node", &transferred_feature);
    assert_eq!(
        fixture.json_stdout(&transferred_feature)["feature"]["node_id"],
        "transfer-node"
    );
    assert_eq!(
        fixture.goal_field(&feature_goal_id, "node_id"),
        "transfer-node"
    );

    fixture.assert_success(
        "node activate transfer for cleanup",
        &fixture.run_refine(&["node", "activate", "transfer-node"]),
    );
    fixture.assert_success(
        "goal delete transferred",
        &fixture.run_refine(&["goal", "delete", &goal_id]),
    );
    fixture.assert_success(
        "goal delete transferred feature member",
        &fixture.run_refine(&["goal", "delete", &feature_goal_id]),
    );
    fixture.assert_success(
        "feature delete transferred",
        &fixture.run_refine(&["feature", "delete", &feature_id]),
    );
    fixture.assert_success(
        "node activate default after transfer cleanup",
        &fixture.run_refine(&["node", "activate", "default"]),
    );
    fixture.assert_success(
        "node archive transfer",
        &fixture.run_refine(&["node", "archive", "transfer-node"]),
    );
}

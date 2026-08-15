use super::super::*;

pub(crate) fn fleet_local_registry_commands(fixture: &IntegrationFixture) {
    let list = fixture.run_refine(&["fleet", "list"]);
    fixture.assert_success("fleet list", &list);
    assert!(fixture.json_stdout(&list)["nodes"].is_array());

    let add = fixture.run_refine(&["fleet", "add-node", "fleet-smoke"]);
    fixture.assert_success("fleet add-node", &add);
    let duplicate = fixture.run_refine(&["fleet", "add-node", "fleet-smoke"]);
    assert!(!duplicate.status.success(), "duplicate node succeeded");
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("already exists"),
        "stderr:\n{}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
    let invalid_host = fixture.run_refine(&[
        "fleet",
        "edit-node",
        "fleet-smoke",
        "--ssh-host",
        "deploy@example.com",
    ]);
    assert!(
        !invalid_host.status.success(),
        "invalid fleet ssh host succeeded"
    );
    assert!(
        String::from_utf8_lossy(&invalid_host.stderr).contains("ssh_host"),
        "stderr:\n{}",
        String::from_utf8_lossy(&invalid_host.stderr)
    );
    let edit = fixture.run_refine(&[
        "fleet",
        "edit-node",
        "fleet-smoke",
        "--display-name",
        "Fleet Smoke",
        "--ssh-host",
        "127.0.0.1",
        "--ssh-port",
        "22",
        "--target-app-path",
        fixture.app_root.to_str().unwrap(),
        "--refine-port",
        &fixture.port.to_string(),
        "--enabled",
        "true",
    ]);
    fixture.assert_success("fleet edit-node", &edit);
    let show = fixture.run_refine(&["fleet", "show", "fleet-smoke"]);
    fixture.assert_success("fleet show", &show);
    let shown = fixture.json_stdout(&show);
    assert_eq!(shown["node"]["display_name"], "Fleet Smoke");
    assert_eq!(shown["node"]["enabled"], true);

    fixture.assert_success(
        "fleet disable-node",
        &fixture.run_refine(&["fleet", "disable-node", "fleet-smoke"]),
    );
    fixture.assert_success(
        "fleet enable-node",
        &fixture.run_refine(&["fleet", "enable-node", "fleet-smoke"]),
    );
    let sync = fixture.run_refine(&["fleet", "sync"]);
    fixture.assert_success("fleet sync", &sync);
    let sync_payload = fixture.json_stdout(&sync);
    assert_eq!(sync_payload["ok"], true, "{sync_payload:#}");
    assert_eq!(sync_payload["git_sync"]["ok"], true, "{sync_payload:#}");
    let maintenance = fixture.run_refine(&["fleet", "maintenance"]);
    fixture.assert_success("fleet maintenance", &maintenance);
    let maintenance_payload = fixture.json_stdout(&maintenance);
    assert_eq!(maintenance_payload["ok"], true, "{maintenance_payload:#}");
    assert_eq!(
        maintenance_payload["maintenance"]["active"], true,
        "{maintenance_payload:#}"
    );
    assert!(
        maintenance_payload["fleet"]["nodes"].is_array(),
        "{maintenance_payload:#}"
    );

    let goal_id = fixture.create_goal("fleet transfer goal");
    let transfer = fixture.run_refine(&["fleet", "transfer", "fleet-smoke", &goal_id]);
    fixture.assert_success("fleet transfer", &transfer);
    let transfer_payload = fixture.json_stdout(&transfer);
    assert_eq!(transfer_payload["target_node_id"], "fleet-smoke");
    assert_eq!(transfer_payload["updated"], 1);
    assert_eq!(fixture.goal_field(&goal_id, "node_id"), "fleet-smoke");

    let missing_run = fixture.run_refine(&["fleet", "run", "missing-fleet-node", "printf ok"]);
    assert!(
        !missing_run.status.success(),
        "fleet run unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&missing_run.stderr).contains("missing-fleet-node"),
        "stderr:\n{}",
        String::from_utf8_lossy(&missing_run.stderr)
    );

    fixture.assert_success(
        "node activate fleet cleanup",
        &fixture.run_refine(&["node", "activate", "fleet-smoke"]),
    );
    fixture.assert_success(
        "goal delete fleet transferred",
        &fixture.run_refine(&["goal", "delete", &goal_id]),
    );
    fixture.assert_success(
        "node activate default after fleet cleanup",
        &fixture.run_refine(&["node", "activate", "default"]),
    );
    fixture.assert_success(
        "node archive fleet cleanup",
        &fixture.run_refine(&["node", "archive", "fleet-smoke"]),
    );
    fixture.assert_success(
        "fleet remove-node",
        &fixture.run_refine(&["fleet", "remove-node", "fleet-smoke"]),
    );
}

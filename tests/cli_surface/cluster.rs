use super::super::*;

pub(crate) fn cluster_local_registry_commands(fixture: &IntegrationFixture) {
    let list = fixture.run_refine(&["cluster", "list"]);
    fixture.assert_success("cluster list", &list);
    assert!(fixture.json_stdout(&list)["nodes"].is_array());

    let add = fixture.run_refine(&["cluster", "add-node", "cluster-smoke"]);
    fixture.assert_success("cluster add-node", &add);
    let duplicate = fixture.run_refine(&["cluster", "add-node", "cluster-smoke"]);
    assert!(!duplicate.status.success(), "duplicate node succeeded");
    assert!(
        String::from_utf8_lossy(&duplicate.stderr).contains("already exists"),
        "stderr:\n{}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
    let invalid_host = fixture.run_refine(&[
        "cluster",
        "edit-node",
        "cluster-smoke",
        "--ssh-host",
        "deploy@example.com",
    ]);
    assert!(
        !invalid_host.status.success(),
        "invalid cluster ssh host succeeded"
    );
    assert!(
        String::from_utf8_lossy(&invalid_host.stderr).contains("ssh_host"),
        "stderr:\n{}",
        String::from_utf8_lossy(&invalid_host.stderr)
    );
    let edit = fixture.run_refine(&[
        "cluster",
        "edit-node",
        "cluster-smoke",
        "--display-name",
        "Cluster Smoke",
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
    fixture.assert_success("cluster edit-node", &edit);
    let show = fixture.run_refine(&["cluster", "show", "cluster-smoke"]);
    fixture.assert_success("cluster show", &show);
    let shown = fixture.json_stdout(&show);
    assert_eq!(shown["node"]["display_name"], "Cluster Smoke");
    assert_eq!(shown["node"]["enabled"], true);

    fixture.assert_success(
        "cluster disable-node",
        &fixture.run_refine(&["cluster", "disable-node", "cluster-smoke"]),
    );
    fixture.assert_success(
        "cluster enable-node",
        &fixture.run_refine(&["cluster", "enable-node", "cluster-smoke"]),
    );
    let sync = fixture.run_refine(&["cluster", "sync"]);
    fixture.assert_success("cluster sync", &sync);
    let sync_payload = fixture.json_stdout(&sync);
    assert_eq!(
        sync_payload["operation"]["owner"], "project:sync",
        "{sync_payload:#}"
    );
    assert!(
        sync_payload["operation"]["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "{sync_payload:#}"
    );
    let maintenance = fixture.run_refine(&["cluster", "maintenance"]);
    fixture.assert_success("cluster maintenance", &maintenance);
    let maintenance_payload = fixture.json_stdout(&maintenance);
    assert_eq!(maintenance_payload["ok"], true, "{maintenance_payload:#}");
    assert_eq!(
        maintenance_payload["maintenance"]["active"], true,
        "{maintenance_payload:#}"
    );
    assert!(
        maintenance_payload["cluster"]["nodes"].is_array(),
        "{maintenance_payload:#}"
    );

    let goal_id = fixture.create_goal("cluster transfer goal");
    let transfer = fixture.run_refine(&["cluster", "transfer", "cluster-smoke", &goal_id]);
    fixture.assert_success("cluster transfer", &transfer);
    let transfer_payload = fixture.json_stdout(&transfer);
    assert_eq!(transfer_payload["target_node_id"], "cluster-smoke");
    assert_eq!(transfer_payload["updated"], 1);
    assert_eq!(fixture.goal_field(&goal_id, "node_id"), "cluster-smoke");

    let missing_run = fixture.run_refine(&["cluster", "run", "missing-cluster-node", "printf ok"]);
    assert!(
        !missing_run.status.success(),
        "cluster run unexpectedly succeeded"
    );
    assert!(
        String::from_utf8_lossy(&missing_run.stderr).contains("missing-cluster-node"),
        "stderr:\n{}",
        String::from_utf8_lossy(&missing_run.stderr)
    );

    fixture.assert_success(
        "node activate cluster cleanup",
        &fixture.run_refine(&["node", "activate", "cluster-smoke"]),
    );
    fixture.assert_success(
        "goal delete cluster transferred",
        &fixture.run_refine(&["goal", "delete", &goal_id]),
    );
    fixture.assert_success(
        "node activate default after cluster cleanup",
        &fixture.run_refine(&["node", "activate", "default"]),
    );
    fixture.assert_success(
        "node archive cluster cleanup",
        &fixture.run_refine(&["node", "archive", "cluster-smoke"]),
    );
    fixture.assert_success(
        "cluster remove-node",
        &fixture.run_refine(&["cluster", "remove-node", "cluster-smoke"]),
    );
}

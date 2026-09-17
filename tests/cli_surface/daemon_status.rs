use super::super::*;

pub(crate) fn system_status_reports_reachable_daemon_with_disabled_workflow(
    fixture: &IntegrationFixture,
) {
    let port = fixture.port.to_string();
    let runtime_root = fixture.runtime_root.display().to_string();
    let output = fixture.run_refine(&[
        "system",
        "status",
        "--port",
        &port,
        "--runtime-root",
        &runtime_root,
    ]);
    fixture.assert_success("system status", &output);
    let payload = fixture.json_stdout(&output);
    assert!(
        payload["running_ports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_u64() == Some(fixture.port.into())),
        "{payload:#}"
    );
    let status = payload["ports"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["port"].as_u64() == Some(fixture.port.into()))
        .expect("test daemon port should be listed");
    // This suite deliberately disables agent automation. Reachability must
    // remain true while shared workflow health truthfully reports no worker.
    assert_eq!(status["daemon_healthy"], false, "{status:#}");
    assert_eq!(status["workflow_health"]["healthy"], false);
    assert_eq!(
        status["workflow_health"]["reason"],
        "expected one workflow worker; observed 0"
    );
    assert_eq!(status["web_available"], true);
}

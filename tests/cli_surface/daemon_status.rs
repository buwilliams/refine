use super::super::*;

pub(crate) fn system_status_reports_healthy_daemon(fixture: &IntegrationFixture) {
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
    assert_eq!(status["daemon_healthy"], true);
    assert_eq!(status["web_available"], true);
}

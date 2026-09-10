//! Stale callers must not stop or retire a different registration with the same ID.
use super::*;

#[test]
fn termination_rejects_stale_registration_before_signalling_descendants() {
    for field in ["pid", "started_at", "owner", "incarnation"] {
        let root = std::env::temp_dir().join(format!("refine-stale-stop-{}", uuid::Uuid::new_v4()));
        let owner = FileProcessSupervisor::new(&root);
        let fixture = test_fixture::UnobservedChild::launch(
            &owner,
            true,
            json!({"workflow_incarnation":"current"}),
        );
        let mut stale = fixture.group.process.clone();
        match field {
            "pid" => stale.pid = Some(std::process::id()),
            "started_at" => stale.started_at = "earlier registration".into(),
            "owner" => stale.owner = ProcessOwner::Quality,
            _ => stale.details = Some(json!({"workflow_incarnation":"earlier"}).to_string()),
        }
        let result = owner.terminate_and_confirm_exit(&stale, Duration::from_secs(2));
        let survived = fixture.child_alive();
        owner
            .stop_owned_group(&fixture.group, Duration::from_secs(2))
            .unwrap();
        assert!(
            result.is_err(),
            "stale {field} caller was accepted: {result:?}"
        );
        assert!(survived, "stale {field} caller killed current descendants");
        assert!(
            owner.group_pending(&stale).unwrap_or(true),
            "another registration's proof released stale capacity"
        );
        assert!(!owner.group_pending(&fixture.group.process).unwrap());
        drop(fixture);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn terminal_registration_cannot_overwrite_a_replacement_execution() {
    let root =
        std::env::temp_dir().join(format!("refine-stale-settlement-{}", uuid::Uuid::new_v4()));
    let owner = FileProcessSupervisor::new(&root);
    let fixture = test_fixture::UnobservedChild::launch(
        &owner,
        true,
        json!({"workflow_incarnation":"current"}),
    );
    let mut stale = fixture.group.process.clone();
    stale.started_at = "earlier registration".into();
    stale.state = "failed".into();
    let path = owner.processes_dir().join(format!("{}.json", stale.id));
    let before = fs::read(&path).unwrap();
    let result = owner.register(stale);
    let preserved = fs::read(&path).unwrap() == before;
    // Restore the fixture after probing the old implementation so cleanup is safe.
    fs::write(&path, before).unwrap();
    owner
        .stop_owned_group(&fixture.group, Duration::from_secs(2))
        .unwrap();
    assert!(
        result.is_err(),
        "stale settlement overwrote current registration"
    );
    assert!(preserved);
    drop(fixture);
    fs::remove_dir_all(root).unwrap();
}

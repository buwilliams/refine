use super::*;

struct CompletedScope {
    root: PathBuf,
    owner: FileProcessSupervisor,
    process: ManagedProcess,
    group: OwnedGroup,
    handoff: Option<fs::File>,
}
impl CompletedScope {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("refine-proof-recovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let owner = FileProcessSupervisor::new(&root);
        let gate = root.join("finish");
        let mut process = owner.launch(ManagedProcessSpec {
            owner: ProcessOwner::Maintenance,
            command: "/bin/sh".into(),
            args: vec!["-c".into(), "while [ ! -e \"$1\" ]; do sleep 0.01; done; printf complete".into(), "sh".into(), gate.display().to_string()],
            cwd: None, env: Vec::new(), stdin: None, limits: None,
            authorization_command: None, sensitive: false,
            metadata: serde_json::from_value(json!({"isolated_process_group": true, "workflow_incarnation": "previous-worker"})).unwrap(),
        }).unwrap();
        let handoff = owner.begin_artifact_handoff(&process.id).unwrap();
        fs::write(gate, "finish").unwrap();
        owner.wait_for_reaper_idle(&process.id).unwrap();
        let group = owner
            .owned_groups()
            .unwrap()
            .into_iter()
            .find(|g| g.process.id == process.id)
            .expect("deferred transcript cleanup must preserve its group");
        assert!(group.launch_scope.as_ref().unwrap().proof(&group).unwrap());
        process.state = "exited".into();
        // Reproduce the retained terminal registration from interrupted cleanup.
        owner.write_process(&process).unwrap();
        Self {
            root,
            owner,
            process,
            group,
            handoff: Some(handoff),
        }
    }
    fn lose_group(&self) {
        fs::remove_file(self.owner.group_path(&self.process.id)).unwrap();
    }
}
impl Drop for CompletedScope {
    fn drop(&mut self) {
        self.handoff.take();
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn deferred_transient_cleanup_keeps_ownership_until_artifacts_are_retired() {
    let mut fixture = CompletedScope::new();
    fixture
        .owner
        .remove_process_artifacts(&fixture.process)
        .unwrap();
    assert!(fixture.owner.group_path(&fixture.process.id).exists());
    assert!(Path::new(fixture.process.stdout_path.as_ref().unwrap()).exists());
    assert!(
        fixture
            .owner
            .processes_dir()
            .join(format!("{}.json", fixture.process.id))
            .exists()
    );
    fixture.handoff.take();
    fixture
        .owner
        .remove_process_artifacts(&fixture.process)
        .unwrap();
    assert!(!fixture.owner.group_path(&fixture.process.id).exists());
    assert!(!Path::new(fixture.process.stdout_path.as_ref().unwrap()).exists());
}

#[test]
fn complete_scope_proof_recovers_missing_group_and_releases_worker_ownership() {
    let fixture = CompletedScope::new();
    fixture.lose_group();
    assert!(!fixture.owner.group_pending(&fixture.process).unwrap());
    let restored = fixture
        .owner
        .owned_group_for_process(&fixture.process)
        .unwrap();
    assert!(restored.confirmed_exit);
    assert!(restored.witnesses.is_empty());
    assert_eq!(restored.pgid, None);
    assert_eq!(restored.launch_scope, fixture.group.launch_scope);
    assert_eq!(
        fixture.owner.assess_owned_group(&restored).unwrap(),
        OwnershipAssessment::Exited
    );
    assert!(fixture.owner.capacity_processes().unwrap().is_empty());
    let health = crate::application::workflow::health::assess_workflow_health(&fixture.root);
    assert_ne!(health.state, "ownership_unverified", "{health:?}");
    assert_eq!(
        fs::read_to_string(fixture.process.stdout_path.as_ref().unwrap()).unwrap(),
        "complete"
    );
    assert!(!fixture.owner.group_pending(&fixture.process).unwrap());
}

#[test]
fn missing_group_recovery_preserves_unverified_or_replaced_evidence() {
    for failure in [
        "partial",
        "pid",
        "inode",
        "registration",
        "scope",
        "corrupt-group",
    ] {
        let fixture = CompletedScope::new();
        fixture.lose_group();
        let proof = &fixture.group.launch_scope.as_ref().unwrap().proof_path;
        match failure {
            "partial" => {
                let bytes = fs::read(proof).unwrap();
                fs::write(proof, &bytes[..8]).unwrap();
            }
            "pid" => {
                let mut bytes = fs::read(proof).unwrap();
                bytes[..4].copy_from_slice(&1u32.to_ne_bytes());
                fs::write(proof, bytes).unwrap();
            }
            "inode" => {
                let next = proof.with_extension("replacement");
                fs::write(&next, fs::read(proof).unwrap()).unwrap();
                fs::rename(next, proof).unwrap();
            }
            "registration" => {
                let mut newer = fixture.process.clone();
                newer.started_at.push('1');
                fixture.owner.write_process(&newer).unwrap();
            }
            "scope" => {
                let mut newer = fixture.process.clone();
                let mut details: Value =
                    serde_json::from_str(newer.details.as_ref().unwrap()).unwrap();
                details["launch_scope"]["proof_inode"] = json!(0);
                newer.details = Some(details.to_string());
                fixture.owner.write_process(&newer).unwrap();
            }
            "corrupt-group" => {
                fs::write(fixture.owner.group_path(&fixture.process.id), "corrupt").unwrap()
            }
            _ => unreachable!(),
        }
        assert!(
            fixture
                .owner
                .group_pending(&fixture.process)
                .unwrap_or(true),
            "{failure}"
        );
        if failure == "corrupt-group" {
            assert_eq!(
                fs::read_to_string(fixture.owner.group_path(&fixture.process.id)).unwrap(),
                "corrupt"
            );
        } else {
            assert!(
                !fixture.owner.group_path(&fixture.process.id).exists(),
                "{failure}"
            );
        }
        assert!(
            Path::new(fixture.process.stdout_path.as_ref().unwrap()).exists(),
            "{failure}"
        );
    }
}

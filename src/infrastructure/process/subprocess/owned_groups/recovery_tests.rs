use super::*;

fn await_condition(label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "nested ownership fixture did not settle: {label}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct NestedFixtureCleanup {
    root: PathBuf,
    owner: FileProcessSupervisor,
    parent: Option<ManagedProcess>,
    live_witness: Option<std::process::Child>,
}
impl Drop for NestedFixtureCleanup {
    fn drop(&mut self) {
        // Setup assertions must not strand a helper using the test executable
        // while the next build relinks it. Signal only this fixture's exact
        // retained workload identities; allow guardians to publish real receipts.
        let _ = fs::write(self.root.join("finish-parent"), "finish");
        let groups = [self.root.clone(), self.root.join("agents")]
            .into_iter()
            .flat_map(|root| {
                FileProcessSupervisor::new(root)
                    .owned_group_observations()
                    .unwrap_or_default()
            })
            .flatten()
            .collect::<Vec<_>>();
        for group in &groups {
            for (pid, identity) in &group.witnesses {
                if *pid != std::process::id()
                    && os_process_identity(*pid).ok().flatten().as_ref() == Some(identity)
                {
                    unsafe {
                        libc::kill(*pid as i32, libc::SIGKILL);
                    }
                }
            }
        }
        if let Some(child) = &mut self.live_witness {
            let _ = child.kill();
            let _ = child.wait();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while groups.iter().any(|group| {
            group
                .launch_scope
                .as_ref()
                .is_some_and(|scope| scope.alive().unwrap_or(false))
        }) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        for group in &groups {
            if let Some(scope) = &group.launch_scope
                && scope.alive().unwrap_or(false)
            {
                unsafe {
                    libc::kill(scope.guardian_pid as i32, libc::SIGKILL);
                }
            }
        }
        if let Some(parent) = &self.parent {
            let _ = self.owner.wait_for_reaper_idle(&parent.id);
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn nested_spec(
    root: &Path,
    command: String,
    args: Vec<String>,
    owner: ProcessOwner,
) -> ManagedProcessSpec {
    ManagedProcessSpec {
        owner,
        command,
        args,
        cwd: None,
        env: vec![(
            "REFINE_NESTED_RECOVERY_ROOT".into(),
            root.display().to_string(),
        )],
        stdin: None,
        limits: None,
        authorization_command: None,
        sensitive: false,
        metadata: serde_json::from_value(json!({
            "isolated_process_group": true, "workflow_incarnation": root.display().to_string(),
            "worker_kind": "workflow"
        }))
        .unwrap(),
    }
}

#[test]
fn nested_scope_recovery_requires_authentic_complete_enclosing_receipt() {
    // The helper is a real managed workflow workload. It launches the inner
    // scope, preserving the registration's actual creator PID relationship.
    if let Some(root) = std::env::var_os("REFINE_NESTED_RECOVERY_ROOT") {
        let root = PathBuf::from(root);
        let child_root = if std::env::var_os("REFINE_NESTED_RECOVERY_AGENTS").is_some() {
            root.join("agents")
        } else {
            root.clone()
        };
        let owner = FileProcessSupervisor::new(&child_root);
        let child = owner
            .launch(nested_spec(
                &root,
                "/bin/sleep".into(),
                vec!["60".into()],
                ProcessOwner::Agent,
            ))
            .unwrap();
        let _handoff = owner.begin_artifact_handoff(&child.id).unwrap();
        write_json_atomically(
            &root.join("nested-child.json"),
            &serde_json::to_vec(&child).unwrap(),
            "nested test registration",
        )
        .unwrap();
        await_condition("parent release", || root.join("finish-parent").exists());
        return;
    }
    for agents in [false, true] {
        nested_scope_recovery_case(agents, false);
        nested_scope_recovery_case(agents, true);
    }
}

fn nested_scope_recovery_case(agents: bool, corrupt_guardian_record: bool) {
    let root =
        std::env::temp_dir().join(format!("refine-nested-recovery-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let parent_owner = FileProcessSupervisor::new(&root);
    let mut cleanup = NestedFixtureCleanup {
        root: root.clone(),
        owner: parent_owner.clone(),
        parent: None,
        live_witness: None,
    };
    let child_root = if agents {
        root.join("agents")
    } else {
        root.clone()
    };
    fs::create_dir_all(&child_root).unwrap();
    let owner = FileProcessSupervisor::new(child_root);
    let mut spec = nested_spec(
        &root,
        std::env::current_exe().unwrap().display().to_string(),
        vec![
            "--exact".into(),
            format!(
                "{}::nested_scope_recovery_requires_authentic_complete_enclosing_receipt",
                module_path!().split_once("::").unwrap().1
            ),
            "--nocapture".into(),
        ],
        ProcessOwner::Runner,
    );
    if agents {
        spec.env
            .push(("REFINE_NESTED_RECOVERY_AGENTS".into(), "1".into()));
    }
    let parent = parent_owner.launch(spec).unwrap();
    cleanup.parent = Some(parent.clone());
    let parent_handoff = parent_owner.begin_artifact_handoff(&parent.id).unwrap();
    await_condition("nested child registration", || {
        root.join("nested-child.json").exists()
    });
    let child: ManagedProcess =
        serde_json::from_slice(&fs::read(root.join("nested-child.json")).unwrap()).unwrap();
    let mut child_group = owner.owned_group_for_process(&child).unwrap();
    let mut parent_group = parent_owner
        .observe_owned_group(&parent_owner.owned_group_for_process(&parent).unwrap())
        .unwrap();
    let child_scope = child_group.launch_scope.as_ref().unwrap().clone();
    assert_eq!(registered_launcher_pid(&child.id), parent.pid);
    assert_eq!(
        parent_group.witnesses.get(&child.pid.unwrap()),
        child_group.witnesses.get(&child.pid.unwrap())
    );
    assert!(
        owner
            .enclosing_scope_exit_evidence(&child_group)
            .unwrap()
            .is_none(),
        "live guardian cannot be retired"
    );
    if corrupt_guardian_record {
        // Lose the inner guardian's registration while its real helper remains
        // alive. OS role inspection must preserve it through enclosing stop;
        // corruption remains reported independently of valid deadline cleanup.
        let path = owner.group_path(&child.id);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"corrupt guardian evidence").unwrap();
        assert!(
            owner
                .owned_group_observations()
                .unwrap()
                .iter()
                .any(Result::is_err)
        );
        let observed = parent_owner.observe_owned_group(&parent_group).unwrap();
        assert!(!observed.witnesses.contains_key(&child_scope.guardian_pid));
        assert!(child_scope.alive().unwrap());
        let stopped = parent_owner
            .stop_owned_group(&observed, Duration::from_secs(3))
            .unwrap();
        assert!(stopped.confirmed_exit);
        assert!(
            child_scope.proof(&child_group).unwrap(),
            "inner guardian must survive to publish its authentic ECHILD receipt"
        );
        assert!(
            owner
                .owned_group_observations()
                .unwrap()
                .iter()
                .any(Result::is_err)
        );
        fs::write(path, original).unwrap();
        assert_eq!(
            owner.assess_owned_group(&child_group).unwrap(),
            OwnershipAssessment::Exited
        );
        drop(parent_handoff);
        drop(cleanup);
        return;
    }
    // Reproduce legacy direct shutdown losing the inner guardian. The real outer
    // subreaper still adopts and reaps its orphaned workload; no proof is fabricated.
    assert_eq!(
        unsafe { libc::kill(child_scope.guardian_pid as i32, libc::SIGKILL) },
        0
    );
    assert_eq!(
        unsafe { libc::kill(child.pid.unwrap() as i32, libc::SIGKILL) },
        0
    );
    await_condition("lost inner guardian and workload exit", || {
        os_process_identity(child.pid.unwrap()).unwrap().is_none() && !child_scope.alive().unwrap()
    });
    assert!(
        owner
            .enclosing_scope_exit_evidence(&child_group)
            .unwrap()
            .is_none(),
        "an incomplete outer scope is not exit proof"
    );
    child_group = owner.observe_owned_group(&child_group).unwrap();
    let previous_gap = child_group.ownership_gap.clone();
    assert!(!child_group.confirmed_exit);
    fs::write(root.join("finish-parent"), "finish").unwrap();
    parent_owner.wait_for_reaper_idle(&parent.id).unwrap();
    parent_group = parent_owner.owned_group_for_process(&parent).unwrap();
    assert!(
        parent_group
            .launch_scope
            .as_ref()
            .unwrap()
            .proof(&parent_group)
            .unwrap()
    );
    let original_child_proof = fs::read(&child_scope.proof_path).unwrap();
    assert_eq!(original_child_proof.len(), 4);
    let original_parent_proof =
        fs::read(&parent_group.launch_scope.as_ref().unwrap().proof_path).unwrap();
    cleanup.live_witness = Some(Command::new("/bin/sleep").arg("60").spawn().unwrap());
    let live_witness = cleanup.live_witness.as_ref().unwrap().id();

    // Each variant changes retained evidence, not running processes. Restore the
    // exact real records after each rejection so the positive case remains authentic.
    for failure in [
        "identity",
        "outside-witness",
        "scope",
        "parent-inode",
        "unrelated-proof",
        "parent-partial",
        "parent-flag-only",
        "nonisolated",
        "pty",
        "redacted",
        "pgid",
        "mixed-epoch",
        "live-witness",
        "launcher",
    ] {
        let mut altered_child = child_group.clone();
        let mut altered_parent = parent_group.clone();
        match failure {
            "identity" => {
                altered_parent.witnesses.insert(
                    child.pid.unwrap(),
                    format!("{}0", child_group.witnesses[&child.pid.unwrap()]),
                );
            }
            "outside-witness" => {
                altered_child.witnesses.insert(
                    u32::MAX - 1,
                    child_group.witnesses[&child.pid.unwrap()].clone(),
                );
            }
            "scope" => {
                altered_child.launch_scope.as_mut().unwrap().proof_inode += 1;
            }
            "parent-inode" => {
                altered_parent.launch_scope.as_mut().unwrap().proof_inode += 1;
                let mut metadata: Value =
                    serde_json::from_str(altered_parent.process.details.as_deref().unwrap())
                        .unwrap();
                metadata["launch_scope"] =
                    serde_json::to_value(&altered_parent.launch_scope).unwrap();
                altered_parent.process.details = Some(metadata.to_string());
                parent_owner.write_process(&altered_parent.process).unwrap();
            }
            "unrelated-proof" => {
                altered_parent.launch_scope.as_mut().unwrap().proof_path =
                    child_scope.proof_path.clone();
            }
            "parent-partial" | "parent-flag-only" => {
                altered_parent.confirmed_exit = failure == "parent-flag-only";
                fs::write(
                    &altered_parent.launch_scope.as_ref().unwrap().proof_path,
                    &original_parent_proof[..4],
                )
                .unwrap();
            }
            "nonisolated" | "pty" | "redacted" => {
                let mut metadata: Value =
                    serde_json::from_str(altered_child.process.details.as_deref().unwrap())
                        .unwrap();
                match failure {
                    "nonisolated" => metadata["isolated_process_group"] = json!(false),
                    "pty" => metadata["pty_owned_scope"] = json!(true),
                    _ => metadata["command"] = json!("redacted"),
                }
                altered_child.process.details = Some(metadata.to_string());
                owner.write_process(&altered_child.process).unwrap();
            }
            "pgid" => {
                altered_child.pgid = parent.pid;
            }
            "mixed-epoch" => {
                altered_child.witnesses.insert(
                    child.pid.unwrap(),
                    format!("linux:{}:1", uuid::Uuid::new_v4()),
                );
            }
            "live-witness" => {
                let token = os_process_identity(live_witness).unwrap().unwrap();
                altered_child.witnesses.insert(live_witness, token.clone());
                altered_parent.witnesses.insert(live_witness, token);
            }
            "launcher" => {
                altered_parent.process.pid = Some(std::process::id());
            }
            _ => unreachable!(),
        }
        parent_owner.write_owned_group(&altered_parent).unwrap();
        assert!(
            owner
                .enclosing_scope_exit_evidence(&altered_child)
                .unwrap_or(None)
                .is_none(),
            "{failure}"
        );
        owner.write_process(&child).unwrap();
        parent_owner.write_process(&parent_group.process).unwrap();
        parent_owner.write_owned_group(&parent_group).unwrap();
        fs::write(
            &parent_group.launch_scope.as_ref().unwrap().proof_path,
            &original_parent_proof,
        )
        .unwrap();
    }
    let mut live_witness = cleanup.live_witness.take().unwrap();
    live_witness.kill().unwrap();
    live_witness.wait().unwrap();
    // Early registration commonly has no observed PGID yet; isolation comes
    // from the original non-PTY setsid launch protocol, not that optional probe.
    child_group.pgid = None;
    parent_group.pgid = None;
    parent_group.confirmed_exit = false;
    owner.write_owned_group(&child_group).unwrap();
    parent_owner.write_owned_group(&parent_group).unwrap();
    let recovered = owner.observe_owned_group(&child_group).unwrap();
    assert!(recovered.confirmed_exit);
    let evidence = recovered.enclosing_scope_exit.as_ref().unwrap();
    assert_eq!(evidence.process_id, parent.id);
    assert_eq!(evidence.previous_ownership_gap, previous_gap);
    assert_eq!(
        evidence.launch_scope,
        parent_group.launch_scope.clone().unwrap()
    );
    assert_eq!(
        fs::read(&child_scope.proof_path).unwrap(),
        original_child_proof
    );
    assert_eq!(
        owner.assess_owned_group(&recovered).unwrap(),
        OwnershipAssessment::Exited
    );
    assert!(!owner.group_pending(&child).unwrap());
    assert!(
        !owner
            .capacity_processes()
            .unwrap()
            .iter()
            .any(|p| p.id == child.id)
    );
    // Neither cached true nor derived provenance replaces the original receipt.
    let parent_proof_path = &parent_group.launch_scope.as_ref().unwrap().proof_path;
    let retained_parent_proof = parent_proof_path.with_extension("retained");
    fs::rename(parent_proof_path, &retained_parent_proof).unwrap();
    let unverified = owner.observe_owned_group(&recovered).unwrap();
    assert!(!unverified.confirmed_exit);
    assert!(unverified.enclosing_scope_exit.is_none());
    assert!(unverified.ownership_gap.is_some());
    assert_eq!(
        fs::read(&child_scope.proof_path).unwrap(),
        original_child_proof
    );
    fs::rename(retained_parent_proof, parent_proof_path).unwrap();
    assert_eq!(
        owner.assess_owned_group(&unverified).unwrap(),
        OwnershipAssessment::Exited
    );
    drop(parent_handoff);
    drop(cleanup);
}

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

#[test]
fn later_lifetime_evidence_clears_cached_ownership_uncertainty() {
    let fixture = CompletedScope::new();
    let mut group = fixture.group.clone();
    group.ownership_gap = Some("earlier guardian observation was inconclusive".into());
    fixture.owner.write_owned_group(&group).unwrap();
    let observed = fixture.owner.observe_owned_group(&group).unwrap();
    assert!(observed.confirmed_exit);
    assert!(observed.ownership_gap.is_none());
    assert!(!fixture.owner.group_pending(&fixture.process).unwrap());
}

#[test]
fn expired_execution_epoch_releases_all_ownership_without_signalling_reused_pids() {
    let fixture = CompletedScope::new();
    let mut group = fixture.group.clone();
    let previous = format!("linux:{}:42", uuid::Uuid::new_v4());
    let scope = group.launch_scope.as_mut().unwrap();
    scope.guardian_identity = Some(previous.clone());
    scope.guardian_pid = std::process::id();
    fs::write(&scope.proof_path, b"").unwrap();
    group.witnesses = BTreeMap::from([(std::process::id(), previous)]);
    group.ownership_gap = Some("guardian disappeared before settling".into());
    let mut metadata: Value =
        serde_json::from_str(group.process.details.as_deref().unwrap()).unwrap();
    metadata["launch_scope"] = serde_json::to_value(&group.launch_scope).unwrap();
    group.process.details = Some(metadata.to_string());
    fixture.owner.write_process(&group.process).unwrap();
    fixture.owner.write_owned_group(&group).unwrap();
    let observed = fixture
        .owner
        .stop_owned_group(&group, Duration::from_millis(100))
        .unwrap();
    assert!(observed.confirmed_exit);
    assert!(observed.ownership_gap.is_none());
    assert!(os_process_identity(std::process::id()).unwrap().is_some());
    assert!(!fixture.owner.group_pending(&group.process).unwrap());

    // Mixed-epoch witnesses are contradictory, not evidence of complete exit.
    group.witnesses.insert(
        std::process::id(),
        os_process_identity(std::process::id()).unwrap().unwrap(),
    );
    fixture.owner.write_owned_group(&group).unwrap();
    assert!(matches!(
        fixture.owner.assess_owned_group(&group).unwrap(),
        OwnershipAssessment::Unverified { .. }
    ));
}

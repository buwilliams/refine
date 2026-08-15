use super::*;

#[test]
fn upgrade_agent_and_required_infrastructure_do_not_deadlock_runtime_drain() {
    use crate::process::subprocess::ManagedProcess;

    let root = test_directory("source-upgrade-agent-drain");
    let runtime = root.join("runtime/8080");
    let service = FileSourcePromotionService::new(root.join("checkout"), &runtime, 8080);
    let supervisor = FileProcessSupervisor::new(&runtime);
    supervisor.set_workflow_paused(true).unwrap();
    let mut upgrade = operation();
    upgrade.status = "running".to_string();
    upgrade.stage = "drain_work".to_string();
    upgrade.agent_process_id = Some("upgrade-agent".to_string());
    upgrade.agent_worker_process_id = Some("upgrade-launcher".to_string());
    service.save_operation(&upgrade).unwrap();
    let process = |id: &str, owner: ProcessOwner, details: Option<String>| ManagedProcess {
        id: id.to_string(),
        owner,
        pid: Some(std::process::id()),
        state: "running".to_string(),
        label: Some(id.to_string()),
        details,
        stdout_path: None,
        stderr_path: None,
        stdin_path: None,
        limits: None,
        started_at: now_timestamp(),
        exit_code: None,
    };
    supervisor
        .register(process(
            "upgrade-agent",
            ProcessOwner::Agent,
            Some(format!(
                r#"{{"kind":"source_upgrade_agent","source_upgrade_operation":"{}"}}"#,
                upgrade.id
            )),
        ))
        .unwrap();
    supervisor
        .register(process(
            "upgrade-launcher",
            ProcessOwner::Maintenance,
            Some(format!(
                r#"{{"kind":"source_upgrade_agent_launcher","source_upgrade_operation":"{}"}}"#,
                upgrade.id
            )),
        ))
        .unwrap();
    supervisor
        .register(process("target-app", ProcessOwner::TargetApp, None))
        .unwrap();
    supervisor
        .register(process("quality", ProcessOwner::Quality, None))
        .unwrap();

    let active = service.active_work().unwrap();
    assert!(!active.iter().any(|item| item.contains("upgrade-agent")));
    assert!(!active.iter().any(|item| item.contains("upgrade-launcher")));
    assert!(!active.iter().any(|item| item.contains("target-app")));
    assert!(active.iter().any(|item| item.contains("quality")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn upgrade_restores_exact_prior_workflow_admission_intent() {
    let root = test_directory("source-upgrade-pause-restore");
    let runtime = root.join("runtime/8080");
    let service = FileSourcePromotionService::new(root.join("checkout"), &runtime, 8080);
    let supervisor = FileProcessSupervisor::new(&runtime);

    for prior_paused in [false, true] {
        supervisor.set_workflow_paused(true).unwrap();
        let mut upgrade = operation();
        upgrade.pre_upgrade_workflow_paused = Some(prior_paused);
        service.restore_workflow_admission(&mut upgrade).unwrap();
        assert_eq!(
            supervisor.pause_state().unwrap().workflow_paused,
            prior_paused
        );
        assert_eq!(upgrade.workflow_pause_restored, Some(true));
    }

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn granular_agent_capabilities_preserve_prior_admission_and_queue_one_handoff() {
    use std::os::unix::fs::PermissionsExt;

    for prior_paused in [false, true] {
        let repo = initialize_promotion_repository(if prior_paused {
            "typed-agent-plan-paused"
        } else {
            "typed-agent-plan-running"
        });
        let runtime = repo.root.join("run/8080");
        let service = FileSourcePromotionService::new(&repo.checkout, &runtime, 8080);
        let supervisor = FileProcessSupervisor::new(&runtime);
        supervisor.set_workflow_paused(prior_paused).unwrap();
        let snapshot = service.inspect(false).unwrap();
        let mut upgrade = SourcePromotionOperation::queued(&snapshot);
        let registered = service
            .operation_registry()
            .register_exclusive_with_request(
                "maintenance:source-upgrade",
                json!({"restart_recovery": "capability"}),
            )
            .unwrap();
        upgrade.id = registered.id;
        let agent_script = repo.root.join("agent.sh");
        fs::write(&agent_script, "#!/bin/sh\nsleep 5\n").unwrap();
        let mut permissions = fs::metadata(&agent_script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&agent_script, permissions).unwrap();
        let mut metadata = serde_json::Map::new();
        metadata.insert("kind".to_string(), json!("source_upgrade_agent"));
        metadata.insert("operation_id".to_string(), json!(&upgrade.id));
        let agent_process = supervisor
            .launch(crate::process::subprocess::ManagedProcessSpec {
                owner: ProcessOwner::Agent,
                command: agent_script.display().to_string(),
                args: Vec::new(),
                cwd: Some(repo.checkout.display().to_string()),
                env: Vec::new(),
                stdin: None,
                limits: None,
                authorization_command: Some(agent_script.display().to_string()),
                sensitive: false,
                metadata,
            })
            .unwrap();
        upgrade.status = "running".to_string();
        upgrade.stage = "agent_running".to_string();
        upgrade.pre_upgrade_workflow_paused = Some(prior_paused);
        service.save_operation(&upgrade).unwrap();
        let launcher = RecordingHelperLauncher::default();
        service
            .run_agent_capability_with(
                &upgrade.id,
                "pause-admission",
                Path::new("/mock/refine"),
                &launcher,
            )
            .unwrap();
        let observation = service
            .run_agent_capability_with(
                &upgrade.id,
                "observe-work",
                Path::new("/mock/refine"),
                &launcher,
            )
            .unwrap();
        assert_eq!(observation["evidence"]["settled"], true);
        let mut prepared = service.load_operation().unwrap().unwrap();
        prepared.agent_decisions.push(json!({
            "action": "refresh-source",
            "at": now_timestamp(),
            "evidence": {"in_flight": false}
        }));
        service.save_operation(&prepared).unwrap();
        let refreshed_snapshot = service.inspect(false).unwrap();
        fs::write(
            service.update_check_state_path(),
            serde_json::to_vec_pretty(&SourceUpdateCheckState {
                last_successful_check_at: Some(now_timestamp()),
                current_source_identity: Some(repo.from_commit.clone()),
                available_source_identity: Some(repo.to_commit.clone()),
                freshness: "fresh".to_string(),
                source: Some(refreshed_snapshot),
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        service
            .run_agent_capability_with(
                &upgrade.id,
                "prepare-candidate",
                Path::new("/mock/refine"),
                &launcher,
            )
            .unwrap();
        service
            .run_agent_capability_with(
                &upgrade.id,
                "handoff-promotion",
                Path::new("/mock/refine"),
                &launcher,
            )
            .unwrap();
        let handed_off = service.load_operation().unwrap().unwrap();
        assert_eq!(handed_off.stage, "restart_safe_handoff");
        assert!(supervisor.pause_state().unwrap().workflow_paused);
        assert_eq!(launcher.handoffs.borrow().len(), 1);
        assert_eq!(handed_off.pre_upgrade_workflow_paused, Some(prior_paused));

        supervisor.signal(&agent_process.id, "terminate").unwrap();

        fs::remove_dir_all(repo.root).unwrap();
    }
}

#[test]
fn granular_agent_observation_preserves_active_claim_until_agent_selects_recovery() {
    let repo = initialize_promotion_repository("typed-agent-plan-active-claim");
    let runtime = repo.root.join("run/8080");
    let service = FileSourcePromotionService::new(&repo.checkout, &runtime, 8080);
    let supervisor = FileProcessSupervisor::new(&runtime);
    supervisor.set_workflow_paused(false).unwrap();
    let active_claim = supervisor
        .launch(crate::process::subprocess::ManagedProcessSpec {
            owner: ProcessOwner::Agent,
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "sleep 30".to_string()],
            cwd: None,
            env: Vec::new(),
            stdin: None,
            limits: None,
            authorization_command: Some("refine test active Goal claim".to_string()),
            sensitive: false,
            metadata: serde_json::from_value(json!({
                "kind": "goal_agent",
                "goal_id": "G1",
                "claim_id": "claim-1"
            }))
            .unwrap(),
        })
        .unwrap();
    let snapshot = service.inspect(false).unwrap();
    let mut upgrade = SourcePromotionOperation::queued(&snapshot);
    upgrade.status = "running".to_string();
    upgrade.stage = "agent_running".to_string();
    upgrade.pre_upgrade_workflow_paused = Some(false);
    service.save_operation(&upgrade).unwrap();

    service
        .run_agent_capability_with(
            &upgrade.id,
            "pause-admission",
            Path::new("/mock/refine"),
            &RecordingHelperLauncher::default(),
        )
        .unwrap();
    let observation = service
        .run_agent_capability_with(
            &upgrade.id,
            "observe-work",
            Path::new("/mock/refine"),
            &RecordingHelperLauncher::default(),
        )
        .unwrap();
    assert_eq!(observation["evidence"]["settled"], false);
    assert!(
        observation["evidence"]["active_work"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("running agent process"))
    );
    let error = service
        .run_agent_capability_with(
            &upgrade.id,
            "prepare-candidate",
            Path::new("/mock/refine"),
            &RecordingHelperLauncher::default(),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("observe preserved managed work settled")
    );
    supervisor.signal(&active_claim.id, "terminate").unwrap();
    service
        .run_agent_capability_with(
            &upgrade.id,
            "restore-admission",
            Path::new("/mock/refine"),
            &RecordingHelperLauncher::default(),
        )
        .unwrap();
    assert!(!supervisor.pause_state().unwrap().workflow_paused);
    assert!(
        !runtime.join("workflow-automation-state.json").exists(),
        "current transient execution ownership must not be replaced with a durable claim file"
    );
    let recovered = service.load_operation().unwrap().unwrap();
    assert_eq!(recovered.workflow_pause_restored, Some(true));

    fs::remove_dir_all(repo.root).unwrap();
}

#[test]
fn workflow_admission_restoration_failure_is_terminal_and_preserves_both_outcomes() {
    let root = test_directory("source-upgrade-pause-restore-failure");
    let runtime = root.join("runtime/8080");
    let service = FileSourcePromotionService::new(root.join("checkout"), &runtime, 8080);
    let supervisor = FileProcessSupervisor::new(&runtime);
    supervisor.set_workflow_paused(true).unwrap();
    let mut upgrade = operation();
    upgrade.status = "succeeded".to_string();
    upgrade.pre_upgrade_workflow_paused = Some(false);
    service.save_operation(&upgrade).unwrap();
    fs::remove_file(supervisor.pause_state_path()).unwrap();
    fs::create_dir(supervisor.pause_state_path()).unwrap();

    let error = service
        .restore_workflow_admission(&mut upgrade)
        .unwrap_err();
    assert!(error.to_string().contains("process control"));
    let persisted = service.load_operation().unwrap().unwrap();
    assert_eq!(persisted.status, "failed");
    assert_eq!(persisted.stage, "restore_workflow_admission");
    assert_eq!(persisted.primary_outcome.as_deref(), Some("succeeded"));
    assert_eq!(persisted.workflow_pause_restored, Some(false));
    assert!(persisted.restoration_error.is_some());
    assert!(persisted.recovery.as_deref().unwrap().contains("running"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_upgrade_requests_return_the_same_active_receipt() {
    let root = test_directory("source-upgrade-idempotency");
    fs::create_dir_all(root.join("checkout/bin")).unwrap();
    fs::write(root.join("checkout/bin/refine"), b"installed-refine").unwrap();
    let service =
        FileSourcePromotionService::new(root.join("checkout"), root.join("run/8080"), 8080);
    let mut active = operation();
    active.status = "running".to_string();
    active.stage = "restart_safe_handoff".to_string();
    service.save_operation(&active).unwrap();

    assert_eq!(service.queue_agent("smoke-ai").unwrap(), active);
    assert_eq!(service.queue_agent("smoke-ai").unwrap(), active);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn upgrade_reservation_without_agent_settles_interrupted_and_retryable_after_restart() {
    let repo = initialize_promotion_repository("source-upgrade-reservation-failpoint");
    let runtime = repo.root.join("run/8080");
    let port = spawn_single_http_probe();
    let service = FileSourcePromotionService::new(&repo.checkout, &runtime, port);
    persist_cached_source(&service, &repo);

    let error = service
        .queue_agent_with_failpoint(
            "smoke-ai",
            Path::new("/mock/refine"),
            AgentLaunchFailpoint::AfterReservation,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("after source-upgrade reservation")
    );
    let operation_id = service.load_operation().unwrap().unwrap().id;
    let restarted = FileSourcePromotionService::new(&repo.checkout, &runtime, port);
    let settled = restarted.reconcile_interrupted_agent().unwrap().unwrap();
    assert_eq!(settled.status, "interrupted");
    assert!(settled.error.as_deref().unwrap().contains("No correlated"));
    assert_eq!(
        restarted
            .operation_registry()
            .status(&operation_id)
            .unwrap()
            .state,
        OperationState::Failed
    );
    let replacement = restarted
        .operation_registry()
        .register_exclusive_with_request(
            "maintenance:source-upgrade",
            json!({"restart_recovery": "capability"}),
        )
        .unwrap();
    assert_ne!(replacement.id, operation_id);

    fs::remove_dir_all(repo.root).unwrap();
}

#[test]
fn active_launch_reservation_is_not_settled_before_its_process_receipt() {
    use fs2::FileExt;

    let root = test_directory("source-upgrade-active-launch-reservation");
    let runtime = root.join("run/8080");
    let service = FileSourcePromotionService::new(root.join("checkout"), &runtime, 8080);
    let registered = service
        .operation_registry()
        .register_exclusive_with_request(
            "maintenance:source-upgrade",
            json!({"restart_recovery": "capability"}),
        )
        .unwrap();
    let mut active = operation();
    active.id = registered.id.clone();
    active.status = "queued".to_string();
    active.stage = "agent_queued".to_string();
    service.save_operation(&active).unwrap();

    fs::create_dir_all(&runtime).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(runtime.join(".source-upgrade-queue.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();

    let observed = service.reconcile_interrupted_agent().unwrap().unwrap();
    assert_eq!(observed.status, "queued");
    assert_eq!(observed.stage, "agent_queued");
    assert_eq!(
        service
            .operation_registry()
            .status(&registered.id)
            .unwrap()
            .state,
        OperationState::Running
    );

    drop(lock);
    let settled = service.reconcile_interrupted_agent().unwrap().unwrap();
    assert_eq!(settled.status, "interrupted");
    assert_eq!(settled.stage, "agent_interrupted");
    assert_eq!(
        service
            .operation_registry()
            .status(&registered.id)
            .unwrap()
            .state,
        OperationState::Failed
    );

    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn launched_upgrade_agent_is_adopted_without_projection_receipt_and_not_duplicated() {
    use std::os::unix::fs::PermissionsExt;

    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let repo = initialize_promotion_repository("source-upgrade-launch-failpoint");
    let runtime = repo.root.join("run/8080");
    let provider = repo.root.join("smoke-ai");
    fs::write(&provider, "#!/bin/sh\nsleep 3\n").unwrap();
    let mut permissions = fs::metadata(&provider).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&provider, permissions).unwrap();
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }
    let port = spawn_single_http_probe();
    let service = FileSourcePromotionService::new(&repo.checkout, &runtime, port);
    persist_cached_source(&service, &repo);

    service
        .queue_agent_with_failpoint(
            "smoke-ai",
            Path::new("/mock/refine"),
            AgentLaunchFailpoint::AfterLaunchBeforeProjectionReceipt,
        )
        .unwrap_err();
    let operation_id = service.load_operation().unwrap().unwrap().id;
    let restarted = FileSourcePromotionService::new(&repo.checkout, &runtime, port);
    let adopted = restarted.reconcile_interrupted_agent().unwrap().unwrap();
    assert_eq!(adopted.status, "running");
    assert!(adopted.agent_process_id.is_some());
    let repeated = restarted
        .queue_agent_with("smoke-ai", Path::new("/mock/refine"))
        .unwrap();
    assert_eq!(repeated.id, operation_id);
    let correlated = restarted.correlated_processes(&operation_id).unwrap();
    assert_eq!(correlated.len(), 1);
    assert_eq!(correlated[0].owner, ProcessOwner::Agent);
    let _ = FileProcessSupervisor::new(&runtime).signal(&correlated[0].id, "terminate");

    unsafe {
        match previous {
            Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
            None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
        }
    }
    fs::remove_dir_all(repo.root).unwrap();
}

#[cfg(unix)]
#[test]
fn installed_provider_fixture_conditionally_invokes_multiple_granular_capabilities() {
    use std::os::unix::fs::PermissionsExt;

    let _env_guard = crate::tools::host::agent_providers::smoke_ai_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = test_directory("source-upgrade-installed-provider");
    let runtime = root.join("run/8080");
    let checkout = root.join("checkout");
    fs::create_dir_all(&checkout).unwrap();
    let provider = root.join("smoke-ai");
    let capability = root.join("typed-capability");
    let invocations = root.join("capability-invocations.log");
    let context = root.join("source-upgrade-context.json");
    fs::write(&context, "{\"pre_upgrade_workflow_paused\":true}\n").unwrap();
    fs::write(
        &provider,
        format!(
            "#!/bin/sh\nprompt=$1\ncontext=$(printf '%s\\n' \"$prompt\" | sed -n 's/^CONTEXT //p')\nactions='inspect observe-work'\nif grep -q 'true' \"$context\"; then actions=\"$actions recover\"; else actions=\"$actions pause-admission\"; fi\nfor action in $actions; do '{}' \"$action\"; done\n",
            capability.display()
        ),
    )
    .unwrap();
    fs::write(
        &capability,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
            invocations.display()
        ),
    )
    .unwrap();
    for path in [&provider, &capability] {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
    let previous = std::env::var_os("REFINE_SMOKE_AI_PATH");
    unsafe {
        std::env::set_var("REFINE_SMOKE_AI_PATH", &provider);
    }

    let operation = FileOperationRegistry::new(&runtime)
        .register_exclusive_with_request(
            "maintenance:source-upgrade",
            json!({"restart_recovery": "capability"}),
        )
        .unwrap();
    let process =
        crate::tools::host::agent_providers::HostAgentProviderService::with_runtime_root(&runtime)
            .launch_managed(crate::tools::host::agent_providers::ProviderInvocation {
                provider: "smoke-ai".to_string(),
                prompt: format!("CONTEXT {}", context.display()),
                session_id: None,
                cwd: Some(checkout.display().to_string()),
                process_metadata: serde_json::Map::from_iter([
                    ("kind".to_string(), json!("source_upgrade_agent")),
                    ("operation_id".to_string(), json!(&operation.id)),
                ]),
            })
            .unwrap();
    let supervisor = FileProcessSupervisor::new(&runtime);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while supervisor.wait(&process.id).unwrap().state == "running"
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        fs::read_to_string(&invocations)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["inspect", "observe-work", "recover"]
    );
    assert_eq!(process.owner, ProcessOwner::Agent);
    assert!(
        process
            .details
            .as_deref()
            .unwrap()
            .contains(&format!("\"operation_id\":\"{}\"", operation.id))
    );
    assert!(!runtime.join("workflow-automation-state.json").exists());

    unsafe {
        match previous {
            Some(value) => std::env::set_var("REFINE_SMOKE_AI_PATH", value),
            None => std::env::remove_var("REFINE_SMOKE_AI_PATH"),
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn interrupted_upgrade_agent_reconciles_pause_state_and_stays_retryable() {
    let root = test_directory("source-upgrade-interrupted");
    let runtime = root.join("run/8080");
    let service = FileSourcePromotionService::new(root.join("checkout"), &runtime, 8080);
    let supervisor = FileProcessSupervisor::new(&runtime);
    supervisor.set_workflow_paused(true).unwrap();
    let mut active = operation();
    active.status = "running".to_string();
    active.stage = "drain_work".to_string();
    active.agent_worker_process_id = Some("missing-agent-worker".to_string());
    active.pre_upgrade_workflow_paused = Some(false);
    service.save_operation(&active).unwrap();

    let reconciled = service.reconcile_interrupted_agent().unwrap().unwrap();
    assert_eq!(reconciled.status, "interrupted");
    assert_eq!(reconciled.stage, "agent_interrupted");
    assert!(reconciled.recovery.as_deref().unwrap().contains("retry"));
    assert_eq!(reconciled.workflow_pause_restored, Some(true));
    assert!(!supervisor.pause_state().unwrap().workflow_paused);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn queueing_an_update_stashes_dirty_work_and_records_the_reference_durably() {
    let repo = initialize_promotion_repository("source-upgrade-auto-stash");
    let runtime = repo.root.join("run/8080");
    let port = spawn_single_http_probe();
    let service = FileSourcePromotionService::new(&repo.checkout, &runtime, port);
    persist_cached_source(&service, &repo);
    fs::write(repo.checkout.join("fixture.txt"), "uncommitted local edit\n").unwrap();
    fs::write(repo.checkout.join("scratch.txt"), "untracked local work\n").unwrap();

    let error = service
        .queue_agent_with_failpoint(
            "smoke-ai",
            Path::new("/mock/refine"),
            AgentLaunchFailpoint::AfterReservation,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("after source-upgrade reservation")
    );

    assert!(
        git_text(&repo.checkout, &["status", "--porcelain"])
            .unwrap()
            .is_empty(),
        "the tree must be clean after the automatic stash"
    );
    let stash_list = git_text(&repo.checkout, &["stash", "list"]).unwrap();
    assert!(stash_list.contains("refine-update-"), "{stash_list}");

    let operation = service.load_operation().unwrap().unwrap();
    let reference = operation.stashed_changes.as_deref().unwrap();
    assert!(reference.contains("refine-update-"), "{reference}");
    assert!(
        operation.message.contains("preserved in stash"),
        "{}",
        operation.message
    );

    let cached = service.inspect_cached().unwrap();
    assert!(
        cached.source.clean,
        "the cached snapshot must reflect the stashed (clean) tree"
    );

    fs::remove_dir_all(repo.root).unwrap();
}

#[test]
fn queueing_never_stashes_when_the_checkout_diverged_from_upstream() {
    let repo = initialize_promotion_repository("source-upgrade-diverged-dirty");
    // Diverge: a local commit that upstream does not contain.
    fs::write(repo.checkout.join("local.txt"), "local divergent commit\n").unwrap();
    git_ok(&repo.checkout, &["add", "local.txt"]).unwrap();
    git_ok(
        &repo.checkout,
        &["commit", "--quiet", "-m", "local divergence"],
    )
    .unwrap();
    let runtime = repo.root.join("run/8080");
    let port = spawn_single_http_probe();
    let service = FileSourcePromotionService::new(&repo.checkout, &runtime, port);
    persist_cached_source(&service, &repo);
    fs::write(repo.checkout.join("fixture.txt"), "uncommitted local edit\n").unwrap();

    let error = service
        .queue_agent_with("smoke-ai", Path::new("/mock/refine"))
        .unwrap_err();
    assert!(error.to_string().contains("diverged"), "{error}");

    let status = git_text(&repo.checkout, &["status", "--porcelain"]).unwrap();
    assert!(
        status.contains("fixture.txt"),
        "the dirty diverged tree must be left untouched: {status}"
    );
    assert!(
        git_text(&repo.checkout, &["stash", "list"]).unwrap().is_empty(),
        "no stash may be created for a diverged checkout"
    );

    fs::remove_dir_all(repo.root).unwrap();
}

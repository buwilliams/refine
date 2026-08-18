use std::cell::{Cell, RefCell};

use super::*;

#[derive(Default)]
struct ProtocolLauncher {
    handoffs: RefCell<Vec<RestartSafeHandoff>>,
    live: Cell<bool>,
    terminated: Cell<bool>,
}

impl RestartSafeHandoffLauncher for ProtocolLauncher {
    fn launch(
        &self,
        handoff: &RestartSafeHandoff,
        _service_manager: Option<&str>,
    ) -> RefineResult<()> {
        self.handoffs.borrow_mut().push(handoff.clone());
        Ok(())
    }

    fn submit(
        &self,
        handoff: &RestartSafeHandoff,
        _service_manager: Option<&str>,
    ) -> RefineResult<HandoffLaunchReceipt> {
        self.handoffs.borrow_mut().push(handoff.clone());
        Ok(HandoffLaunchReceipt {
            mechanism: "detached".to_string(),
            mechanism_identity: format!("detached:{}", handoff_argument_fingerprint(handoff)),
            submitted_at: now_timestamp(),
            executable: handoff.executable.display().to_string(),
            argument_fingerprint: handoff_argument_fingerprint(handoff),
            pid: None,
            process_identity: None,
        })
    }

    fn observe(&self, _receipt: &HandoffLaunchReceipt) -> RefineResult<HandoffObservation> {
        Ok(if self.live.get() {
            HandoffObservation::Live
        } else {
            HandoffObservation::Exited
        })
    }

    fn terminate(&self, _receipt: &HandoffLaunchReceipt) -> RefineResult<()> {
        self.terminated.set(true);
        self.live.set(false);
        Ok(())
    }
}

#[test]
fn reservation_before_submission_settles_interrupted_after_deadline() {
    let (root, service, snapshot, mut operation) = protocol_fixture("reserve-crash");
    let launcher = ProtocolLauncher::default();
    let error = service
        .launch_operation_two_phase_with_failpoint(
            &mut operation,
            &snapshot,
            Path::new("/mock/refine"),
            &launcher,
            HandoffFailpoint::BeforeSubmission,
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("before restart-safe helper submission")
    );
    assert!(launcher.handoffs.borrow().is_empty());
    let registry = service.operation_registry();
    let reserved = registry.status(&operation.id).unwrap();
    assert_eq!(
        reserved.external_attempt.as_ref().unwrap().state,
        ExternalAttemptState::Reserved
    );
    registry
        .compare_and_set(&operation.id, reserved.revision, |candidate| {
            candidate
                .external_attempt
                .as_mut()
                .unwrap()
                .claim_deadline_at = "2000-01-01T00:00:00Z".to_string();
            Ok(())
        })
        .unwrap();
    service
        .reconcile_handoff_attempt(&mut operation, &launcher)
        .unwrap();
    assert_eq!(operation.status, "interrupted");
    assert_eq!(
        registry.status(&operation.id).unwrap().state,
        OperationState::Interrupted
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn submitted_before_receipt_adopts_atomic_helper_claim_after_restart() {
    let (root, service, snapshot, mut operation) = protocol_fixture("submitted-claim");
    let launcher = ProtocolLauncher::default();
    launcher.live.set(true);
    service
        .launch_operation_two_phase_with_failpoint(
            &mut operation,
            &snapshot,
            Path::new("/mock/refine"),
            &launcher,
            HandoffFailpoint::AfterSubmissionBeforeReceipt,
        )
        .unwrap_err();
    let handoff = launcher.handoffs.borrow()[0].clone();
    let attempt_id = arg_value(&handoff.args, "--attempt-id");
    let nonce = arg_value(&handoff.args, "--claim-nonce");
    let durable =
        serde_json::to_string(&service.operation_registry().status(&operation.id).unwrap())
            .unwrap();
    assert!(!durable.contains(nonce));
    assert!(durable.contains(&nonce_verifier(nonce)));
    service
        .claim_handoff(&operation.id, attempt_id, nonce)
        .unwrap();
    service
        .reconcile_handoff_attempt(&mut operation, &launcher)
        .unwrap();
    assert_eq!(operation.stage, "restart_safe_handoff");
    assert_eq!(
        operation.handoff_attempt.as_ref().unwrap().state,
        ExternalAttemptState::Active
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn receipt_before_activation_is_adopted_and_projection_repairs_from_registry() {
    let (root, service, snapshot, mut operation) = protocol_fixture("receipt-crash");
    let launcher = ProtocolLauncher::default();
    launcher.live.set(true);
    service
        .launch_operation_two_phase_with_failpoint(
            &mut operation,
            &snapshot,
            Path::new("/mock/refine"),
            &launcher,
            HandoffFailpoint::AfterReceiptBeforeActivation,
        )
        .unwrap_err();
    let received = service.operation_registry().status(&operation.id).unwrap();
    assert!(
        received
            .external_attempt
            .as_ref()
            .unwrap()
            .receipt
            .is_some()
    );
    fs::remove_file(service.state_path()).unwrap();
    let recovered = service.load_operation().unwrap().unwrap();
    assert_eq!(recovered.id, operation.id);
    service
        .reconcile_handoff_attempt(&mut operation, &launcher)
        .unwrap();
    assert_eq!(operation.stage, "restart_safe_handoff");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_and_late_helper_claims_are_fenced_by_attempt_ownership() {
    let (root, service, snapshot, mut operation) = protocol_fixture("claim-fencing");
    let launcher = ProtocolLauncher::default();
    service
        .launch_operation_two_phase_with_failpoint(
            &mut operation,
            &snapshot,
            Path::new("/mock/refine"),
            &launcher,
            HandoffFailpoint::AfterSubmissionBeforeReceipt,
        )
        .unwrap_err();
    let handoff = launcher.handoffs.borrow()[0].clone();
    let attempt_id = arg_value(&handoff.args, "--attempt-id");
    let nonce = arg_value(&handoff.args, "--claim-nonce");
    service
        .claim_handoff(&operation.id, attempt_id, nonce)
        .unwrap();
    assert!(
        service
            .claim_handoff(&operation.id, attempt_id, nonce)
            .unwrap_err()
            .to_string()
            .contains("authoritative claimant")
    );
    let handle = service.operation_registry().status(&operation.id).unwrap();
    service
        .operation_registry()
        .compare_and_set(&operation.id, handle.revision, |candidate| {
            candidate.state = OperationState::Interrupted;
            candidate.external_attempt.as_mut().unwrap().state = ExternalAttemptState::Interrupted;
            Ok(())
        })
        .unwrap();
    assert!(
        service
            .claim_handoff(&operation.id, attempt_id, nonce)
            .unwrap_err()
            .to_string()
            .contains("no longer accepts helper claims")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancellation_fences_then_terminates_exact_received_helper() {
    let (root, service, snapshot, mut operation) = protocol_fixture("cancel-race");
    let launcher = ProtocolLauncher::default();
    launcher.live.set(true);
    service
        .launch_operation_two_phase_with_failpoint(
            &mut operation,
            &snapshot,
            Path::new("/mock/refine"),
            &launcher,
            HandoffFailpoint::AfterReceiptBeforeActivation,
        )
        .unwrap_err();
    let cancelled = service
        .cancel_operation_with(&operation.id, &launcher)
        .unwrap();
    assert_eq!(cancelled.state, OperationState::Cancelled);
    assert!(launcher.terminated.get());
    assert_eq!(
        cancelled.external_attempt.unwrap().state,
        ExternalAttemptState::Cancelled
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancellation_during_source_mutation_waits_for_primary_rollback_evidence() {
    let (root, service, snapshot, mut operation) = protocol_fixture("cancel-mutation");
    let launcher = ProtocolLauncher::default();
    launcher.live.set(true);
    service
        .launch_operation_two_phase_with_failpoint(
            &mut operation,
            &snapshot,
            Path::new("/mock/refine"),
            &launcher,
            HandoffFailpoint::AfterReceiptBeforeActivation,
        )
        .unwrap_err();
    operation.stage = "activate_source".to_string();
    operation.message = "Source mutation is active".to_string();
    service.save_operation(&operation).unwrap();
    let error = service
        .cancel_operation_with(&operation.id, &launcher)
        .unwrap_err();
    assert!(error.to_string().contains("cannot terminate a helper"));
    assert!(!launcher.terminated.get());
    assert_eq!(
        service
            .operation_registry()
            .status(&operation.id)
            .unwrap()
            .state,
        OperationState::Cancelling
    );

    operation.status = "failed".to_string();
    operation.stage = "restore_workflow_admission".to_string();
    operation.rollback_attempted = true;
    operation.rollback_succeeded = Some(true);
    operation.workflow_pause_restored = Some(true);
    operation.error = Some("activation cancelled and rolled back".to_string());
    service.save_operation(&operation).unwrap();
    let settled = service
        .cancel_operation_with(&operation.id, &launcher)
        .unwrap();
    assert_eq!(settled.state, OperationState::Failed);
    assert_eq!(
        settled.external_attempt.unwrap().state,
        ExternalAttemptState::Failed
    );
    fs::remove_dir_all(root).unwrap();
}

fn protocol_fixture(
    name: &str,
) -> (
    PathBuf,
    FileSourcePromotionService,
    SourcePromotionSnapshot,
    SourcePromotionOperation,
) {
    let root = std::env::temp_dir().join(format!(
        "refine-handoff-protocol-{name}-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    let checkout = root.join("checkout");
    let runtime = root.join("run/8080");
    fs::create_dir_all(&checkout).unwrap();
    let service = FileSourcePromotionService::new(&checkout, &runtime, 8080);
    let snapshot = SourcePromotionSnapshot {
        checkout_path: checkout.display().to_string(),
        current_commit: "aaa".to_string(),
        remote: "origin".to_string(),
        local_branch: "main".to_string(),
        branch: "main".to_string(),
        available_commit: "bbb".to_string(),
        relationship: "behind".to_string(),
        clean: true,
        fast_forward: true,
        update_available: true,
        active_work: Vec::new(),
        operation: None,
    };
    let mut operation = SourcePromotionOperation::queued(&snapshot);
    operation.pre_upgrade_workflow_paused = Some(false);
    service
        .operation_registry()
        .register_with_id(
            &operation.id,
            "maintenance:source-upgrade",
            json!({
                "restart_recovery": "capability",
                "defer_cancellation_terminal": true
            }),
        )
        .unwrap();
    service.save_operation(&operation).unwrap();
    (root, service, snapshot, operation)
}

fn arg_value<'a>(args: &'a [String], name: &str) -> &'a str {
    let index = args.iter().position(|arg| arg == name).unwrap();
    args[index + 1].as_str()
}

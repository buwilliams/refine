use super::*;

pub fn run_source_promotion<H, F>(
    host: &mut H,
    operation: &mut SourcePromotionOperation,
    mut persist: F,
) -> RefineResult<()>
where
    H: SourcePromotionHost,
    F: FnMut(&SourcePromotionOperation) -> RefineResult<()>,
{
    update_operation(
        operation,
        "running",
        "build_candidate",
        "Building the fetched source candidate before activation",
    );
    persist(operation)?;
    let candidate = match host.build_candidate(&operation.to_commit) {
        Ok(candidate) => candidate,
        Err(error) => return fail_operation(operation, "build_candidate", error, &mut persist),
    };

    update_operation(
        operation,
        "running",
        "verify_idle",
        "Candidate built; rechecking checkout safety and runtime quiescence",
    );
    persist(operation)?;
    if let Err(error) = host.verify_preconditions(&operation.from_commit, &operation.to_commit) {
        return fail_operation(operation, "verify_idle", error, &mut persist);
    }

    update_operation(
        operation,
        "running",
        "prepare_service_registration",
        "Candidate built; preparing the installed service registration",
    );
    operation.candidate_executable = Some(candidate.display().to_string());
    persist(operation)?;
    match host.prepare_restart(&candidate) {
        Ok(Some(update)) => {
            operation.service_manager = Some(update.service_manager);
            operation.registration_updated = true;
            persist(operation)?;
        }
        Ok(None) => {}
        Err(error) => {
            return fail_operation(
                operation,
                "prepare_service_registration",
                error,
                &mut persist,
            );
        }
    }

    update_operation(
        operation,
        "running",
        "stop_daemon",
        "Candidate built; stopping the Refine daemon",
    );
    persist(operation)?;
    if let Err(error) = host.stop_daemon() {
        settle_registration_rollback(host, operation);
        return fail_operation(operation, "stop_daemon", error, &mut persist);
    }

    update_operation(
        operation,
        "running",
        "activate_source",
        "Daemon stopped; activating the fast-forward source commit",
    );
    persist(operation)?;
    if let Err(error) = host.activate(&operation.from_commit, &operation.to_commit) {
        let source_verification = host.verify_previous_source(&operation.from_commit);
        let source_restored = Some(source_verification.is_ok());
        let source_error = source_verification.err().map(|verification_error| {
            format!(
                "prior source verification after activation failure did not settle: {verification_error}"
            )
        });
        settle_daemon_rollback(host, operation, source_restored, &error, &mut persist)?;
        if let Some(source_error) = source_error
            && let Some(evidence) = operation.rollback_evidence.as_mut()
        {
            evidence.errors.insert(0, source_error);
            operation.rollback_succeeded = Some(false);
            operation.recovery = Some(rollback_recovery(operation));
        }
        return fail_operation(operation, "activate_source", error, &mut persist);
    }

    update_operation(
        operation,
        "running",
        "restart_daemon",
        "Source activated; restarting Refine from the candidate binary",
    );
    persist(operation)?;
    let restart_result = host
        .restart_daemon(&candidate)
        .and_then(|_| host.verify_daemon(&operation.to_commit, &candidate));
    if let Err(error) = restart_result {
        let source_rollback = host.rollback(&operation.from_commit, &operation.to_commit);
        let source_restored = Some(source_rollback.is_ok());
        let source_error = source_rollback.err().map(|rollback_error| {
            format!(
                "source rollback to {} failed: {rollback_error}",
                operation.from_commit
            )
        });
        settle_daemon_rollback(host, operation, source_restored, &error, &mut persist)?;
        if let Some(source_error) = source_error
            && let Some(evidence) = operation.rollback_evidence.as_mut()
        {
            evidence.errors.insert(0, source_error);
            operation.rollback_succeeded = Some(false);
            operation.recovery = Some(rollback_recovery(operation));
        }
        return fail_operation(operation, "restart_daemon", error, &mut persist);
    }
    operation.observed_executable = restart_result
        .ok()
        .map(|executable| executable.display().to_string());
    if let Err(error) = host.complete_restart_registration() {
        operation.recovery = Some(
            "The candidate daemon identity was verified, but the durable registration backup could not be finalized; inspect the backup before another promotion"
                .to_string(),
        );
        return fail_operation(
            operation,
            "finalize_service_registration",
            error,
            &mut persist,
        );
    }

    update_operation(
        operation,
        "succeeded",
        "complete",
        "Latest source promoted and Refine is healthy",
    );
    operation.recovery = None;
    persist(operation)
}

fn settle_registration_rollback<H: SourcePromotionHost>(
    host: &mut H,
    operation: &mut SourcePromotionOperation,
) {
    if operation.registration_updated {
        let restored = host.restore_restart_registration();
        operation.registration_rollback_succeeded = Some(restored.is_ok());
        let verified = restored
            .and_then(|restored| {
                if restored {
                    host.verify_previous_registration().map(|_| ())
                } else {
                    Err(RefineError::Conflict(
                        "service registration rollback backup was missing".to_string(),
                    ))
                }
            })
            .and_then(|_| host.complete_restart_registration());
        operation.recovery = Some(if operation.registration_rollback_succeeded == Some(true) {
            if verified.is_ok() {
                "The service registration was restored and verified after shutdown failed; the fresh lifecycle evidence determines whether the prior daemon is still active"
            } else {
                "The service registration was restored after shutdown failed but could not be verified or finalized; inspect the durable backup and fresh lifecycle evidence before retrying"
            }
                .to_string()
        } else {
            "Shutdown and service-registration rollback did not both settle; inspect the durable registration backup and daemon reachability before retrying"
                .to_string()
        });
    }
}

fn restore_registration<H: SourcePromotionHost>(
    host: &mut H,
    operation: &mut SourcePromotionOperation,
) -> RefineResult<bool> {
    if !operation.registration_updated {
        return Ok(false);
    }
    let result = host.restore_restart_registration().and_then(|restored| {
        if restored {
            Ok(true)
        } else {
            Err(RefineError::Conflict(
                "service registration rollback was required, but its durable backup was missing"
                    .to_string(),
            ))
        }
    });
    operation.registration_rollback_succeeded = Some(result.is_ok());
    result
}

fn settle_daemon_rollback<H, F>(
    host: &mut H,
    operation: &mut SourcePromotionOperation,
    source_restored: Option<bool>,
    original_error: &RefineError,
    persist: &mut F,
) -> RefineResult<()>
where
    H: SourcePromotionHost,
    F: FnMut(&SourcePromotionOperation) -> RefineResult<()>,
{
    operation.rollback_attempted = true;
    operation.error = Some(original_error.to_string());
    operation.stage = "rollback_restore_registration".to_string();
    operation.message =
        "Candidate restart failed; restoring and verifying the prior service registration"
            .to_string();
    operation.rollback_evidence = Some(SourcePromotionRollbackEvidence {
        source_restored,
        reachability: "not_checked".to_string(),
        ..Default::default()
    });
    persist(operation)?;

    if operation.registration_updated {
        let restored = restore_registration(host, operation);
        let evidence = operation.rollback_evidence.as_mut().unwrap();
        evidence.registration_restored = Some(restored.is_ok());
        if let Err(error) = restored {
            evidence
                .errors
                .push(format!("service registration restoration failed: {error}"));
        } else {
            match host.verify_previous_registration() {
                Ok(executable) => {
                    evidence.registration_verified = Some(true);
                    evidence.registered_executable = Some(executable.display().to_string());
                }
                Err(error) => {
                    evidence.registration_verified = Some(false);
                    evidence.errors.push(format!(
                        "restored service registration verification failed: {error}"
                    ));
                }
            }
        }
    }
    persist(operation)?;

    let registration_settled = !operation.registration_updated
        || operation
            .rollback_evidence
            .as_ref()
            .is_some_and(|evidence| evidence.registration_verified == Some(true));
    let source_settled = source_restored == Some(true);
    if source_settled && registration_settled {
        operation.stage = "rollback_replace_daemon".to_string();
        operation.message =
            "Prior registration restored; forcibly replacing the candidate daemon".to_string();
        let replacement = host.restart_previous_daemon();
        let evidence = operation.rollback_evidence.as_mut().unwrap();
        evidence.replacement_attempted = true;
        evidence.replacement_succeeded = Some(replacement.is_ok());
        if let Err(error) = replacement {
            evidence
                .errors
                .push(format!("forced prior daemon replacement failed: {error}"));
        }
    } else {
        let evidence = operation.rollback_evidence.as_mut().unwrap();
        evidence.errors.push(
            "forced daemon replacement was withheld because source or registration restoration was not authoritatively verified"
                .to_string(),
        );
    }
    persist(operation)?;

    operation.stage = "rollback_verify_daemon".to_string();
    operation.message =
        "Fresh-probing reachability and live executable identity after rollback".to_string();
    let observation = host.observe_previous_daemon();
    operation.observed_executable = observation.observed_executable.clone();
    let observation_verified = observation.verified();
    {
        let evidence = operation.rollback_evidence.as_mut().unwrap();
        evidence.reachability = observation.reachability;
        evidence.expected_executable = Some(observation.expected_executable);
        evidence.observed_executable = observation.observed_executable;
        evidence.identity_matches = observation.identity_matches;
        if let Some(error) = observation.error {
            evidence.errors.push(error);
        }
    }

    let replacement_succeeded = operation
        .rollback_evidence
        .as_ref()
        .and_then(|evidence| evidence.replacement_succeeded)
        == Some(true);
    let mut rollback_succeeded =
        source_settled && registration_settled && replacement_succeeded && observation_verified;
    if rollback_succeeded
        && operation.registration_updated
        && let Err(error) = host.complete_restart_registration()
    {
        operation
            .rollback_evidence
            .as_mut()
            .unwrap()
            .errors
            .push(format!(
                "verified registration rollback could not be finalized: {error}"
            ));
        rollback_succeeded = false;
    }
    operation.rollback_succeeded = Some(rollback_succeeded);
    operation.recovery = Some(rollback_recovery(operation));
    persist(operation)
}

fn rollback_recovery(operation: &SourcePromotionOperation) -> String {
    if operation.rollback_succeeded == Some(true) {
        return format!(
            "Refine was authoritatively restored to {} with its prior registered and live executable; inspect the original promotion failure before retrying",
            operation.from_commit
        );
    }
    let evidence = operation.rollback_evidence.as_ref();
    let reachability = evidence
        .map(|evidence| evidence.reachability.as_str())
        .unwrap_or("not_checked");
    let observed = evidence
        .and_then(|evidence| evidence.observed_executable.as_deref())
        .unwrap_or("unknown");
    format!(
        "Automatic rollback is partial or indeterminate (reachability: {reachability}, observed executable: {observed}); inspect the durable registration backup and service manager, terminate any remaining candidate, restore {} at {}, then force `./r system restart --port <port>` and verify `/system/version`",
        operation.from_commit, operation.checkout_path
    )
}

fn fail_operation<F>(
    operation: &mut SourcePromotionOperation,
    stage: &str,
    error: RefineError,
    persist: &mut F,
) -> RefineResult<()>
where
    F: FnMut(&SourcePromotionOperation) -> RefineResult<()>,
{
    operation.status = "failed".to_string();
    operation.stage = stage.to_string();
    operation.message = format!("Source promotion failed during {stage}");
    operation.error = Some(error.to_string());
    operation.updated_at = now_timestamp();
    if operation.recovery.is_none() {
        operation.recovery = Some(
            "Resolve the reported stage failure, then check for source updates again".to_string(),
        );
    }
    persist(operation)?;
    Err(error)
}

fn update_operation(
    operation: &mut SourcePromotionOperation,
    status: &str,
    stage: &str,
    message: &str,
) {
    operation.status = status.to_string();
    operation.stage = stage.to_string();
    operation.message = message.to_string();
    operation.error = None;
    operation.updated_at = now_timestamp();
}

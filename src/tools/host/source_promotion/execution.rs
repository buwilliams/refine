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
        "stop_daemon",
        "Candidate built; stopping the Refine daemon",
    );
    persist(operation)?;
    if let Err(error) = host.stop_daemon() {
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
        let _ = host.restart_previous_daemon();
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
        .and_then(|_| host.verify_daemon(&operation.to_commit));
    if let Err(error) = restart_result {
        operation.rollback_attempted = true;
        let rollback = host
            .rollback(&operation.from_commit, &operation.to_commit)
            .and_then(|_| host.restart_previous_daemon());
        operation.rollback_succeeded = Some(rollback.is_ok());
        operation.recovery = Some(if rollback.is_ok() {
            format!(
                "Refine was restored to {}; inspect the restart failure before retrying",
                operation.from_commit
            )
        } else {
            format!(
                "Automatic rollback failed; from {} restore ref {} to {} and run `./r system start --port <port>`",
                operation.checkout_path, operation.from_commit, operation.from_commit
            )
        });
        return fail_operation(operation, "restart_daemon", error, &mut persist);
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

use super::*;

pub(super) fn run_publication(
    registry: &FileOperationRegistry,
    operation_id: &str,
    host: &mut dyn ReleaseHost,
    preparation: &TrustedPreparation,
) -> RefineResult<PublishedRelease> {
    stage(
        registry,
        operation_id,
        "preflight",
        "Checking clean synchronized main, merge ancestry, version, tags, remote, and credentials",
        Some(&preparation.goal_id),
    )?;
    let preflight = host.preflight(preparation)?;
    stage(
        registry,
        operation_id,
        "local_tag",
        "Creating or validating the local semantic tag",
        Some(&preparation.goal_id),
    )?;
    host.ensure_local_tag(preparation, &preflight)?;
    stage(
        registry,
        operation_id,
        "remote_tag",
        "Pushing or validating the remote semantic tag",
        Some(&preparation.goal_id),
    )?;
    host.ensure_remote_tag(preparation, &preflight)?;
    stage(
        registry,
        operation_id,
        "github_release",
        "Creating or validating the GitHub release",
        Some(&preparation.goal_id),
    )?;
    host.ensure_github_release(preparation, &preflight)?;
    stage(
        registry,
        operation_id,
        "delivery",
        "Observing deployment and package workflows to a terminal result",
        Some(&preparation.goal_id),
    )?;
    let deployment = host.observe_delivery(preparation, &preflight)?;
    stage(
        registry,
        operation_id,
        "verify",
        "Verifying the published tag and GitHub release",
        Some(&preparation.goal_id),
    )?;
    let release_url = host.verify(preparation, &preflight)?;
    Ok(PublishedRelease {
        version: preparation.version.clone(),
        tag: preparation.tag.clone(),
        commit: preflight.main_commit,
        remote: preflight.remote,
        deployment,
        release_url,
        verified: true,
    })
}

pub(super) fn stage(
    registry: &FileOperationRegistry,
    operation_id: &str,
    name: &str,
    message: &str,
    goal_id: Option<&str>,
) -> RefineResult<()> {
    registry.update_progress(operation_id, json!({"stage": name, "message": message}))?;
    registry.append_log(
        operation_id,
        LogEntry {
            datetime: String::new(),
            severity: "info".to_string(),
            category: "release".to_string(),
            message: message.to_string(),
            actor: Some("release-service".to_string()),
            goal_id: goal_id.map(ToString::to_string),
            actions: Vec::new(),
            details: Some(json!({"stage": name}).as_object().unwrap().clone()),
        },
    )?;
    Ok(())
}

pub(super) fn operation_json(operation: OperationHandle) -> Value {
    json!({
        "id": operation.id,
        "owner": operation.owner,
        "status": operation.state.as_api_status(),
        "progress": operation.progress,
        "result": operation.result,
        "error": operation.error,
    })
}

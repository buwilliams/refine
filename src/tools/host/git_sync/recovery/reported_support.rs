use super::*;
use crate::tools::host::git_sync::conflict_report::{conflict_report_id, state_map_digest};

pub(super) fn validate_report_identity(report: &StateSyncConflictReport) -> RefineResult<()> {
    if !matches!(report.version, 1 | 2)
        || report.report_id.is_empty()
        || report.report_id != conflict_report_id(report)?
        || report.unresolved_paths.is_empty()
    {
        return Err(RefineError::InvalidInput(
            "The local state-sync conflict report is invalid or incomplete.".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_report_authority(
    service: &FileGitSyncService,
    report: &StateSyncConflictReport,
    remote: &str,
) -> RefineResult<()> {
    if remote != report.configured_remote
        || service.target_identity()? != report.target_identity
        || service.repository_identity(remote)? != report.repository_identity
    {
        return Err(stale_recovery(
            "the target, repository, or configured remote identity changed",
        ));
    }
    Ok(())
}

pub(super) fn validate_reported_preview_shape(preview: &StateRecoveryPreview) -> RefineResult<()> {
    if preview.version != REPORTED_RECOVERY_VERSION
        || preview.baseline_status != "valid_conflict"
        || preview.baseline_snapshot.is_none()
        || preview.conflict_report_id.is_none()
        || preview.conflicting_paths.is_empty()
        || preview.conflicting_paths_truncated != 0
        || preview.evidence_id.is_empty()
        || preview.evidence_id != preview_evidence_id(preview)?
    {
        return Err(RefineError::InvalidInput(
            "The supplied valid-baseline recovery preview is invalid or incomplete.".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_recovery_decision(
    decision: &StateRecoveryDecision,
    conflicts: &[String],
) -> RefineResult<BTreeMap<String, StateRecoveryAuthority>> {
    let reported = conflicts.iter().cloned().collect::<BTreeSet<_>>();
    let mut overrides = BTreeMap::new();
    for entry in &decision.overrides {
        let normalized = PathBuf::from(&entry.path)
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if normalized.is_empty()
            || normalized.starts_with("../")
            || PathBuf::from(&entry.path).is_absolute()
        {
            return Err(RefineError::InvalidInput(format!(
                "Recovery override path {} is invalid.",
                entry.path
            )));
        }
        if !reported.contains(&normalized) {
            return Err(RefineError::InvalidInput(format!(
                "Recovery override path {normalized} is outside the reported conflict set."
            )));
        }
        if overrides
            .insert(normalized.clone(), entry.authority)
            .is_some()
        {
            return Err(RefineError::InvalidInput(format!(
                "Recovery override path {normalized} was supplied more than once."
            )));
        }
    }
    Ok(overrides)
}

pub(super) fn recovery_decision_id(decision: &StateRecoveryDecision) -> RefineResult<String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(decision).map_err(|error| {
        RefineError::Serialization(format!("failed to encode recovery decision: {error}"))
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn validate_owned_manifest(
    manifest: &StateRecoveryManifest,
    preview: &StateRecoveryPreview,
    decision: &StateRecoveryDecision,
    decision_id: &str,
) -> RefineResult<()> {
    if manifest.version != REPORTED_RECOVERY_VERSION
        || manifest.evidence_id != preview.evidence_id
        || manifest.authority != decision.default_authority
        || manifest.overrides != decision.overrides
        || manifest.decision_id != decision_id
        || manifest.target_identity != preview.target_identity
        || manifest.repository_identity != preview.repository_identity
        || manifest.configured_remote != preview.configured_remote
        || manifest.local_state_head_before != preview.local_state_head
        || manifest.remote_state_head_before != preview.remote_state_head
        || manifest.live_snapshot_before != preview.live_snapshot
        || manifest.baseline_snapshot_before != preview.baseline_snapshot
        || manifest.conflict_report_id != preview.conflict_report_id
    {
        return Err(RefineError::Conflict(
            "Recovery retry does not match the exact owned preview and decision set.".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_owned_target(
    service: &FileGitSyncService,
    target_ref: &str,
    target_head: &str,
    target_snapshot: &str,
) -> RefineResult<()> {
    if target_ref.is_empty()
        || !service.git_success(&["show-ref", "--verify", "--quiet", target_ref])?
        || service.git_stdout(&["rev-parse", target_ref])? != target_head
    {
        return Err(RefineError::Conflict(
            "Recovery target ref is missing, moved, or unowned.".to_string(),
        ));
    }
    let target = service.observe_repository_ref(target_ref)?;
    let target_refine = target.path.join(".refine");
    let state = durable_state_map(&target_refine)?;
    if state_map_digest(&state) != target_snapshot {
        return Err(RefineError::Conflict(
            "Recovery target ref does not match the manifest snapshot.".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn replace_target_path(
    source_root: &std::path::Path,
    target_root: &std::path::Path,
    relative: &std::path::Path,
    source_state: &DurableStateMap,
) -> RefineResult<()> {
    let target = target_root.join(relative);
    if source_state.contains_key(relative) {
        copy_state_file(&source_root.join(relative), &target)
    } else if target.exists() {
        fs::remove_file(&target).map_err(|error| {
            RefineError::Io(format!(
                "failed to apply recovery deletion {}: {error}",
                target.display()
            ))
        })
    } else {
        Ok(())
    }
}

pub(super) fn verify_owned_heads(
    service: &FileGitSyncService,
    remote: &str,
    target_head: &str,
) -> RefineResult<()> {
    if service.remote_state_head(remote)?.as_deref() != Some(target_head)
        || service.local_state_head()?.as_deref() != Some(target_head)
    {
        return Err(stale_recovery(
            "the local or remote refine/state head moved from the owned target",
        ));
    }
    Ok(())
}

pub(super) fn reported_result(
    preview: &StateRecoveryPreview,
    decision: &StateRecoveryDecision,
    manifest_path: &std::path::Path,
    manifest: StateRecoveryManifest,
) -> RefineResult<StateRecoveryResult> {
    Ok(StateRecoveryResult {
        ok: true,
        authority: decision.default_authority,
        overrides: decision.overrides.clone(),
        baseline_created: true,
        local_state_head: manifest.local_state_head_after,
        remote_state_head: manifest.remote_state_head_after.unwrap_or_default(),
        recovery_location: manifest.recovery_location,
        manifest_path: manifest_path.display().to_string(),
        path_counts: preview.path_counts.clone(),
        detail: manifest.message,
    })
}

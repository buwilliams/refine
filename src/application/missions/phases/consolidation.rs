//! Consolidation: final Mission approval starts deterministic consolidation;
//! there is no separate ordinary Publish action.
//!
//! The two-commit receipt avoids making a manifest claim the identity of the
//! commit containing itself: `C` is the Outcome publication commit; `D`
//! proves that Refine durably recorded the publication and terminal
//! transition. Every Outcome path is read back from `C` with `git show` and
//! its exact bytes verified; only after `D` is read back is the Mission
//! exposed as Done. A crash between steps is idempotently recoverable. See
//! `docs/mission-spec.md` ("Consolidate and Done").

use std::path::Path;

use serde_json::{Value, json};

use crate::application::missions::persistence::outcome_manifest_path;
use crate::application::missions::service::FileMissionService;
use crate::application::persistence_sync::state::FileGitSyncService;
use crate::error::{RefineError, RefineResult};
use crate::model::mission::{Mission, MissionStatus, OutcomePublication};

use super::current_round;

/// The path of one Outcome manifest relative to the live state root.
pub fn outcome_state_relative_path(mission_id: &str, round: usize) -> RefineResult<String> {
    let Some(path) = outcome_manifest_path(Path::new(""), mission_id, round) else {
        return Err(RefineError::InvalidInput(format!(
            "Mission id must be at least three characters: {mission_id}"
        )));
    };
    Ok(path.to_string_lossy().replace('\\', "/"))
}

/// Consolidate the approved Outcome: write the manifest bytes, prove them
/// from the publication commit, record the receipt, and expose Done only
/// after the terminal record is durably synchronized.
pub fn consolidate(
    service: &FileMissionService,
    target_root: &Path,
    runtime_root: &Path,
    mission_id: &str,
) -> RefineResult<Mission> {
    let mission = service.show_mission(mission_id)?;
    if mission.status != MissionStatus::Consolidate {
        return Err(RefineError::Conflict(format!(
            "Mission {} is in {}; consolidation requires the Consolidate phase",
            mission.id,
            mission.status.as_str()
        )));
    }
    let round = current_round(&mission)?;
    let round_number = round.number;
    let Some(manifest) = round.outcome.clone() else {
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} Round {round_number} has no approved Outcome"
        )));
    };

    // 1. Write the immutable Outcome manifest bytes into live state.
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        RefineError::Serialization(format!("failed to encode Outcome manifest: {error}"))
    })?;
    let Some(manifest_path) = outcome_manifest_path(&service.refine_dir, mission_id, round_number)
    else {
        return Err(RefineError::InvalidInput(
            "Mission id must be at least three characters".to_string(),
        ));
    };
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!(
                "failed to create Outcome directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    crate::application::missions::persistence::write_immutable_file(
        &manifest_path,
        &manifest_bytes,
    )?;

    // 2. Synchronize; the state commit containing the manifest is `C`.
    let sync = FileGitSyncService::new(target_root, runtime_root);
    let result = sync.sync()?;
    if !result.ok {
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} Outcome publication sync failed: {}",
            result
                .detail
                .unwrap_or_else(|| "unknown sync failure".to_string())
        )));
    }
    let commit_c = result.commit.clone().ok_or_else(|| {
        RefineError::Conflict(format!(
            "Mission {mission_id} Outcome publication produced no state commit"
        ))
    })?;

    // 3. Read every Outcome path back from `C` and verify exact bytes.
    let relative = outcome_state_relative_path(mission_id, round_number)?;
    let state_path = format!(".refine/{relative}");
    let read_back = sync.state_bytes_at_commit(&commit_c, &state_path)?;
    let verified = read_back == Some(manifest_bytes);
    if !verified {
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} Outcome manifest read-back from commit {commit_c} did not match the written bytes"
        )));
    }

    // 4. Record the publication receipt for `C` and the terminal state.
    let publication = OutcomePublication {
        manifest_digest: manifest.manifest_digest.clone(),
        outcome_state_commit: Some(commit_c.clone()),
        verified_path_digests: vec![manifest.manifest_digest.clone().unwrap_or_default()],
        published_by: mission.reporter.clone(),
        verified_at: Some(
            crate::application::missions::service::FileMissionService::now_timestamp(),
        ),
    };
    let mission = service.record_publication(mission_id, publication, Some(mission.revision))?;
    let _mission =
        service.transition_mission(mission_id, MissionStatus::Done, Some(mission.revision))?;

    // 5. Synchronize the terminal record; commit `D` proves durability.
    let terminal = sync.sync()?;
    if !terminal.ok {
        return Err(RefineError::Conflict(format!(
            "Mission {mission_id} terminal record sync failed: {}",
            terminal
                .detail
                .unwrap_or_else(|| "unknown sync failure".to_string())
        )));
    }
    if let Some(commit_d) = terminal.commit.clone() {
        let mission_path = mission_json_state_path(&service.refine_dir, mission_id)?;
        let read_back = sync.state_bytes_at_commit(&commit_d, &mission_path)?;
        let done_exposed = read_back
            .as_ref()
            .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
            .and_then(|record| {
                record
                    .get("status")
                    .and_then(Value::as_str)
                    .map(|status| status == "done")
            })
            .unwrap_or(false);
        if !done_exposed {
            return Err(RefineError::Conflict(format!(
                "Mission {mission_id} terminal read-back from commit {commit_d} did not expose the Done state"
            )));
        }
    }

    let evidence = json!({
        "stage": "consolidation",
        "outcome_state_commit": commit_c,
        "terminal_state_commit": terminal.commit,
        "verified_paths": vec![state_path],
    });
    super::write_phase_evidence(service, mission_id, "consolidation", evidence)?;
    service.show_mission(mission_id)
}

fn mission_json_state_path(refine_dir: &Path, mission_id: &str) -> RefineResult<String> {
    let Some(path) =
        crate::application::missions::persistence::mission_json_path(refine_dir, mission_id)
    else {
        return Err(RefineError::InvalidInput(
            "Mission id must be at least three characters".to_string(),
        ));
    };
    let relative = path
        .strip_prefix(refine_dir)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    Ok(format!(".refine/{relative}"))
}

use super::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineReconstructionSource {
    RetainedSnapshot,
    LegacyHistory,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateSyncReconciliationKind {
    ThreeWayMerged,
    BaselineUnavailableFallbackMerged,
    BaselineBytesUnavailable,
    SemanticMergeRejected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateSyncReconciliationOutcome {
    pub path: String,
    pub outcome: StateSyncReconciliationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_source: Option<BaselineReconstructionSource>,
    pub detail: String,
}

pub(super) enum BaselineFileResolution {
    Loaded {
        bytes: Vec<u8>,
        source: BaselineReconstructionSource,
    },
    Unavailable,
}

pub(super) struct PreparedSemanticChanges {
    pub changes: BTreeMap<PathBuf, Vec<u8>>,
    pub outcomes: Vec<StateSyncReconciliationOutcome>,
}

impl FileGitSyncService {
    pub(super) fn prepare_semantic_state_changes(
        &self,
        state_root: &std::path::Path,
        live_refine: &std::path::Path,
        state_refine: &std::path::Path,
        base: &DurableStateMap,
        local: &DurableStateMap,
        remote: &DurableStateMap,
    ) -> RefineResult<PreparedSemanticChanges> {
        let unresolved = BTreeSet::new();
        let mut changes = BTreeMap::new();
        let mut outcomes = Vec::new();
        for relative in state_conflict_paths(base, local, remote, &unresolved) {
            if relative != std::path::Path::new("nodes.json") && !is_goal_record(&relative) {
                continue;
            }
            let (Some(base_fingerprint), Some(_), Some(_)) = (
                base.get(&relative),
                local.get(&relative),
                remote.get(&relative),
            ) else {
                continue;
            };
            let local_bytes = read_reconciliation_file(&live_refine.join(&relative))?;
            let remote_bytes = read_reconciliation_file(&state_refine.join(&relative))?;
            let path = relative.to_string_lossy().replace('\\', "/");
            match self.reconstruct_baseline_file(state_root, &relative, *base_fingerprint)? {
                BaselineFileResolution::Loaded { bytes, source } => {
                    let merged = if relative == std::path::Path::new("nodes.json") {
                        merge_node_registry(&bytes, &local_bytes, &remote_bytes)
                    } else {
                        merge_goal_record(&bytes, &local_bytes, &remote_bytes)
                    };
                    if let Some(merged) = merged {
                        changes.insert(relative, merged);
                        outcomes.push(StateSyncReconciliationOutcome {
                            path,
                            outcome: StateSyncReconciliationKind::ThreeWayMerged,
                            baseline_source: Some(source),
                            detail: format!("three-way semantic merge used {source:?}"),
                        });
                    } else {
                        outcomes.push(StateSyncReconciliationOutcome {
                            path,
                            outcome: StateSyncReconciliationKind::SemanticMergeRejected,
                            baseline_source: Some(source),
                            detail: "semantic merge rejected ambiguous or malformed state"
                                .to_string(),
                        });
                    }
                }
                BaselineFileResolution::Unavailable
                    if relative == std::path::Path::new("nodes.json") =>
                {
                    if let Some(merged) =
                        merge_node_registry_without_base(&local_bytes, &remote_bytes)
                    {
                        changes.insert(relative, merged);
                        outcomes.push(StateSyncReconciliationOutcome {
                            path,
                            outcome:
                                StateSyncReconciliationKind::BaselineUnavailableFallbackMerged,
                            baseline_source: None,
                            detail: "recorded baseline bytes were unavailable; safe two-way per-node reconciliation succeeded"
                                .to_string(),
                        });
                    } else {
                        outcomes.push(StateSyncReconciliationOutcome {
                            path,
                            outcome: StateSyncReconciliationKind::BaselineBytesUnavailable,
                            baseline_source: None,
                            detail: "recorded baseline bytes were unavailable and safe two-way per-node reconciliation rejected malformed or equal-time disagreement"
                                .to_string(),
                        });
                    }
                }
                BaselineFileResolution::Unavailable => {
                    outcomes.push(StateSyncReconciliationOutcome {
                        path,
                        outcome: StateSyncReconciliationKind::BaselineBytesUnavailable,
                        baseline_source: None,
                        detail: "recorded baseline bytes were unavailable; no baseline-less authority exists for this path"
                            .to_string(),
                    });
                }
            }
        }
        Ok(PreparedSemanticChanges { changes, outcomes })
    }
}

fn read_reconciliation_file(path: &std::path::Path) -> RefineResult<Vec<u8>> {
    fs::read(path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read conflicting Refine state {}: {error}",
            path.display()
        ))
    })
}

//! Shared live/exited/unverified ownership semantics for admission, recovery and cleanup.
use super::*;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OwnershipAssessment {
    Live,
    Exited,
    Unverified { reason: String },
}
impl OwnershipAssessment {
    pub fn pending(&self) -> bool {
        !matches!(self, Self::Exited)
    }
}

impl FileProcessSupervisor {
    pub fn assess_owned_group(&self, group: &OwnedGroup) -> RefineResult<OwnershipAssessment> {
        let observed = match self.observe_owned_group(group) {
            Ok(observed) => observed,
            Err(error) => {
                return Ok(OwnershipAssessment::Unverified {
                    reason: error.to_string(),
                });
            }
        };
        Ok(if observed.confirmed_exit {
            OwnershipAssessment::Exited
        } else if let Some(reason) = observed.ownership_gap {
            OwnershipAssessment::Unverified { reason }
        } else {
            OwnershipAssessment::Live
        })
    }

    pub(super) fn assess_group_members(
        &self,
        group: &OwnedGroup,
    ) -> RefineResult<(OwnershipAssessment, BTreeMap<u32, String>)> {
        if self.runtime_root.canonicalize().ok().as_ref() != Some(&group.runtime_root) {
            return Err(RefineError::Conflict("owned group runtime changed".into()));
        }
        // Cached uncertainty is diagnostic history, not a permanent veto on
        // newer authoritative lifetime evidence. Reassess the launch owner.
        #[cfg(target_os = "linux")]
        let gap = match &group.launch_scope {
            Some(scope) => {
                if scope.proof(group)? {
                    return Ok((OwnershipAssessment::Exited, BTreeMap::new()));
                }
                if scope.alive()? {
                    None
                } else if scope.proof(group)? {
                    // The guardian can publish its receipt between probes.
                    return Ok((OwnershipAssessment::Exited, BTreeMap::new()));
                } else {
                    Some("launch ownership guardian disappeared without complete exit proof".into())
                }
            }
            None => Some("registration has no complete launch-time ownership coverage; descendants may have escaped observation".into()),
        };
        #[cfg(not(target_os = "linux"))]
        let gap = Some("complete ownership inspection is unavailable on this platform".into());
        let members = if group.pgid.is_none() && gap.is_some() {
            BTreeMap::new()
        } else {
            group_members(group)?
        };
        if let Some(reason) = gap {
            return Ok((OwnershipAssessment::Unverified { reason }, members));
        }
        // Even an empty scan with a live guardian retains capacity until ECHILD proof arrives.
        Ok((OwnershipAssessment::Live, members))
    }
}

//! Mission Application capabilities: CRUD, storage, optimistic revision,
//! Round creation, plan approval, read projections, and reconciliation.
//!
//! See `docs/mission-spec.md` and `docs/mission-reconciliation.md`.

mod persistence;
pub mod reconciliation;
mod service;
mod workflow;

pub use service::{FileMissionService, MissionService};
pub use workflow::{MissionAuthoringRequest, MissionPlanApproval, MissionRoundAuthoring};

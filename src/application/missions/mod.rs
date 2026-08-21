//! Mission Application capabilities: CRUD, storage, optimistic revision,
//! Round creation, plan approval, read projections, agent phases, and
//! reconciliation.
//!
//! See `docs/mission-spec.md` and `docs/mission-reconciliation.md`.

pub mod agent_phase;
pub mod contracts;
mod persistence;
pub mod phases;
pub mod reconciliation;
pub mod runner;
mod service;
mod verification_sources;
mod workflow;

pub use runner::{MissionEvaluation, MissionWorkflowEngine};
pub use service::{FileMissionService, MissionService};
pub use workflow::{MissionAuthoringRequest, MissionPlanApproval, MissionRoundAuthoring};

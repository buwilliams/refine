//! State synchronization, conflict evidence, recovery, and resolution policy.

pub mod conflict_reports;
pub(crate) mod goal_ownership;
pub mod health;
pub mod recovery;
pub mod resolution;
pub mod state;
pub mod state_merge;

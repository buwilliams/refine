//! Machine-to-machine protocol constants shared by application adapters.

/// The API contract version this build speaks.
// 3: `/sync` family replaced `/project/sync` and `/project/state-recovery/*`.
pub const API_CONTRACT_VERSION: &str = "3";

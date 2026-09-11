//! Machine-to-machine protocol constants shared by application adapters.

/// The API contract version this build speaks.
// 3: `/sync` family replaced `/project/sync` and `/project/state-recovery/*`.
// 4: Events and Skills replace the separate policy configuration APIs.
// 5: Skills own one trigger; manual runs are independent of Goals.
// 6: Explicit workflow outcomes, four lifecycle edges, and Knowledge Hub surfaces.
pub const API_CONTRACT_VERSION: &str = "6";

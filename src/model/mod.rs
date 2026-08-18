pub mod feature;
pub mod fleet;
pub mod goal;
pub mod log;
pub mod node;
pub mod project;
pub mod workflow;

pub type Timestamp = String;
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

/// The API contract version this build speaks. It lives here, not in the
/// daemon, because it is the one version fact nodes exchange with each other:
/// a fleet upgrades node by node, so a node still on the previous build
/// answers a newer node's request by rejecting this version, and the fleet
/// reports that node as pending upgrade.
// 3: `/sync` family replaced `/project/sync` and `/project/state-recovery/*`.
pub const API_CONTRACT_VERSION: &str = "3";

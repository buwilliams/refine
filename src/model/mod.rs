//! Domain types, invariants, status policies, and pure derivations.
//!
//! Model code has no runtime, filesystem, process, or surface dependencies.

pub mod feature;
pub mod fleet;
pub mod goal;
pub mod log;
pub mod mission;
pub mod node;
pub mod project;
pub mod workflow;

pub type Timestamp = String;
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

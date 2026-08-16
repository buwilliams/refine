//! The shared boundary for structured agent output.
//!
//! Every place Refine parses JSON produced by a coding agent goes through this
//! module: one bounded extraction strategy ([`extract::decode_structured`] /
//! [`extract::select_value`]), one typed error taxonomy
//! ([`error::StructuredOutputError`]: transport vs schema vs validation), and
//! one repair policy ([`repair`]). Contracts shown to agents in prompts are
//! rendered from the same serde types the decoder targets, so the prompt and
//! the parser cannot drift apart.

pub mod contract;
pub mod error;
pub mod extract;
pub mod persisted;
pub mod repair;

#[cfg(test)]
pub use contract::assert_contract_roundtrip;
pub use contract::Contract;
pub use error::{StructuredOutputError, StructuredOutputErrorKind};
pub use extract::{DecodeOptions, Selection, decode_structured, json_candidates, select_value};
pub use persisted::decode_persisted;
pub use repair::{
    AttemptOutcome, DIAGNOSTIC_REPAIR_ATTEMPTS, MAX_INVALID_SIGNAL_REPLACEMENTS, RepairDirective,
    RepairPolicy, run_with_repair,
};

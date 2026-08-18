use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::extract::{DecodeOptions, decode_structured};
use crate::error::StructuredOutputError;

/// One agent-output contract: the serde type is the single source of truth.
///
/// The JSON example agents see in prompts is rendered from [`Contract::example`]
/// by [`Contract::contract_json`], and agent output is decoded back through the
/// same type by [`Contract::decode`], so the prompt and the parser cannot
/// drift apart. Semantic rules that need no external context belong in
/// [`Contract::validate`]; cross-artifact rules stay at the call site.
pub trait Contract: Serialize + DeserializeOwned {
    /// Human label used in every diagnostic, e.g. "implementation plan JSON".
    const LABEL: &'static str;

    /// Completion-envelope fields this contract may arrive wrapped under.
    const ENVELOPE_FIELDS: &'static [&'static str] = &[];

    /// Populated canonical example; every array field shows one element.
    fn example() -> Self;

    /// Pre-deserialization alias/shape normalization of the selected value.
    fn normalize(_value: &mut Value) -> Result<(), StructuredOutputError> {
        Ok(())
    }

    /// Context-free semantic validation of the decoded value.
    fn validate(&self) -> Result<(), StructuredOutputError> {
        Ok(())
    }

    /// The rendered contract JSON injected into prompts and completion
    /// contracts.
    fn contract_json() -> String {
        serde_json::to_string(&Self::example()).expect("contract example must serialize")
    }

    /// Decode one agent output through the shared transport boundary, then
    /// normalize, deserialize with path-aware diagnostics, and validate.
    fn decode(output: &str) -> Result<Self, StructuredOutputError> {
        let decoded: Self = decode_structured(
            output,
            &DecodeOptions::with_envelopes(Self::LABEL, Self::ENVELOPE_FIELDS),
            Self::normalize,
        )?;
        decoded.validate()?;
        Ok(decoded)
    }
}

/// Assert that a contract's rendered example decodes and validates through its
/// own decoder — the drift guard every `Contract` impl must pass.
#[cfg(test)]
pub fn assert_contract_roundtrip<T: Contract>() {
    T::decode(&T::contract_json()).unwrap_or_else(|error| {
        panic!(
            "rendered {} contract example must decode through its own decoder: {error}",
            T::LABEL
        )
    });
}

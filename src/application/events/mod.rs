//! Shared Events and Skills capabilities for workflow and every surface.
pub(crate) mod completion;
mod configuration;
mod context;
pub(crate) mod dispatch;
pub(crate) mod execution;
pub(crate) mod gate_configuration;
pub(crate) mod migration;
pub(crate) mod outcomes;
mod parameters;
mod records;
pub(crate) mod transitions;
pub mod workflow;

pub use configuration::FileEventService;
pub use execution::{EventInvocation, InvocationContext, InvocationState};

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod test_support;

mod waiting;

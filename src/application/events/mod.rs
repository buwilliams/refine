//! Shared Events and Skills capabilities for workflow and every surface.
mod configuration;
pub(crate) mod dispatch;
pub(crate) mod execution;
pub(crate) mod migration;
pub(crate) mod transitions;
pub mod workflow;

pub use configuration::FileEventService;
pub use execution::{EventInvocation, InvocationContext, InvocationState};

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod test_support;

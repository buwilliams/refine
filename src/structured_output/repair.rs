/// Maximum number of diagnostic follow-up invocations after an initial
/// structured response fails parsing or contract validation. Each follow-up is
/// a full provider invocation, so this is deliberately small; the diagnostic it
/// carries names the exact transport, schema-path, or validation fault.
pub const DIAGNOSTIC_REPAIR_ATTEMPTS: usize = 2;

use crate::core::supervisor::errors::{RefineError, RefineResult};

pub fn not_implemented_fixture(name: &str) -> RefineResult<()> {
    Err(RefineError::NotImplemented(format!(
        "test fixture {name} has not been implemented yet"
    )))
}

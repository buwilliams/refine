//! Shared Git primitives: the supervised command runner, the repository lock,
//! CAS ref publication, and ancestry classification. Every module that talks
//! to a Git repository goes through here so one lock discipline and one
//! command policy hold across state sync, worktrees, and integration.

pub mod ancestry;
pub mod locks;
pub mod merge;
pub mod refs;
pub mod repo;
pub mod state_driver;

pub use ancestry::{Ancestry, classify};
pub use locks::with_repository_git_lock;

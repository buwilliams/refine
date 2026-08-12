mod active_goals;
mod deep_search;
mod helpers;
mod query;
mod store;
#[cfg(test)]
mod tests;
mod types;

pub use active_goals::ActiveGoalIndex;
pub use deep_search::goal_text_matches;
pub use query::ProjectionQuery;
pub use store::FileProjectProjectionStore;
pub use types::*;

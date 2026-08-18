use super::*;

#[test]
fn terminal_mutations_do_not_refresh_projection_cache() {
    assert!(!should_refresh_projection_after_mutation(
        "/api/terminal/session"
    ));
    assert!(!should_refresh_projection_after_mutation(
        "/api/terminal/session-1/input"
    ));
    assert!(!should_refresh_projection_after_mutation(
        "/terminal/session-1/resize"
    ));
    assert!(!should_refresh_projection_after_mutation("/api/sync"));
    assert!(!should_refresh_projection_after_mutation("/api/goals"));
    assert!(should_refresh_projection_after_mutation(
        "/api/goals/GOAL1/start"
    ));
}

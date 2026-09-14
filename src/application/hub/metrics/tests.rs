use super::*;
fn at(s: &str) -> DateTime<Utc> {
    time(&json!(s)).unwrap()
}
#[test]
fn metrics_separate_creation_cohorts_delivery_evidence_and_current_work() {
    let now = at("2026-09-14T12:00:00Z");
    let records = vec![
        json!({"id":"a","status":"done","created":"2026-09-13T12:00:00Z","updated":"2026-09-14T10:00:00Z","node_id":"node-a","reporter":"Alice","rounds":[{"created":"2026-09-13T12:00:00Z"},{"created":"2026-09-14T09:00:00Z","workflow_integration":{"integrated_at":"2026-09-14T10:00:00Z"}}]}),
        json!({"id":"b","status":"done","created":"2026-09-01T00:00:00Z","updated":"2026-09-14T10:00:00Z","rounds":[{"created":"2026-09-01T00:00:00Z","workflow_integration":{"integrated_at":"2026-09-14T11:00:00Z"}}]}),
        json!({"id":"c","status":"done","created":"2026-09-14T01:00:00Z","updated":"2026-09-14T11:00:00Z","rounds":[{"created":"2026-09-14T01:00:00Z"}]}),
        json!({"id":"d","status":"plan","created":"2026-09-14T02:00:00Z","updated":"2026-09-14T11:00:00Z","rounds":[]}),
        json!({"id":"e","status":"failed","created":"2026-09-01T00:00:00Z","updated":"2026-09-02T00:00:00Z","rounds":[{"created":"bad-date"}]}),
        json!({"id":"f","status":"review","created":"bad-date","updated":"bad-date","rounds":[]}),
    ];
    let snapshot = summarize(&records, now);
    let day = &snapshot["periods"]["day"];
    assert_eq!(day["goals_created"], 3); // inclusive window start
    assert_eq!(day["goals_delivered"], 2); // one delivered Goal predates creation cohort
    assert_eq!(day["rounds_per_goal"], 1.0);
    assert_eq!(day["round_distribution"], json!([1, 1, 1, 0]));
    assert_eq!(day["rounds_created"], 3);
    assert_eq!(day["single_round_delivery_share"], 0.5);
    assert_eq!(day["delivery_time_samples"], 2);
    assert_eq!(snapshot["coverage"]["done_without_integration_time"], 1);
    assert_eq!(snapshot["coverage"]["goals_without_created_time"], 1);
    assert_eq!(snapshot["current"]["unfinished"], 3);
    assert_eq!(snapshot["current"]["quiet_seven_days"], 1);
    assert_eq!(snapshot["periods"]["day"]["previous"]["goals_created"], 0);
    for key in ["day", "week", "month", "quarter", "year"] {
        let p = &snapshot["periods"][key];
        for (bucket, total) in [
            ("created", "goals_created"),
            ("delivered", "goals_delivered"),
        ] {
            let sum: u64 = p["trend"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b[bucket].as_u64().unwrap())
                .sum();
            assert_eq!(sum, p[total].as_u64().unwrap());
        }
    }
    assert!(!snapshot.to_string().contains("\"id\":\"a\""));
}
#[test]
fn empty_windows_have_no_invented_averages_and_reopened_goals_are_not_delivered() {
    let now = at("2026-09-14T12:00:00Z");
    let empty = summarize(&[], now);
    assert!(empty["periods"]["day"]["rounds_per_goal"].is_null());
    assert!(empty["periods"]["year"]["median_delivery_hours"].is_null());
    let records = vec![
        json!({"status":"plan","created":"2026-09-14T02:00:00Z","updated":"2026-09-14T11:00:00Z","rounds":[{"created":"2026-09-14T02:00:00Z","workflow_integration":{"integrated_at":"2026-09-14T10:00:00Z"}}]}),
    ];
    assert_eq!(
        summarize(&records, now)["periods"]["day"]["goals_delivered"],
        0
    );
}
#[test]
fn default_hub_refresh_is_persisted_without_goal_content_or_agent_execution() {
    let root = std::env::temp_dir().join(format!("refine-metrics-{}", uuid::Uuid::new_v4()));
    let state = root.join(".refine");
    let hub = Hub::new(&state, root.join("runtime"));
    assert!(hub.metrics_snapshot().unwrap()["generated_at"].is_null());
    let service = crate::application::work_items::FileWorkItemService::new(&state);
    service
        .create_goal_summary("Private business request", Some("METRICS1"))
        .unwrap();
    let before = service.show_goal_detail("METRICS1").unwrap();
    let metrics = hub.refresh_metrics().unwrap();
    assert_eq!(metrics["current"]["goals"], 1);
    assert_eq!(hub.metrics_snapshot().unwrap(), metrics);
    assert!(
        !serde_json::to_string(&metrics)
            .unwrap()
            .contains("Private business request")
    );
    assert_eq!(service.show_goal_detail("METRICS1").unwrap(), before);
    assert!(hub.delete_site(ID, "anything").is_err());
    assert!(hub.asset(ID, "index.html", true).is_ok());
    assert_eq!(hub.for_skill(SKILL_ID).unwrap()[0]["id"], ID);
    let skills = crate::application::events::FileEventService::with_runtime_root(
        &state,
        root.join("runtime"),
    );
    let config = skills.config().unwrap();
    assert!(
        skills
            .manual_skill_event(&config, SKILL_ID, "default")
            .is_ok()
    );
    assert!(skills.remove("skills", SKILL_ID, config.revision).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

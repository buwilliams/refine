use super::*;

#[test]
fn hub_record_validation_revision_and_query_cache_agree() {
    let temp = std::env::temp_dir().join(format!("refine-hub-{}", uuid::Uuid::new_v4()));
    let hub = Hub::new(temp.join("state"), temp.join("runtime"));
    hub.save_site("reports", &json!({"name":"Reports"}))
        .unwrap();
    hub.save_collection(
        "reports",
        "events",
        &json!({"indexes":{"fields":{"value":"number"},"search":["message"]}}),
    )
    .unwrap();
    assert!(
        hub.put(
            "reports",
            "events",
            "bad",
            &json!({"data":{"value":"wrong"}})
        )
        .is_err()
    );
    assert!(hub.get("reports", "events", "bad").is_err());
    let first = hub
        .put(
            "reports",
            "events",
            "one",
            &json!({"data":{"value":3,"message":"Useful history"},"request_id":"create-one"}),
        )
        .unwrap();
    let query = serde_json::from_value(
        json!({"search":"history", "filters":[{"field":"value","op":"gte","value":2}]}),
    )
    .unwrap();
    assert_eq!(hub.query("reports", "events", &query).unwrap()["total"], 1);
    assert!(
        hub.put(
            "reports",
            "events",
            "one",
            &json!({"revision":"stale","data":{"value":4}})
        )
        .is_err()
    );
    let updated = hub
        .put(
            "reports",
            "events",
            "one",
            &json!({"revision":first["revision"],"data":{"value":1,"message":"Useful history"}}),
        )
        .unwrap();
    assert_eq!(hub.query("reports", "events", &query).unwrap()["total"], 0);
    hub.delete(
        "reports",
        "events",
        "one",
        updated["revision"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(
        hub.query("reports", "events", &Default::default()).unwrap()["total"],
        0
    );
    query::invalidate(&hub.root);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
#[ignore = "explicit 100k-record Hub performance benchmark"]
fn hub_100k_warm_indexed_reads_stay_within_fifty_milliseconds() {
    let temp = std::env::temp_dir().join(format!("refine-hub-benchmark-{}", uuid::Uuid::new_v4()));
    let hub = Hub::new(temp.join("state"), temp.join("runtime"));
    hub.save_site("reports", &json!({"name":"Reports"}))
        .unwrap();
    hub.save_collection(
        "reports",
        "events",
        &json!({"indexes":{"fields":{"value":"number","group":"string"}}}),
    )
    .unwrap();
    let records = hub.collection("reports", "events").unwrap().join("records");
    for shard in 0..256 {
        fs::create_dir_all(records.join(format!("{shard:02x}"))).unwrap();
    }
    for number in 0..100_000 {
        let key = format!("event-{number:06}");
        let value = json!({"id":key,"data":{"value":number,"group":"events"},"updated":"2026-09-11T00:00:00Z"});
        // Bulk fixture construction bypasses mutation fsync; production writes do not.
        fs::write(
            records
                .join(&digest(key.as_bytes())[..2])
                .join(format!("{key}.json")),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    let rebuild = hub.rebuild_index("reports", "events").unwrap();
    assert_eq!(rebuild["records"], 100_000);
    let query = serde_json::from_value(
        json!({"filters":[{"field":"value","op":"gte","value":99900}],"sort":"value","limit":25}),
    )
    .unwrap();
    hub.query("reports", "events", &query).unwrap();
    let mut milliseconds = Vec::new();
    for _ in 0..20 {
        let started = std::time::Instant::now();
        let result = hub.query("reports", "events", &query).unwrap();
        milliseconds.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(result["total"], 100);
    }
    milliseconds.sort_by(f64::total_cmp);
    eprintln!(
        "Hub 100k rebuild: {rebuild}; warm indexed read p95: {} ms",
        milliseconds[18]
    );
    query::invalidate(&hub.root);
    fs::remove_dir_all(temp).unwrap();
    assert!(
        milliseconds[18] < 50.0,
        "warm indexed p95 exceeded 50 ms: {}",
        milliseconds[18]
    );
}

#[test]
fn hub_publication_is_explicit_and_pins_assets() {
    let temp = std::env::temp_dir().join(format!("refine-hub-{}", uuid::Uuid::new_v4()));
    let hub = Hub::new(temp.join("state"), temp.join("runtime"));
    let site = hub
        .save_site("reports", &json!({"name":"Reports"}))
        .unwrap();
    let draft = hub
        .save_asset("reports", "index.html", b"first", None, false)
        .unwrap();
    assert!(hub.asset("reports", "index.html", true).is_err());
    hub.publish("reports", site["revision"].as_str().unwrap(), &[], true)
        .unwrap();
    hub.save_asset(
        "reports",
        "index.html",
        b"second",
        draft["revision"].as_str(),
        false,
    )
    .unwrap();
    assert_eq!(
        hub.asset("reports", "index.html", true).unwrap().0,
        b"first"
    );
    assert_eq!(
        hub.asset("reports", "index.html", false).unwrap().0,
        b"second"
    );
    assert!(
        hub.save_asset("reports", "../escape", b"x", None, false)
            .is_err()
    );
    assert!(
        hub.public_query("reports", "private", &Default::default())
            .is_err()
    );
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn hub_timestamp_indexes_compare_instants_across_offsets() {
    let temp = std::env::temp_dir().join(format!("refine-hub-time-{}", uuid::Uuid::new_v4()));
    let hub = Hub::new(temp.join("state"), temp.join("runtime"));
    hub.save_site("reports", &json!({"name":"Reports"}))
        .unwrap();
    hub.save_collection(
        "reports",
        "events",
        &json!({"indexes":{"fields":{"at":"timestamp"}}}),
    )
    .unwrap();
    hub.put(
        "reports",
        "events",
        "first",
        &json!({"data":{"at":"2026-09-11T12:00:00+02:00"}}),
    )
    .unwrap();
    hub.put(
        "reports",
        "events",
        "second",
        &json!({"data":{"at":"2026-09-11T10:30:00Z"}}),
    )
    .unwrap();
    let q = serde_json::from_value(
        json!({"sort":"at", "filters":[{"field":"at","op":"eq","value":"2026-09-11T10:00:00Z"}]}),
    )
    .unwrap();
    let result = hub.query("reports", "events", &q).unwrap();
    assert_eq!(result["total"], 1);
    assert_eq!(result["rows"][0]["item"]["id"], "first");
    let q = serde_json::from_value(json!({"sort":"at"})).unwrap();
    assert_eq!(
        hub.query("reports", "events", &q).unwrap()["rows"][0]["item"]["id"],
        "first"
    );
    query::invalidate(&hub.root);
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn hub_ordered_indexes_aggregate_page_and_invalidate_after_updates() {
    let temp = std::env::temp_dir().join(format!("refine-hub-query-{}", uuid::Uuid::new_v4()));
    let hub = Hub::new(temp.join("state"), temp.join("runtime"));
    hub.save_site("reports", &json!({"name":"Reports"}))
        .unwrap();
    hub.save_collection("reports", "events", &json!({"indexes":{"fields":{"value":"number","team":"string","at":"timestamp"},"search":["message"]}})).unwrap();
    for (id, value, team) in [("one", -2, "a"), ("two", 0, "a"), ("three", 5, "b")] {
        hub.put("reports", "events", id, &json!({"data":{"value":value,"team":team,"at":"2026-09-11T00:10:00Z","message":"significant event"}})).unwrap();
    }
    let query = |value| serde_json::from_value(value).unwrap();
    let first = hub.query("reports", "events", &query(json!({"filters":[{"field":"value","op":"gte","value":-2}],"sort":"value","limit":1}))).unwrap();
    assert_eq!(first["rows"][0]["item"]["id"], "one");
    let mut next = query(
        json!({"filters":[{"field":"value","op":"gte","value":-2}],"sort":"value","limit":1,"cursor":first["next_cursor"]}),
    );
    assert_eq!(
        hub.query("reports", "events", &next).unwrap()["rows"][0]["item"]["id"],
        "two"
    );
    let aggregate = hub.query("reports", "events", &query(json!({"search":"significant","group_by":["team"],"aggregates":{"total":{"op":"sum","field":"value"},"events":{"op":"count"}},"sort":"total","descending":true}))).unwrap();
    assert_eq!(
        aggregate["rows"][0],
        json!({"team":"b","total":5.0,"events":1})
    );
    let buckets = hub.query("reports", "events", &query(json!({"time_bucket":{"field":"at","seconds":3600},"aggregates":{"events":{"op":"count"}},"sort":"bucket"}))).unwrap();
    assert_eq!(buckets["rows"][0]["events"], 3);
    let record = hub.get("reports", "events", "two").unwrap();
    hub.put("reports", "events", "two", &json!({"revision":record["revision"],"data":{"value":-5,"team":"a","at":"2026-09-11T00:10:00Z"}})).unwrap();
    assert!(matches!(
        hub.query("reports", "events", &next),
        Err(RefineError::Conflict(_))
    ));
    next.cursor = None;
    assert_eq!(hub.query("reports", "events", &next).unwrap()["total"], 2);
    assert!(
        hub.save_collection(
            "reports",
            "invalid",
            &json!({"indexes":{"fields":{"id":"number"}}})
        )
        .is_err()
    );
    query::invalidate(&hub.root);
    fs::remove_dir_all(temp).unwrap();
}

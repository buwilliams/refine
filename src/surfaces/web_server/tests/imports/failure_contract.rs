use super::*;

#[test]
fn import_failure_indexes_are_one_based_and_select_the_same_browser_draft() {
    for failed_zero_based in 0..3 {
        let temp_root = unique_temp_dir(&format!("http-import-index-{failed_zero_based}"));
        let mut server = server_with_projection();
        server.target_root = Some(temp_root.clone());
        let drafts = (0..3)
            .map(|index| {
                json!({
                    "name": format!("Draft {}", index + 1),
                    "prompt": format!("Prompt {}", index + 1),
                    "reporter": if index == failed_zero_based {
                        "invalid\nreporter"
                    } else {
                        "QA"
                    },
                    "priority": "medium"
                })
            })
            .collect::<Vec<_>>();

        let response = server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/import/persist".to_string(),
            body: Some(json!({"drafts": drafts})),
        });

        assert_eq!(response.status, 207, "{}", response.body);
        let api_index = response.body["failures"][0]["index"].as_u64().unwrap() as usize;
        assert_eq!(api_index, failed_zero_based + 1);
        assert_ne!(
            api_index, 0,
            "first-item fallback must never mask index zero"
        );
        assert_eq!(
            response.body["failures"][0]["name"],
            drafts[api_index - 1]["name"],
            "the one-based API index must select the same draft as the browser retry UI"
        );

        remove_temp_dir(&temp_root);
    }
}

#[cfg(unix)]
#[test]
fn synchronous_import_surfaces_injected_feature_rollback_failure() {
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir("http-import-sync-partial-failure");
    let refine_dir = temp_root.join(".refine");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    let drafts = (0..200)
        .map(|index| {
            json!({
                "name": format!("Synchronous draft {index:03}"),
                "prompt": format!("Synchronous prompt {index:03}"),
                "reporter": if index == 199 { "invalid\nreporter" } else { "QA" },
                "priority": "medium"
            })
        })
        .collect::<Vec<_>>();
    let worker = thread::spawn(move || {
        server.handle(ApiRequest {
            method: "POST".to_string(),
            path: "/api/import/persist".to_string(),
            body: Some(json!({
                "new_feature_name": "Synchronous unrecovered Feature",
                "drafts": drafts
            })),
        })
    });

    let feature_path = wait_for_import_record(&refine_dir.join("features"), "feature.json");
    let locked_directory = feature_path.parent().unwrap().to_path_buf();
    fs::set_permissions(&locked_directory, fs::Permissions::from_mode(0o555)).unwrap();
    let response = worker.join().unwrap();
    fs::set_permissions(&locked_directory, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(response.status, 500, "{}", response.body);
    assert_eq!(response.body["code"], "import_rollback_incomplete");
    assert_eq!(response.body["kind"], "partial_failure");
    assert_eq!(response.body["cancelled"], false);
    assert_eq!(response.body["failures"][0]["index"], 200);
    assert_eq!(
        response.body["created_goal_ids"].as_array().unwrap().len(),
        200
    );
    assert_eq!(
        response.body["rolled_back_goal_ids"]
            .as_array()
            .unwrap()
            .len(),
        200
    );
    assert!(
        response.body["unrecovered_goal_ids"]
            .as_array()
            .unwrap()
            .is_empty(),
        "Goal cleanup completed even though Feature cleanup failed"
    );
    assert_eq!(
        response.body["unrecovered_feature_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        response.body["rollback_failures"][0]
            .as_str()
            .unwrap()
            .contains("Feature")
    );
    assert!(
        response.body["recovery_guidance"]
            .as_str()
            .unwrap()
            .contains("unrecovered")
    );

    remove_temp_dir(&temp_root);
}

#[cfg(unix)]
#[test]
fn background_import_rollback_failure_is_durably_failed_after_cancellation() {
    use std::os::unix::fs::PermissionsExt;

    let temp_root = unique_temp_dir("http-import-background-partial-failure");
    let refine_dir = temp_root.join(".refine");
    let runtime_root = temp_root.join("run/8080");
    let mut server = server_with_projection();
    server.target_root = Some(temp_root.clone());
    server.runtime_root = Some(runtime_root.clone());
    let drafts = (0..240)
        .map(|index| {
            json!({
                "name": format!("Background partial draft {index:03}"),
                "prompt": format!("Background partial prompt {index:03}"),
                "reporter": "QA",
                "priority": "medium"
            })
        })
        .collect::<Vec<_>>();
    let started = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: "/api/import/persist".to_string(),
        body: Some(json!({
            "background": true,
            "new_feature_name": "Background unrecovered Feature",
            "drafts": drafts
        })),
    });
    assert_eq!(started.status, 202, "{}", started.body);
    let operation_id = started.body["operation"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let registry = FileOperationRegistry::new(&runtime_root);
    let progress_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let operation = registry.status(&operation_id).unwrap();
        if operation.progress["completed"].as_u64().unwrap_or(0) >= 1 {
            break;
        }
        assert!(
            Instant::now() < progress_deadline,
            "timed out waiting for the import to create rollback candidates: {operation:?}"
        );
        thread::sleep(Duration::from_millis(5));
    }

    let feature_path = wait_for_import_record(&refine_dir.join("features"), "feature.json");
    let locked_directory = feature_path.parent().unwrap().to_path_buf();
    fs::set_permissions(&locked_directory, fs::Permissions::from_mode(0o555)).unwrap();
    let cancel = server.handle(ApiRequest {
        method: "POST".to_string(),
        path: format!("/api/operations/{operation_id}/cancel"),
        body: None,
    });
    assert_eq!(cancel.status, 200, "{}", cancel.body);
    assert_eq!(cancel.body["operation"]["status"], "cancelling");

    let operation = wait_for_operation_status(&registry, &operation_id, OperationState::Failed);
    fs::set_permissions(&locked_directory, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        operation.error.as_ref().unwrap()["code"],
        "import_rollback_incomplete"
    );
    assert_eq!(operation.error.as_ref().unwrap()["kind"], "partial_failure");
    assert_eq!(operation.result["cancelled"], false);
    assert_ne!(operation.result["cancelled"], true);
    assert_eq!(
        operation.result["unrecovered_feature_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        operation.result["rollback_failures"][0]
            .as_str()
            .unwrap()
            .contains("Feature")
    );
    assert!(
        operation.error.as_ref().unwrap()["details"]["recovery_guidance"]
            .as_str()
            .unwrap()
            .contains("unrecovered")
    );
    let api_readback = server.handle(ApiRequest {
        method: "GET".to_string(),
        path: format!("/api/operations/{operation_id}"),
        body: None,
    });
    assert_eq!(api_readback.status, 200, "{}", api_readback.body);
    assert_eq!(api_readback.body["operation"]["status"], "failed");
    assert_eq!(
        api_readback.body["operation"]["error"]["code"],
        "import_rollback_incomplete"
    );
    assert_eq!(
        api_readback.body["operation"]["result"]["rollback_failures"],
        operation.result["rollback_failures"]
    );

    let restarted_registry = FileOperationRegistry::new(&runtime_root);
    let durable = restarted_registry.status(&operation_id).unwrap();
    assert_eq!(durable.state, OperationState::Failed);
    assert_eq!(durable.error, operation.error);
    assert_eq!(durable.result, operation.result);
    assert!(
        restarted_registry
            .recover()
            .unwrap()
            .into_iter()
            .any(|candidate| {
                candidate.id == operation_id
                    && candidate.state == OperationState::Failed
                    && candidate.error.as_ref().unwrap()["code"] == "import_rollback_incomplete"
            })
    );
    let unrecovered_feature_id = durable.result["unrecovered_feature_ids"][0]
        .as_str()
        .unwrap();
    assert!(
        FileWorkItemService::new(&refine_dir)
            .show_feature_summary(unrecovered_feature_id)
            .is_ok(),
        "the durable evidence must identify the imported record that remains"
    );

    remove_temp_dir(&temp_root);
}

fn wait_for_import_record(root: &Path, filename: &str) -> PathBuf {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(prefixes) = fs::read_dir(root) {
            for prefix in prefixes.flatten() {
                let Ok(records) = fs::read_dir(prefix.path()) else {
                    continue;
                };
                for record in records.flatten() {
                    let candidate = record.path().join(filename);
                    if candidate.is_file() {
                        return candidate;
                    }
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {filename} under {}",
            root.display()
        );
        thread::sleep(Duration::from_millis(2));
    }
}

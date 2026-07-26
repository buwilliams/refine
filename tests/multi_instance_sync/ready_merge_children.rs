use super::*;

#[test]
#[ignore = "production Ready Merge child process used by the multi-instance gate"]
fn ready_merge_child_process() {
    if std::env::var("REFINE_READY_MERGE_CHILD").ok().as_deref() != Some("1") {
        return;
    }
    let runtime_root = PathBuf::from(std::env::var("REFINE_CHILD_RUNTIME").unwrap());
    let refine_dir = PathBuf::from(std::env::var("REFINE_CHILD_STATE").unwrap());
    let repo = PathBuf::from(std::env::var("REFINE_CHILD_REPO").unwrap());
    let goal_id = std::env::var("REFINE_CHILD_GOAL").unwrap();
    let claim_id = std::env::var("REFINE_CHILD_CLAIM").unwrap();
    let execution_id = std::env::var("REFINE_CHILD_EXECUTION").unwrap();
    let branch = std::env::var("REFINE_CHILD_BRANCH").unwrap();
    let candidate = std::env::var("REFINE_CHILD_CANDIDATE").unwrap();
    let output_path = PathBuf::from(std::env::var("REFINE_CHILD_OUTPUT").unwrap());
    let outcome = FileMergerService::with_target_root(&runtime_root, &refine_dir, &repo)
        .integrate_workflow_candidate(
            &goal_id,
            0,
            &claim_id,
            &execution_id,
            "default",
            &branch,
            &candidate,
            "origin",
        );
    let value = match outcome {
        Ok(integration) => json!({"ok": true, "integration": integration}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    };
    fs::write(output_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

#[test]
#[ignore = "production operation-settlement child used by the multi-instance gate"]
fn ready_merge_settlement_child_process() {
    if std::env::var("REFINE_SETTLEMENT_CHILD").ok().as_deref() != Some("1") {
        return;
    }
    let runtime_root = PathBuf::from(std::env::var("REFINE_CHILD_RUNTIME").unwrap());
    let operation_id = std::env::var("REFINE_CHILD_OPERATION").unwrap();
    let ready = PathBuf::from(std::env::var("REFINE_CHILD_READY").unwrap());
    let proceed = PathBuf::from(std::env::var("REFINE_CHILD_PROCEED").unwrap());
    let transitioned = PathBuf::from(std::env::var("REFINE_CHILD_TRANSITIONED").unwrap());
    let output = PathBuf::from(std::env::var("REFINE_CHILD_OUTPUT").unwrap());
    fs::write(&ready, b"ready").unwrap();
    wait_for_path(&proceed, "settlement continuation");
    let result = FileOperationRegistry::new(runtime_root).succeed_after(
        &operation_id,
        json!({"stage": "settled"}),
        json!({"integration": "candidate"}),
        || {
            fs::write(&transitioned, b"transitioned").unwrap();
            Ok(())
        },
    );
    let value = match result {
        Ok(_) => json!({"ok": true}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    };
    fs::write(output, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

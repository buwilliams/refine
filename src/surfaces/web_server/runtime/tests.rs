use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

#[test]
fn runtime_fingerprint_summarizes_process_dir_and_skips_atomic_temp_files() {
    let temp_root = unique_temp_dir("runtime-fingerprint-temp");
    let runtime_root = temp_root.join("run/8080");
    let processes = runtime_root.join("processes");
    fs::create_dir_all(&processes).unwrap();
    fs::write(processes.join("proc-live.json"), "{}\n").unwrap();
    fs::write(
        processes.join(".proc-live.json.proc-temp.tmp"),
        "{\"partial\":",
    )
    .unwrap();
    let metrics_log = runtime_root.join("metrics/performance.jsonl");
    fs::create_dir_all(metrics_log.parent().unwrap()).unwrap();
    fs::write(&metrics_log, "{\"duration_ms\":1}\n").unwrap();

    let fingerprint_before_metric_append =
        runtime_projection_fingerprint(&runtime_root, None).unwrap();
    fs::write(&metrics_log, "{\"duration_ms\":1}\n{\"duration_ms\":2}\n").unwrap();
    let fingerprint = runtime_projection_fingerprint(&runtime_root, None).unwrap();

    assert_eq!(fingerprint, fingerprint_before_metric_append);
    assert!(fingerprint.entries.contains_key("processes"));
    assert!(!fingerprint.entries.contains_key("metrics"));
    assert!(!fingerprint.entries.contains_key("processes/proc-live.json"));
    assert!(
        !fingerprint
            .entries
            .contains_key("processes/.proc-live.json.proc-temp.tmp")
    );

    fs::remove_dir_all(temp_root).unwrap();
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("refine-{prefix}-{}-{nanos}", std::process::id()))
}

//! Exercise the HTTP retention consumer, including changes after enumeration.
use super::*;
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, owned_groups::test_fixture::UnobservedChild,
};
use std::fs;
use std::time::{Instant, SystemTime};

fn artifacts(owner: &FileProcessSupervisor, id: &str, aged: bool) -> Vec<std::path::PathBuf> {
    fs::create_dir_all(owner.processes_dir()).unwrap();
    ["stdout.log", "stderr.log", "stdin.txt"]
        .into_iter()
        .map(|suffix| {
            let p = owner.processes_dir().join(format!("{id}.{suffix}"));
            fs::write(&p, format!("retained {suffix}")).unwrap();
            if aged {
                fs::File::options()
                    .write(true)
                    .open(&p)
                    .unwrap()
                    .set_times(
                        std::fs::FileTimes::new()
                            .set_modified(SystemTime::now() - 8 * RETENTION_DAY),
                    )
                    .unwrap();
            }
            p
        })
        .collect()
}
fn retained(paths: &[std::path::PathBuf]) {
    for p in paths {
        assert!(fs::read_to_string(p).unwrap().starts_with("retained "));
    }
}

#[test]
fn retention_preserves_all_streams_without_trustworthy_exit_and_releases_after_proof() {
    for state in [
        "live",
        "unverified",
        "missing",
        "corrupt",
        "unreadable",
        "exited",
    ] {
        let root =
            std::env::temp_dir().join(format!("refine-retention-{state}-{}", uuid::Uuid::new_v4()));
        let owner = FileProcessSupervisor::new(&root);
        let fixture = UnobservedChild::launch(
            &owner,
            state != "unverified",
            serde_json::json!({"workflow_incarnation":"retention"}),
        );
        let id = &fixture.group.process.id;
        let record = root.join("owned-groups").join(format!("{id}.json"));
        let original = fs::read(&record).unwrap();
        let paths = artifacts(&owner, id, true);
        let primary = owner.processes_dir().join(format!("{id}.json"));
        let _ = fs::remove_file(&primary);
        match state {
            "missing" => fs::remove_file(&record).unwrap(),
            "corrupt" => fs::write(&record, "{bad").unwrap(),
            "unreadable" => {
                fs::remove_file(&record).unwrap();
                fs::create_dir(&record).unwrap();
            }
            "exited" => {
                owner
                    .stop_owned_group(&fixture.group, Duration::from_secs(2))
                    .unwrap();
            }
            _ => {}
        }
        sweep_orphan_process_logs(&owner.processes_dir(), 7 * RETENTION_DAY);
        if state == "exited" {
            assert!(paths.iter().all(|p| !p.exists()));
        } else {
            retained(&paths);
            assert!(fixture.child_alive());
        }
        if state == "unreadable" {
            fs::remove_dir(&record).unwrap();
        }
        if ["missing", "corrupt", "unreadable"].contains(&state) {
            fs::write(&record, original).unwrap();
        }
        if state != "unverified" {
            owner
                .stop_owned_group(&fixture.group, Duration::from_secs(2))
                .unwrap();
            sweep_orphan_process_logs(&owner.processes_dir(), 7 * RETENTION_DAY);
            assert!(paths.iter().all(|p| !p.exists()));
        } else {
            fixture.kill_child();
        }
        drop(fixture);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn retention_rechecks_handoff_registration_and_age_and_continues_other_artifacts() {
    let root = std::env::temp_dir().join(format!("refine-retention-race-{}", uuid::Uuid::new_v4()));
    let owner = FileProcessSupervisor::new(&root);
    let first = UnobservedChild::launch(
        &owner,
        true,
        serde_json::json!({"workflow_incarnation":"first"}),
    );
    let second = UnobservedChild::launch(
        &owner,
        true,
        serde_json::json!({"workflow_incarnation":"second"}),
    );
    for fixture in [&first, &second] {
        owner
            .stop_owned_group(&fixture.group, Duration::from_secs(2))
            .unwrap();
        let _ = fs::remove_file(
            owner
                .processes_dir()
                .join(format!("{}.json", fixture.group.process.id)),
        );
    }
    let aged = artifacts(&owner, &first.group.process.id, true);
    let other = artifacts(&owner, &second.group.process.id, true);
    let lease = std::sync::Arc::new(std::sync::Mutex::new(None));
    let acquired = lease.clone();
    let handoff_owner = owner.clone();
    let id = first.group.process.id.clone();
    crate::infrastructure::process::subprocess::install_after_process_enumeration_hook(
        &root,
        move || {
            *acquired.lock().unwrap() = Some(handoff_owner.begin_artifact_handoff(&id).unwrap());
        },
    );
    let start = Instant::now();
    sweep_orphan_process_logs(&owner.processes_dir(), 7 * RETENTION_DAY);
    assert!(start.elapsed() < Duration::from_secs(1));
    retained(&aged);
    assert!(other.iter().all(|p| !p.exists()));
    owner
        .finish_artifact_handoff(lease.lock().unwrap().take().unwrap())
        .unwrap();
    let fresh = artifacts(&owner, &second.group.process.id, false);
    // Discovery is followed by acquiring a handoff/registration fence in the owner.
    let process = first.group.process.clone();
    let registration_owner = owner.clone();
    let registration = process.clone();
    crate::infrastructure::process::subprocess::install_after_process_enumeration_hook(
        &root,
        move || {
            fs::write(
                registration_owner
                    .processes_dir()
                    .join(format!("{}.json", registration.id)),
                serde_json::to_vec(&registration).unwrap(),
            )
            .unwrap();
        },
    );
    sweep_orphan_process_logs(&owner.processes_dir(), 7 * RETENTION_DAY);
    retained(&aged);
    retained(&fresh);
    fs::remove_file(owner.processes_dir().join(format!("{}.json", process.id))).unwrap();
    sweep_orphan_process_logs(&owner.processes_dir(), 7 * RETENTION_DAY);
    assert!(aged.iter().all(|p| !p.exists()));
    retained(&fresh);
    drop(first);
    drop(second);
    fs::remove_dir_all(root).unwrap();
}

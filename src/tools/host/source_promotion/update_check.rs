use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde_json::json;

use crate::process::subprocess::{ManagedProcessSpec, ProcessResourceLimits, ProcessSupervisor};

use super::*;

const UPDATE_CHECK_LOCK_FILE: &str = ".source-update-check.lock";
const UPDATE_CHECK_OPERATION_OWNER: &str = "maintenance:source-update-check";

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateCheckLaunchFailpoint {
    None,
    AfterReservation,
    AfterLaunchBeforeProjectionReceipt,
}

trait UpdateCheckFailpoint: Copy {
    fn after_reservation(self) -> bool;
    fn after_launch(self) -> bool;
}

#[cfg(test)]
impl UpdateCheckFailpoint for UpdateCheckLaunchFailpoint {
    fn after_reservation(self) -> bool {
        self == Self::AfterReservation
    }

    fn after_launch(self) -> bool {
        self == Self::AfterLaunchBeforeProjectionReceipt
    }
}

#[cfg(not(test))]
impl UpdateCheckFailpoint for () {
    fn after_reservation(self) -> bool {
        false
    }

    fn after_launch(self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceUpdateCheckState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_successful_check_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_source_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_source_identity: Option<String>,
    #[serde(default)]
    pub freshness: String,
    #[serde(default)]
    pub in_flight: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default)]
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    /// The last complete Git inspection. HTTP and SSE readers use only this
    /// durable value; only the supervised worker is allowed to refresh it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourcePromotionSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CachedSourcePromotionStatus {
    pub source: SourcePromotionSnapshot,
    pub check: SourceUpdateCheckState,
}

impl FileSourcePromotionService {
    pub fn update_check_state_path(&self) -> PathBuf {
        self.port_runtime_root.join(SOURCE_UPDATE_CHECK_STATE_FILE)
    }

    /// Read the combined source projection without running Git or waiting for
    /// background work. A missing first snapshot is represented explicitly and
    /// becomes authoritative as soon as the queued worker publishes it.
    pub fn inspect_cached(&self) -> RefineResult<CachedSourcePromotionStatus> {
        let mut state = self.reconcile_update_check()?;
        state.freshness = freshness(&state, Utc::now());
        let mut source = state
            .source
            .clone()
            .unwrap_or_else(|| self.empty_cached_snapshot());
        source.operation = self.reconcile_interrupted_agent()?;
        Ok(CachedSourcePromotionStatus {
            source,
            check: state,
        })
    }

    /// Queue one durable, coalesced remote inspection and return immediately.
    /// Both automatic stale checks and explicit refreshes share the same
    /// supervised maintenance process.
    pub fn request_update_check(
        &self,
        force_refresh: bool,
    ) -> RefineResult<CachedSourcePromotionStatus> {
        let executable = self.checkout_path.join("bin/refine");
        require_checkout_binary(&executable, "source update-check worker")?;
        self.request_update_check_with(force_refresh, &executable)
    }

    pub(crate) fn request_update_check_with(
        &self,
        force_refresh: bool,
        executable: &Path,
    ) -> RefineResult<CachedSourcePromotionStatus> {
        #[cfg(test)]
        return self.request_update_check_with_failpoint(
            force_refresh,
            executable,
            UpdateCheckLaunchFailpoint::None,
        );
        #[cfg(not(test))]
        self.request_update_check_inner(force_refresh, executable)
    }

    #[cfg(test)]
    pub(crate) fn request_update_check_with_failpoint(
        &self,
        force_refresh: bool,
        executable: &Path,
        failpoint: UpdateCheckLaunchFailpoint,
    ) -> RefineResult<CachedSourcePromotionStatus> {
        self.request_update_check_inner_with_failpoint(force_refresh, executable, failpoint)
    }

    #[cfg(not(test))]
    fn request_update_check_inner(
        &self,
        force_refresh: bool,
        executable: &Path,
    ) -> RefineResult<CachedSourcePromotionStatus> {
        self.request_update_check_inner_with_failpoint(force_refresh, executable, ())
    }

    fn request_update_check_inner_with_failpoint<F: UpdateCheckFailpoint>(
        &self,
        force_refresh: bool,
        executable: &Path,
        failpoint: F,
    ) -> RefineResult<CachedSourcePromotionStatus> {
        let _guard = self.acquire_update_check_lock()?;
        let mut state = self.reconcile_update_check_unlocked()?;
        state.freshness = freshness(&state, Utc::now());
        if state.in_flight || (!force_refresh && !automatic_check_due(&state, Utc::now())) {
            return self.cached_status_from_state(state);
        }

        let (registry_operation, created) = self.register_exclusive_operation(
            UPDATE_CHECK_OPERATION_OWNER,
            json!({
                "kind": "source_update_check",
                "checkout": self.checkout_path,
                "restart_recovery": "capability"
            }),
        )?;
        let operation_id = registry_operation.id;
        state.in_flight = true;
        state.last_check_at = Some(now_timestamp());
        state.failure = None;
        state.operation_id = Some(operation_id.clone());
        state.process_id = None;
        self.save_update_check_state(&state)?;

        if failpoint.after_reservation() {
            return Err(RefineError::Degraded(
                "injected daemon termination after source-check reservation".to_string(),
            ));
        }

        if !created {
            return self.cached_status_from_state(self.reconcile_update_check_unlocked()?);
        }

        let mut metadata = serde_json::Map::new();
        metadata.insert("kind".to_string(), json!("source_update_check"));
        metadata.insert("source_check_operation".to_string(), json!(&operation_id));
        metadata.insert("operation_id".to_string(), json!(&operation_id));
        let supervisor = FileProcessSupervisor::new(&self.port_runtime_root);
        let launch = supervisor.launch(ManagedProcessSpec {
            owner: ProcessOwner::Maintenance,
            command: executable.display().to_string(),
            args: vec![
                "system".to_string(),
                "source-check-worker".to_string(),
                "--checkout".to_string(),
                self.checkout_path.display().to_string(),
                "--port-runtime-root".to_string(),
                self.port_runtime_root.display().to_string(),
                "--port".to_string(),
                self.port.to_string(),
                "--operation-id".to_string(),
                operation_id.clone(),
            ],
            cwd: Some(self.checkout_path.display().to_string()),
            env: Vec::new(),
            stdin: None,
            limits: Some(ProcessResourceLimits {
                kill_on_parent_exit: false,
                stall_timeout_seconds: Some(300),
                ..Default::default()
            }),
            authorization_command: Some(executable.display().to_string()),
            sensitive: false,
            metadata,
        });
        match launch {
            Ok(process) => {
                if failpoint.after_launch() {
                    return Err(RefineError::Degraded(
                        "injected daemon termination after source-check launch".to_string(),
                    ));
                }
                let mut latest = self.read_update_check_state()?;
                if latest.in_flight && latest.operation_id.as_deref() == Some(operation_id.as_str())
                {
                    latest.process_id = Some(process.id);
                    self.save_update_check_state(&latest)?;
                    state = latest;
                } else {
                    state = latest;
                }
            }
            Err(error) => {
                let _ = self.operation_registry().fail_with_error(
                    &operation_id,
                    json!({
                        "code": "source_update_check_launch_failed",
                        "message": error.to_string(),
                        "retryable": true
                    }),
                );
                state.in_flight = false;
                state.failure = Some(format!(
                    "Could not queue the supervised remote update check: {error}. Verify process authorization and retry"
                ));
                state.generation = state.generation.saturating_add(1);
                self.save_update_check_state(&state)?;
                return Err(error);
            }
        }
        self.cached_status_from_state(state)
    }

    /// Entry point used only by the supervised maintenance worker.
    pub fn run_update_check(&self, operation_id: &str) -> RefineResult<()> {
        {
            let _guard = self.acquire_update_check_lock()?;
            let state = self.read_update_check_state()?;
            if !state.in_flight || state.operation_id.as_deref() != Some(operation_id) {
                return Err(RefineError::Conflict(format!(
                    "source update check {operation_id} is no longer current"
                )));
            }
        }

        let result = self.inspect(true);
        let _guard = self.acquire_update_check_lock()?;
        let mut state = self.read_update_check_state()?;
        if state.operation_id.as_deref() != Some(operation_id) {
            return Err(RefineError::Conflict(format!(
                "source update check {operation_id} was superseded"
            )));
        }
        state.in_flight = false;
        state.process_id = None;
        state.generation = state.generation.saturating_add(1);
        match result {
            Ok(source) => {
                state.last_successful_check_at = Some(now_timestamp());
                state.current_source_identity = Some(source.current_commit.clone());
                state.available_source_identity = Some(source.available_commit.clone());
                state.source = Some(source);
                state.freshness = "fresh".to_string();
                state.failure = None;
                self.save_update_check_state(&state)?;
                self.operation_registry().finish_with_result(
                    operation_id,
                    OperationState::Succeeded,
                    json!({
                        "current_source_identity": state.current_source_identity,
                        "available_source_identity": state.available_source_identity,
                        "last_successful_check_at": state.last_successful_check_at
                    }),
                )?;
                Ok(())
            }
            Err(error) => {
                state.freshness = freshness(&state, Utc::now());
                state.failure = Some(format!(
                    "Remote update check failed: {error}. Verify connectivity and the configured upstream, then retry"
                ));
                self.save_update_check_state(&state)?;
                let _ = self.operation_registry().fail_with_error(
                    operation_id,
                    json!({
                        "code": "source_update_check_failed",
                        "message": error.to_string(),
                        "retryable": true
                    }),
                );
                Err(error)
            }
        }
    }

    pub(crate) fn refresh_update_check_after_promotion(&self) -> RefineResult<()> {
        let source = self.inspect(false)?;
        let _guard = self.acquire_update_check_lock()?;
        let mut state = self.read_update_check_state()?;
        state.last_check_at = Some(now_timestamp());
        state.last_successful_check_at = state.last_check_at.clone();
        state.current_source_identity = Some(source.current_commit.clone());
        state.available_source_identity = Some(source.available_commit.clone());
        state.source = Some(source);
        state.freshness = "fresh".to_string();
        state.in_flight = false;
        state.failure = None;
        state.process_id = None;
        state.generation = state.generation.saturating_add(1);
        self.save_update_check_state(&state)
    }

    fn reconcile_update_check(&self) -> RefineResult<SourceUpdateCheckState> {
        let _guard = self.acquire_update_check_lock()?;
        self.reconcile_update_check_unlocked()
    }

    fn reconcile_update_check_unlocked(&self) -> RefineResult<SourceUpdateCheckState> {
        let mut state = self.read_update_check_state()?;
        if !state.in_flight {
            if let Some(active) =
                self.operation_registry()
                    .recover()?
                    .into_iter()
                    .find(|operation| {
                        operation.owner == UPDATE_CHECK_OPERATION_OWNER
                            && matches!(
                                operation.state,
                                OperationState::Pending | OperationState::Running
                            )
                    })
            {
                state.in_flight = true;
                state.operation_id = Some(active.id);
                state.last_check_at.get_or_insert_with(now_timestamp);
            } else {
                return Ok(state);
            }
        }
        let Some(operation_id) = state.operation_id.clone() else {
            state.in_flight = false;
            state.failure = Some(
                "The cached update check had no durable operation ownership; retry to register a new supervised check"
                    .to_string(),
            );
            state.generation = state.generation.saturating_add(1);
            self.save_update_check_state(&state)?;
            return Ok(state);
        };
        let registry = self.operation_registry();
        let registry_state = registry.status(&operation_id).ok();
        let processes = self.correlated_processes(&operation_id)?;
        if registry_state.as_ref().is_some_and(|operation| {
            matches!(
                operation.state,
                OperationState::Pending | OperationState::Running
            )
        }) && processes.len() == 1
        {
            state.process_id = Some(processes[0].id.clone());
            self.save_update_check_state(&state)?;
            return Ok(state);
        }
        if processes.len() > 1 {
            let error = RefineError::Degraded(format!(
                "source update check {operation_id} has {} correlated workers; refusing ambiguous ownership",
                processes.len()
            ));
            let _ = registry.fail_with_error(
                &operation_id,
                json!({"code": "source_update_check_duplicate_workers", "message": error.to_string(), "retryable": true}),
            );
            state.failure = Some(format!(
                "{error}. Stop the duplicate supervised maintenance processes, then retry"
            ));
        } else {
            let registry_error = registry_state
                .as_ref()
                .and_then(|operation| operation.error.as_ref())
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str);
            if registry_state.as_ref().is_some_and(|operation| {
                matches!(
                    operation.state,
                    OperationState::Pending | OperationState::Running
                )
            }) {
                let _ = registry.fail_with_error(
                    &operation_id,
                    json!({
                        "code": "source_update_check_interrupted_before_receipt",
                        "message": "No correlated supervised worker survived durable reservation or launch",
                        "retryable": true
                    }),
                );
            }
            state.failure = Some(registry_error.unwrap_or(
                "The supervised remote update check was interrupted before publishing a result",
            ).to_string());
        }
        state.in_flight = false;
        state.process_id = None;
        state.generation = state.generation.saturating_add(1);
        self.save_update_check_state(&state)?;
        Ok(state)
    }

    fn cached_status_from_state(
        &self,
        mut state: SourceUpdateCheckState,
    ) -> RefineResult<CachedSourcePromotionStatus> {
        state.freshness = freshness(&state, Utc::now());
        let mut source = state
            .source
            .clone()
            .unwrap_or_else(|| self.empty_cached_snapshot());
        source.operation = self.reconcile_interrupted_agent()?;
        Ok(CachedSourcePromotionStatus {
            source,
            check: state,
        })
    }

    fn empty_cached_snapshot(&self) -> SourcePromotionSnapshot {
        SourcePromotionSnapshot {
            checkout_path: self.checkout_path.display().to_string(),
            current_commit: String::new(),
            remote: String::new(),
            local_branch: String::new(),
            branch: String::new(),
            available_commit: String::new(),
            clean: false,
            fast_forward: false,
            update_available: false,
            active_work: Vec::new(),
            operation: None,
        }
    }

    fn read_update_check_state(&self) -> RefineResult<SourceUpdateCheckState> {
        match fs::read(self.update_check_state_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse source update check state {}: {error}",
                    self.update_check_state_path().display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SourceUpdateCheckState::default())
            }
            Err(error) => Err(RefineError::Io(format!(
                "failed to read source update check state {}: {error}",
                self.update_check_state_path().display()
            ))),
        }
    }

    fn save_update_check_state(&self, state: &SourceUpdateCheckState) -> RefineResult<()> {
        fs::create_dir_all(&self.port_runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create port runtime root {}: {error}",
                self.port_runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(state).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to encode source update check state: {error}"
            ))
        })?;
        let path = self.update_check_state_path();
        let pending = path.with_extension("json.pending");
        fs::write(&pending, encoded).map_err(|error| {
            RefineError::Io(format!("failed to write {}: {error}", pending.display()))
        })?;
        fs::rename(&pending, &path).map_err(|error| {
            RefineError::Io(format!("failed to publish {}: {error}", path.display()))
        })
    }

    fn acquire_update_check_lock(&self) -> RefineResult<fs::File> {
        fs::create_dir_all(&self.port_runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create update-check runtime root {}: {error}",
                self.port_runtime_root.display()
            ))
        })?;
        let path = self.port_runtime_root.join(UPDATE_CHECK_LOCK_FILE);
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                RefineError::Io(format!("failed to open {}: {error}", path.display()))
            })?;
        file.lock_exclusive().map_err(|error| {
            RefineError::Io(format!("failed to lock {}: {error}", path.display()))
        })?;
        Ok(file)
    }
}

fn freshness(state: &SourceUpdateCheckState, now: DateTime<Utc>) -> String {
    let Some(checked) = state
        .last_successful_check_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    else {
        return "unknown".to_string();
    };
    if now
        .signed_duration_since(checked.with_timezone(&Utc))
        .num_seconds()
        < SOURCE_UPDATE_CHECK_INTERVAL_SECONDS
    {
        "fresh".to_string()
    } else {
        "stale".to_string()
    }
}

fn automatic_check_due(state: &SourceUpdateCheckState, now: DateTime<Utc>) -> bool {
    state
        .last_check_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_none_or(|attempted| {
            now.signed_duration_since(attempted.with_timezone(&Utc))
                .num_seconds()
                >= SOURCE_UPDATE_CHECK_INTERVAL_SECONDS
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn update_check_freshness_uses_the_single_hourly_cadence() {
        let now = Utc::now();
        let state = SourceUpdateCheckState {
            last_successful_check_at: Some(
                (now - chrono::Duration::seconds(SOURCE_UPDATE_CHECK_INTERVAL_SECONDS - 1))
                    .to_rfc3339(),
            ),
            ..Default::default()
        };
        assert_eq!(freshness(&state, now), "fresh");
        let stale = SourceUpdateCheckState {
            last_successful_check_at: Some(
                (now - chrono::Duration::seconds(SOURCE_UPDATE_CHECK_INTERVAL_SECONDS))
                    .to_rfc3339(),
            ),
            ..Default::default()
        };
        assert_eq!(freshness(&stale, now), "stale");
        let failed_recently = SourceUpdateCheckState {
            last_check_at: Some(
                (now - chrono::Duration::seconds(SOURCE_UPDATE_CHECK_INTERVAL_SECONDS - 1))
                    .to_rfc3339(),
            ),
            failure: Some("offline".to_string()),
            ..Default::default()
        };
        assert!(!automatic_check_due(&failed_recently, now));
        assert!(automatic_check_due(
            &SourceUpdateCheckState {
                last_check_at: Some(
                    (now - chrono::Duration::seconds(SOURCE_UPDATE_CHECK_INTERVAL_SECONDS))
                        .to_rfc3339(),
                ),
                ..Default::default()
            },
            now
        ));
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_clients_share_one_real_supervised_fetch_worker() {
        let root = std::env::temp_dir().join(format!(
            "refine-source-check-concurrent-{}",
            uuid::Uuid::new_v4()
        ));
        let checkout = root.join("checkout");
        let runtime = root.join("run/8080");
        fs::create_dir_all(&checkout).unwrap();
        let worker = root.join("worker.sh");
        fs::write(&worker, "#!/bin/sh\nsleep 2\n").unwrap();
        let mut permissions = fs::metadata(&worker).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&worker, permissions).unwrap();
        fs::create_dir_all(checkout.join("bin")).unwrap();
        fs::copy(&worker, checkout.join("bin/refine")).unwrap();

        let service = Arc::new(FileSourcePromotionService::new(&checkout, &runtime, 8080));
        let barrier = Arc::new(Barrier::new(3));
        let clients = (0..2)
            .map(|_| {
                let service = Arc::clone(&service);
                let barrier = Arc::clone(&barrier);
                let worker = worker.clone();
                thread::spawn(move || {
                    barrier.wait();
                    service.request_update_check_with(true, &worker).unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let left = clients[0].thread().id();
        let statuses = clients
            .into_iter()
            .map(|client| client.join().unwrap())
            .collect::<Vec<_>>();
        assert!(statuses.iter().all(|status| status.check.in_flight));
        assert_eq!(
            statuses[0].check.operation_id,
            statuses[1].check.operation_id
        );
        assert_eq!(statuses[0].check.process_id, statuses[1].check.process_id);
        let after_daemon_restart = FileSourcePromotionService::new(&checkout, &runtime, 8080)
            .request_update_check(true)
            .unwrap();
        assert_eq!(
            after_daemon_restart.check.operation_id, statuses[0].check.operation_id,
            "a reconstructed service must reconcile the same live check"
        );
        assert_eq!(
            FileProcessSupervisor::new(&runtime)
                .list()
                .unwrap()
                .into_iter()
                .filter(|process| process.owner == ProcessOwner::Maintenance)
                .count(),
            1,
            "concurrent client {left:?} must share the supervised worker"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cached_reads_do_not_run_git_and_preserve_persisted_identity() {
        let root = std::env::temp_dir().join(format!(
            "refine-source-check-cached-{}",
            uuid::Uuid::new_v4()
        ));
        let service = FileSourcePromotionService::new(root.join("missing"), root.join("run"), 8080);
        let source = SourcePromotionSnapshot {
            current_commit: "aaa".to_string(),
            available_commit: "bbb".to_string(),
            update_available: true,
            ..service.empty_cached_snapshot()
        };
        service
            .save_update_check_state(&SourceUpdateCheckState {
                source: Some(source),
                freshness: "fresh".to_string(),
                ..Default::default()
            })
            .unwrap();
        let cached = service.inspect_cached().unwrap();
        assert_eq!(cached.source.current_commit, "aaa");
        assert_eq!(cached.source.available_commit, "bbb");
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn reservation_without_worker_settles_retryably_after_restart() {
        let root = std::env::temp_dir().join(format!(
            "refine-source-check-reservation-failpoint-{}",
            uuid::Uuid::new_v4()
        ));
        let checkout = root.join("checkout");
        let runtime = root.join("run/8080");
        fs::create_dir_all(&checkout).unwrap();
        let worker = root.join("worker.sh");
        fs::write(&worker, "#!/bin/sh\nsleep 2\n").unwrap();
        let mut permissions = fs::metadata(&worker).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&worker, permissions).unwrap();
        let service = FileSourcePromotionService::new(&checkout, &runtime, 8080);

        let error = service
            .request_update_check_with_failpoint(
                true,
                &worker,
                UpdateCheckLaunchFailpoint::AfterReservation,
            )
            .unwrap_err();
        assert!(error.to_string().contains("after source-check reservation"));
        let operation_id = service
            .read_update_check_state()
            .unwrap()
            .operation_id
            .unwrap();
        let restarted = FileSourcePromotionService::new(&checkout, &runtime, 8080);
        let settled = restarted.inspect_cached().unwrap();
        assert!(!settled.check.in_flight);
        assert!(
            settled
                .check
                .failure
                .as_deref()
                .unwrap()
                .contains("interrupted")
        );
        assert_eq!(
            restarted
                .operation_registry()
                .status(&operation_id)
                .unwrap()
                .state,
            OperationState::Failed
        );

        let retried = restarted
            .request_update_check_with_failpoint(true, &worker, UpdateCheckLaunchFailpoint::None)
            .unwrap();
        assert!(retried.check.in_flight);
        assert_ne!(
            retried.check.operation_id.as_deref(),
            Some(operation_id.as_str())
        );
        if let Some(process_id) = retried.check.process_id {
            let _ = FileProcessSupervisor::new(&runtime).signal(&process_id, "terminate");
        }
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn launched_worker_is_adopted_without_projection_receipt_and_never_duplicated() {
        let root = std::env::temp_dir().join(format!(
            "refine-source-check-launch-failpoint-{}",
            uuid::Uuid::new_v4()
        ));
        let checkout = root.join("checkout");
        let runtime = root.join("run/8080");
        fs::create_dir_all(&checkout).unwrap();
        let worker = root.join("worker.sh");
        fs::write(&worker, "#!/bin/sh\nsleep 3\n").unwrap();
        let mut permissions = fs::metadata(&worker).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&worker, permissions).unwrap();
        let service = FileSourcePromotionService::new(&checkout, &runtime, 8080);

        service
            .request_update_check_with_failpoint(
                true,
                &worker,
                UpdateCheckLaunchFailpoint::AfterLaunchBeforeProjectionReceipt,
            )
            .unwrap_err();
        let operation_id = service
            .read_update_check_state()
            .unwrap()
            .operation_id
            .unwrap();
        let restarted = FileSourcePromotionService::new(&checkout, &runtime, 8080);
        let adopted = restarted.inspect_cached().unwrap();
        assert!(adopted.check.in_flight);
        assert!(adopted.check.process_id.is_some());
        let repeated = restarted
            .request_update_check_with_failpoint(true, &worker, UpdateCheckLaunchFailpoint::None)
            .unwrap();
        assert_eq!(
            repeated.check.operation_id.as_deref(),
            Some(operation_id.as_str())
        );
        let correlated = restarted.correlated_processes(&operation_id).unwrap();
        assert_eq!(correlated.len(), 1);
        let _ = FileProcessSupervisor::new(&runtime).signal(&correlated[0].id, "terminate");
        let _ = fs::remove_dir_all(root);
    }
}

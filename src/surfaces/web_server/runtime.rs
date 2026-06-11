use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::process::subprocess::{FileProcessSupervisor, ProcessSupervisor};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::observability::metrics::PerformanceQuery;
use crate::tools::product::chat::FileChatService;
use crate::tools::product::project_registry::FileProjectRegistryService;
use crate::tools::product::project_state::{
    FileProjectStateStore, ProjectStateStore, ProjectionSnapshot, RuntimeProjection,
};
use crate::tools::product::work_items::FileWorkItemService;

use super::support::*;
use super::*;

const RUNTIME_PROJECTION_CACHE_TTL: Duration = Duration::from_millis(250);

#[derive(Clone, Debug)]
struct RuntimeProjectionCacheEntry {
    projection: RuntimeProjection,
    refreshed_at: Instant,
    fingerprint: RuntimeProjectionFingerprint,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RuntimeProjectionFingerprint {
    entries: BTreeMap<String, RuntimePathFingerprint>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimePathFingerprint {
    len: u64,
    modified_unix_ms: Option<u128>,
}

static HOT_PROJECTIONS: OnceLock<Mutex<BTreeMap<String, ProjectionSnapshot>>> = OnceLock::new();
static HOT_RUNTIME_PROJECTIONS: OnceLock<Mutex<BTreeMap<String, RuntimeProjectionCacheEntry>>> =
    OnceLock::new();

impl InProcessWebServer {
    pub(super) fn app_registry_runtime_root(&self) -> Option<PathBuf> {
        self.runtime_root.as_ref().map(|runtime_root| {
            if runtime_root
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse::<u16>().ok())
                .is_some()
            {
                runtime_root
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| runtime_root.clone())
            } else {
                runtime_root.clone()
            }
        })
    }

    pub(super) fn project_registry_service(&self) -> Option<FileProjectRegistryService> {
        self.app_registry_runtime_root().map(|runtime_root| {
            FileProjectRegistryService::new(runtime_root, self.durable_root.clone())
        })
    }

    pub(super) fn current_projection(&self) -> RefineResult<ProjectionSnapshot> {
        if let Some(durable_root) = self.current_durable_root()? {
            if let Some(runtime_root) = &self.runtime_root {
                let key = projection_cache_key(&durable_root, runtime_root);
                if let Some(snapshot) = hot_projection(&key)? {
                    return Ok(snapshot);
                }
                let store = FileProjectStateStore::with_runtime_root(&durable_root, runtime_root);
                let snapshot = store.load_or_refresh_projection(&runtime_root.join("cache"))?;
                store_hot_projection(key, snapshot.clone())?;
                Ok(snapshot)
            } else {
                let store = FileProjectStateStore::new(durable_root);
                store.rebuild_projection()
            }
        } else {
            Ok(self.projection.clone())
        }
    }

    pub(super) fn current_durable_root(&self) -> RefineResult<Option<PathBuf>> {
        if let Some(runtime_root) = self.app_registry_runtime_root() {
            let registry = FileProjectRegistryService::new(runtime_root, None);
            if let Some(active_app) = registry.load()?.active_app {
                return Ok(Some(PathBuf::from(active_app).join(".refine")));
            }
        }
        Ok(self.durable_root.clone())
    }

    pub(super) fn chat_service(&self, durable_root: &Path) -> FileChatService {
        if let Some(runtime_root) = &self.runtime_root {
            FileChatService::with_runtime_root(durable_root, runtime_root)
        } else {
            FileChatService::new(durable_root)
        }
    }

    pub(super) fn work_item_service(
        &self,
        durable_root: impl Into<PathBuf>,
    ) -> FileWorkItemService {
        let durable_root = durable_root.into();
        if let Some(runtime_root) = &self.runtime_root {
            FileWorkItemService::with_projection_cache(durable_root, runtime_root.join("cache"))
        } else {
            FileWorkItemService::new(durable_root)
        }
    }

    pub(super) fn current_projection_with_runtime(&self) -> RefineResult<ProjectionSnapshot> {
        let mut projection = self.current_projection()?;
        let runtime = self.current_runtime_projection()?;
        if projection.runtime != runtime {
            projection.runtime = runtime;
            self.persist_runtime_projection_snapshot(&projection)?;
            if let (Some(runtime_root), Some(durable_root)) =
                (&self.runtime_root, self.current_durable_root()?)
            {
                store_hot_projection(
                    projection_cache_key(&durable_root, runtime_root),
                    projection.clone(),
                )?;
            }
        }
        Ok(projection)
    }

    pub(super) fn current_runtime_projection(&self) -> RefineResult<RuntimeProjection> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(RuntimeProjection::default());
        };
        let key = runtime_cache_key(runtime_root);
        let durable_root = self.current_durable_root()?;
        let current_fingerprint =
            runtime_projection_fingerprint(runtime_root, durable_root.as_deref())?;
        {
            let cache = HOT_RUNTIME_PROJECTIONS
                .get_or_init(|| Mutex::new(BTreeMap::new()))
                .lock()
                .map_err(|_| {
                    RefineError::Io("runtime projection cache lock was poisoned".to_string())
                })?;
            if let Some(entry) = cache.get(&key)
                && entry.refreshed_at.elapsed() < RUNTIME_PROJECTION_CACHE_TTL
                && entry.fingerprint == current_fingerprint
            {
                return Ok(entry.projection.clone());
            }
        }
        self.refresh_runtime_projection_cache_with_fingerprint(current_fingerprint)
    }

    pub(super) fn refresh_runtime_projection_cache(&self) -> RefineResult<RuntimeProjection> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(RuntimeProjection::default());
        };
        let durable_root = self.current_durable_root()?;
        let fingerprint = runtime_projection_fingerprint(runtime_root, durable_root.as_deref())?;
        self.refresh_runtime_projection_cache_with_fingerprint(fingerprint)
    }

    fn refresh_runtime_projection_cache_with_fingerprint(
        &self,
        fingerprint: RuntimeProjectionFingerprint,
    ) -> RefineResult<RuntimeProjection> {
        let runtime = self.runtime_projection_uncached()?;
        if let Some(runtime_root) = &self.runtime_root {
            let key = runtime_cache_key(runtime_root);
            let mut cache = HOT_RUNTIME_PROJECTIONS
                .get_or_init(|| Mutex::new(BTreeMap::new()))
                .lock()
                .map_err(|_| {
                    RefineError::Io("runtime projection cache lock was poisoned".to_string())
                })?;
            cache.insert(
                key,
                RuntimeProjectionCacheEntry {
                    projection: runtime.clone(),
                    refreshed_at: Instant::now(),
                    fingerprint,
                },
            );
        }
        Ok(runtime)
    }

    fn runtime_projection_uncached(&self) -> RefineResult<RuntimeProjection> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(RuntimeProjection::default());
        };
        let durable_root = self.current_durable_root()?;
        let process =
            process_summary_value_with_chat_sessions(runtime_root, durable_root.as_deref())?;
        let processes = process
            .get("processes")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(value_object)
            .collect::<Vec<_>>();
        let background_operations = FileOperationRegistry::new(runtime_root)
            .recover()?
            .into_iter()
            .map(operation_response)
            .filter_map(value_object)
            .collect::<Vec<_>>();
        let target_app = match self.target_app_service() {
            Ok(service) => {
                let snapshot = service.snapshot()?;
                value_object(self.target_app_response(snapshot))
            }
            Err(_) => None,
        };
        let performance = performance_report_value(runtime_root, PerformanceQuery::default())
            .ok()
            .and_then(value_object);
        let preflight = provider_status_value().ok().and_then(value_object);
        Ok(RuntimeProjection {
            supervisor: value_object(process),
            processes,
            background_operations,
            target_app,
            performance,
            preflight,
        })
    }

    pub(super) fn refresh_projection_cache_after_mutation(&self) -> RefineResult<()> {
        if self.runtime_root.is_none() {
            return Ok(());
        }
        if self.current_durable_root()?.is_none() {
            return Ok(());
        }
        self.rebuild_current_projection_cache()?;
        self.current_projection_with_runtime().map(|_| ())
    }

    pub(super) fn warm_current_projection_cache(&self) -> RefineResult<Option<ProjectionSnapshot>> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(None);
        };
        let Some(durable_root) = self.current_durable_root()? else {
            return Ok(None);
        };
        let snapshot = FileProjectStateStore::with_runtime_root(&durable_root, runtime_root)
            .load_or_refresh_projection(&runtime_root.join("cache"))?;
        store_hot_projection(
            projection_cache_key(&durable_root, runtime_root),
            snapshot.clone(),
        )?;
        let _ = self.refresh_runtime_projection_cache()?;
        Ok(Some(snapshot))
    }

    pub(super) fn rebuild_current_projection_cache(&self) -> RefineResult<ProjectionSnapshot> {
        let projection = self.rebuild_current_project_projection_cache()?;
        let _ = self.refresh_runtime_projection_cache()?;
        Ok(projection)
    }

    pub(super) fn rebuild_current_project_projection_cache(
        &self,
    ) -> RefineResult<ProjectionSnapshot> {
        let Some(runtime_root) = &self.runtime_root else {
            return Err(RefineError::InvalidInput(
                "runtime root is required to rebuild projection cache".to_string(),
            ));
        };
        let Some(durable_root) = self.current_durable_root()? else {
            return Err(RefineError::InvalidInput(
                "durable root is required to rebuild projection cache".to_string(),
            ));
        };
        let store = FileProjectStateStore::with_runtime_root(&durable_root, runtime_root);
        let projection = store.rebuild_projection()?;
        store.persist_projection_snapshot(&runtime_root.join("cache"), &projection)?;
        store_hot_projection(
            projection_cache_key(&durable_root, runtime_root),
            projection.clone(),
        )?;
        Ok(projection)
    }

    pub(super) fn persist_runtime_projection_override(
        &self,
        apply: impl FnOnce(&mut RuntimeProjection),
    ) -> RefineResult<()> {
        let mut projection = self.current_projection_with_runtime()?;
        apply(&mut projection.runtime);
        self.persist_runtime_projection_snapshot(&projection)
    }

    pub(super) fn persist_runtime_projection_snapshot(
        &self,
        projection: &ProjectionSnapshot,
    ) -> RefineResult<()> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(());
        };
        let Some(durable_root) = self.current_durable_root()? else {
            return Ok(());
        };
        FileProjectStateStore::with_runtime_root(durable_root, runtime_root)
            .persist_projection_snapshot(&runtime_root.join("cache"), projection)
    }

    pub(super) fn reconcile_feature_runtime_work(
        &self,
        feature_id: &str,
        gap_ids: &[String],
    ) -> RefineResult<RuntimeReconcileSummary> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(RuntimeReconcileSummary::default());
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        let mut processes = 0;
        for process in supervisor.list()? {
            if process.state == "running" && runtime_record_matches(&process, feature_id, gap_ids) {
                supervisor.signal(&process.id, "terminate")?;
                processes += 1;
            }
        }

        let registry = FileOperationRegistry::new(runtime_root);
        let mut operations = 0;
        for operation in registry.recover()? {
            if matches!(
                operation.state,
                OperationState::Pending | OperationState::Running | OperationState::Cancelling
            ) && operation_owner_matches(&operation.owner, feature_id, gap_ids)
            {
                registry.cancel(&operation.id)?;
                operations += 1;
            }
        }
        Ok(RuntimeReconcileSummary {
            processes,
            operations,
        })
    }

    pub(super) fn source_root(&self) -> Option<PathBuf> {
        self.current_durable_root()
            .ok()
            .flatten()
            .and_then(|root| root.parent().map(Path::to_path_buf))
    }
}

fn projection_cache_key(durable_root: &Path, runtime_root: &Path) -> String {
    format!(
        "{}|{}",
        durable_root.display(),
        runtime_root.join("cache").display()
    )
}

fn runtime_cache_key(runtime_root: &Path) -> String {
    runtime_root.display().to_string()
}

fn hot_projection(key: &str) -> RefineResult<Option<ProjectionSnapshot>> {
    HOT_PROJECTIONS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| RefineError::Io("projection cache lock was poisoned".to_string()))
        .map(|cache| cache.get(key).cloned())
}

fn store_hot_projection(key: String, snapshot: ProjectionSnapshot) -> RefineResult<()> {
    HOT_PROJECTIONS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| RefineError::Io("projection cache lock was poisoned".to_string()))?
        .insert(key, snapshot);
    Ok(())
}

fn runtime_projection_fingerprint(
    runtime_root: &Path,
    durable_root: Option<&Path>,
) -> RefineResult<RuntimeProjectionFingerprint> {
    let mut fingerprint = RuntimeProjectionFingerprint::default();
    for path in [
        runtime_root.join("processes"),
        runtime_root.join("process-control.json"),
        runtime_root.join("operations"),
        runtime_root.join("target-app-state.json"),
        runtime_root.join("runner-health.json"),
        runtime_root.join("metrics/performance.jsonl"),
    ] {
        collect_runtime_path_fingerprint(runtime_root, &path, &mut fingerprint.entries)?;
    }
    if let Some(durable_root) = durable_root {
        let chat_sessions = durable_root.join("chat/sessions");
        collect_runtime_path_fingerprint(runtime_root, &chat_sessions, &mut fingerprint.entries)?;
    }
    Ok(fingerprint)
}

fn collect_runtime_path_fingerprint(
    runtime_root: &Path,
    path: &Path,
    entries: &mut BTreeMap<String, RuntimePathFingerprint>,
) -> RefineResult<()> {
    if is_transient_runtime_path(path) {
        return Ok(());
    }
    if !path.exists() {
        return Ok(());
    }
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to stat runtime path {}: {error}",
                path.display()
            )));
        }
    };
    let relative = path
        .strip_prefix(runtime_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    entries.insert(
        relative,
        RuntimePathFingerprint {
            len: metadata.len(),
            modified_unix_ms: metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis()),
        },
    );
    if metadata.is_dir() && should_scan_runtime_path_children(runtime_root, path) {
        let dir_entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read runtime directory {}: {error}",
                    path.display()
                )));
            }
        };
        for entry in dir_entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to read runtime entry: {error}"
                    )));
                }
            };
            collect_runtime_path_fingerprint(runtime_root, &entry.path(), entries)?;
        }
    }
    Ok(())
}

fn is_transient_runtime_path(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    file_name.starts_with('.') && path.extension().and_then(|value| value.to_str()) == Some("tmp")
}

fn should_scan_runtime_path_children(runtime_root: &Path, path: &Path) -> bool {
    let relative = path
        .strip_prefix(runtime_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    !matches!(relative.as_str(), "processes" | "operations")
}

#[cfg(test)]
mod tests {
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

        let fingerprint = runtime_projection_fingerprint(&runtime_root, None).unwrap();

        assert!(fingerprint.entries.contains_key("processes"));
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
}

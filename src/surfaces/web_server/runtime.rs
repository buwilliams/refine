use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::process::subprocess::{FileProcessSupervisor, ProcessSupervisor};
use crate::process::supervisor::config::FileSettingsService;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry, OperationState,
};
use crate::tools::host::git_sync::FileGitSyncService;
use crate::tools::host::project_layout::{prepare_refine_dir, refine_dir_for_target_root};
use crate::tools::observability::metrics::PerformanceQuery;
use crate::tools::product::chat::FileChatService;
use crate::tools::product::nodes::FileNodeRegistryService;
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
    pub(super) fn current_git_sync_service(&self) -> RefineResult<Option<FileGitSyncService>> {
        let Some(target_root) = self.current_target_root()? else {
            return Ok(None);
        };
        let runtime_root = self
            .runtime_root
            .clone()
            .unwrap_or(refine_dir_for_target_root(&target_root)?.join("runtime"));
        Ok(Some(FileGitSyncService::new(target_root, runtime_root)))
    }

    pub(super) fn app_registry_runtime_root(&self) -> Option<PathBuf> {
        self.runtime_root.clone()
    }

    pub(super) fn project_registry_service(&self) -> Option<FileProjectRegistryService> {
        self.app_registry_runtime_root().map(|runtime_root| {
            FileProjectRegistryService::new(runtime_root, self.target_root.clone())
        })
    }

    pub(super) fn current_projection(&self) -> RefineResult<ProjectionSnapshot> {
        if let Some(refine_dir) = self.current_refine_dir()? {
            if let Some(runtime_root) = &self.runtime_root {
                let key = projection_cache_key(&refine_dir, runtime_root);
                let store = FileProjectStateStore::with_runtime_root(&refine_dir, runtime_root);
                if let Some(snapshot) = hot_projection(&key)? {
                    if snapshot.source_fingerprints == store.collect_source_fingerprints()? {
                        return Ok(snapshot);
                    }
                }
                let snapshot = store.load_or_refresh_projection(&runtime_root.join("cache"))?;
                store_hot_projection(key, snapshot.clone())?;
                Ok(snapshot)
            } else {
                let store = FileProjectStateStore::new(refine_dir);
                store.rebuild_projection()
            }
        } else {
            Ok(self.projection.clone())
        }
    }

    pub(super) fn current_target_root(&self) -> RefineResult<Option<PathBuf>> {
        if let Some(runtime_root) = self.app_registry_runtime_root() {
            let registry = FileProjectRegistryService::new(runtime_root, None);
            if let Some(active_app) = registry.load()?.active_app {
                return Ok(Some(PathBuf::from(active_app)));
            }
        }
        Ok(self.target_root.clone())
    }

    pub(super) fn current_refine_dir(&self) -> RefineResult<Option<PathBuf>> {
        self.current_target_root()?
            .map(|target_root| {
                #[cfg(test)]
                if !target_root.join(".git").exists() {
                    return Ok(target_root.join(".refine"));
                }
                prepare_refine_dir(&target_root)
            })
            .transpose()
    }

    pub(super) fn chat_service(&self, refine_dir: &Path) -> FileChatService {
        if let Some(runtime_root) = &self.runtime_root {
            FileChatService::with_runtime_root(refine_dir, runtime_root)
        } else {
            FileChatService::new(refine_dir)
        }
    }

    pub(super) fn settings_service(&self, refine_dir: impl Into<PathBuf>) -> FileSettingsService {
        let refine_dir = refine_dir.into();
        match &self.runtime_root {
            Some(runtime_root) => FileSettingsService::with_active_root(refine_dir, runtime_root),
            None => FileSettingsService::new(refine_dir),
        }
    }

    pub(super) fn work_item_service(&self, refine_dir: impl Into<PathBuf>) -> FileWorkItemService {
        let refine_dir = refine_dir.into();
        if let Some(runtime_root) = &self.runtime_root {
            FileWorkItemService::with_projection_cache(refine_dir, runtime_root.join("cache"))
        } else {
            FileWorkItemService::new(refine_dir)
        }
    }

    pub(super) fn node_registry_service(
        &self,
        refine_dir: impl Into<PathBuf>,
    ) -> FileNodeRegistryService {
        let refine_dir = refine_dir.into();
        match &self.runtime_root {
            Some(runtime_root) => {
                FileNodeRegistryService::with_active_root(refine_dir, runtime_root)
            }
            None => FileNodeRegistryService::new(refine_dir),
        }
    }

    pub(super) fn current_projection_with_runtime(&self) -> RefineResult<ProjectionSnapshot> {
        let mut projection = self.current_projection()?;
        let runtime = self.current_runtime_projection()?;
        if projection.runtime != runtime {
            projection.runtime = runtime;
            self.persist_runtime_projection_snapshot(&projection)?;
            if let (Some(runtime_root), Some(refine_dir)) =
                (&self.runtime_root, self.current_refine_dir()?)
            {
                store_hot_projection(
                    projection_cache_key(&refine_dir, runtime_root),
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
        let refine_dir = self.current_refine_dir()?;
        let current_fingerprint =
            runtime_projection_fingerprint(runtime_root, refine_dir.as_deref())?;
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
        let refine_dir = self.current_refine_dir()?;
        let fingerprint = runtime_projection_fingerprint(runtime_root, refine_dir.as_deref())?;
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
        let refine_dir = self.current_refine_dir()?;
        let process =
            process_summary_value_with_chat_sessions(runtime_root, refine_dir.as_deref())?;
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
        if self.current_target_root()?.is_none() {
            return Ok(());
        }
        self.rebuild_current_projection_cache()?;
        self.current_projection_with_runtime().map(|_| ())
    }

    pub(super) fn warm_current_projection_cache(&self) -> RefineResult<Option<ProjectionSnapshot>> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(None);
        };
        let Some(refine_dir) = self.current_refine_dir()? else {
            return Ok(None);
        };
        let snapshot = FileProjectStateStore::with_runtime_root(&refine_dir, runtime_root)
            .load_or_refresh_projection(&runtime_root.join("cache"))?;
        store_hot_projection(
            projection_cache_key(&refine_dir, runtime_root),
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
        let Some(refine_dir) = self.current_refine_dir()? else {
            return Err(RefineError::InvalidInput(
                "target root is required to rebuild projection cache".to_string(),
            ));
        };
        let store = FileProjectStateStore::with_runtime_root(&refine_dir, runtime_root);
        let projection = store.rebuild_projection()?;
        store.persist_projection_snapshot(&runtime_root.join("cache"), &projection)?;
        store_hot_projection(
            projection_cache_key(&refine_dir, runtime_root),
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
        let Some(refine_dir) = self.current_refine_dir()? else {
            return Ok(());
        };
        FileProjectStateStore::with_runtime_root(refine_dir, runtime_root)
            .persist_projection_snapshot(&runtime_root.join("cache"), projection)
    }

    pub(super) fn reconcile_feature_runtime_work(
        &self,
        feature_id: &str,
        goal_ids: &[String],
    ) -> RefineResult<RuntimeReconcileSummary> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(RuntimeReconcileSummary::default());
        };
        let supervisor = FileProcessSupervisor::new(runtime_root);
        let mut processes = 0;
        for process in supervisor.list()? {
            if process.state == "running" && runtime_record_matches(&process, feature_id, goal_ids)
            {
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
            ) && operation_owner_matches(&operation.owner, feature_id, goal_ids)
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

    pub(super) fn target_root(&self) -> Option<PathBuf> {
        self.current_target_root().ok().flatten()
    }
}

fn projection_cache_key(refine_dir: &Path, runtime_root: &Path) -> String {
    format!(
        "{}|{}",
        refine_dir.display(),
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
    refine_dir: Option<&Path>,
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
    if let Some(refine_dir) = refine_dir {
        let chat_sessions = refine_dir.join("chat/sessions");
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

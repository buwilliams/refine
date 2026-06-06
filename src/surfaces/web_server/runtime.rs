use std::path::{Path, PathBuf};

use crate::core::host::process_supervision::{FileProcessSupervisor, ProcessSupervisor};
use crate::core::observability::metrics::PerformanceQuery;
use crate::core::product::chat::FileChatService;
use crate::core::product::project_registry::FileProjectRegistryService;
use crate::core::product::project_state::{
    FileProjectStateStore, ProjectStateStore, ProjectionSnapshot, RuntimeProjection,
};
use crate::core::product::work_items::FileWorkItemService;
use crate::core::supervisor::errors::RefineResult;
use crate::core::supervisor::jobs::{FileJobRegistry, JobRegistry, JobState};

use super::support::*;
use super::*;

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
            let store = FileProjectStateStore::new(durable_root);
            if let Some(runtime_root) = &self.runtime_root {
                store.load_or_refresh_projection(&runtime_root.join("cache"))
            } else {
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
        projection.runtime = self.runtime_projection()?;
        self.persist_runtime_projection_snapshot(&projection)?;
        Ok(projection)
    }

    pub(super) fn runtime_projection(&self) -> RefineResult<RuntimeProjection> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(RuntimeProjection::default());
        };
        let process = process_summary_value(runtime_root)?;
        let processes = process
            .get("processes")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(value_object)
            .collect::<Vec<_>>();
        let background_jobs = FileJobRegistry::new(runtime_root)
            .recover()?
            .into_iter()
            .map(job_response)
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
            background_jobs,
            target_app,
            performance,
            preflight,
        })
    }

    pub(super) fn refresh_projection_cache_after_mutation(&self) -> RefineResult<()> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(());
        };
        let Some(durable_root) = self.current_durable_root()? else {
            return Ok(());
        };
        FileProjectStateStore::new(durable_root)
            .load_or_refresh_projection(&runtime_root.join("cache"))
            .map(|_| ())
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
        FileProjectStateStore::new(durable_root)
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

        let registry = FileJobRegistry::new(runtime_root);
        let mut jobs = 0;
        for job in registry.recover()? {
            if matches!(
                job.state,
                JobState::Pending | JobState::Running | JobState::Cancelling
            ) && job_owner_matches(&job.owner, feature_id, gap_ids)
            {
                registry.cancel(&job.id)?;
                jobs += 1;
            }
        }
        Ok(RuntimeReconcileSummary { processes, jobs })
    }

    pub(super) fn source_root(&self) -> Option<PathBuf> {
        self.current_durable_root()
            .ok()
            .flatten()
            .and_then(|root| root.parent().map(Path::to_path_buf))
    }
}

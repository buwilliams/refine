use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::project::{AppRegistry, ProjectStatus, RegisteredApp};

pub const APP_REGISTRY_FILE: &str = "apps.json";

pub trait ProjectRegistryService {
    fn register(&self, app: RegisteredApp) -> RefineResult<AppRegistry>;
    fn attach(&self, path: &str) -> RefineResult<ProjectStatus>;
    fn switch(&self, name: &str) -> RefineResult<ProjectStatus>;
    fn detach(&self) -> RefineResult<ProjectStatus>;
    fn remove(&self, name: &str) -> RefineResult<AppRegistry>;
    fn inspect(&self, path: &str) -> RefineResult<ProjectStatus>;
}

#[derive(Clone, Debug)]
pub struct FileProjectRegistryService {
    pub runtime_root: PathBuf,
    pub current_durable_root: Option<PathBuf>,
}

impl FileProjectRegistryService {
    pub fn new(runtime_root: impl Into<PathBuf>, current_durable_root: Option<PathBuf>) -> Self {
        Self {
            runtime_root: runtime_root.into(),
            current_durable_root,
        }
    }

    pub fn path(&self) -> PathBuf {
        self.runtime_root.join(APP_REGISTRY_FILE)
    }

    pub fn load(&self) -> RefineResult<AppRegistry> {
        let path = self.path();
        if !path.exists() {
            return Ok(default_registry());
        }
        let bytes = fs::read_to_string(&path).map_err(|error| {
            RefineError::Io(format!(
                "failed to read app registry {}: {error}",
                path.display()
            ))
        })?;
        let mut registry = serde_json::from_str::<AppRegistry>(&bytes).map_err(|error| {
            RefineError::Serialization(format!(
                "failed to parse app registry {}: {error}",
                path.display()
            ))
        })?;
        registry.apps.retain(|_, app| !app.path.trim().is_empty());
        Ok(registry)
    }

    pub fn save(&self, registry: &AppRegistry) -> RefineResult<()> {
        fs::create_dir_all(&self.runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create runtime root {}: {error}",
                self.runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_string_pretty(registry).map_err(|error| {
            RefineError::Serialization(format!("failed to encode app registry: {error}"))
        })?;
        let path = self.path();
        fs::write(&path, format!("{encoded}\n")).map_err(|error| {
            RefineError::Io(format!(
                "failed to write app registry {}: {error}",
                path.display()
            ))
        })
    }

    pub fn list_response(&self) -> RefineResult<serde_json::Value> {
        let registry = self.load()?;
        let apps = registry_apps_array(&registry);
        Ok(serde_json::json!({
            "apps": apps,
            "current": registry.active_app.unwrap_or_default(),
            "registry_enabled": true
        }))
    }

    pub fn status(&self) -> RefineResult<ProjectStatus> {
        let mut registry = self.load()?;
        let startup_app = self
            .current_durable_root
            .as_ref()
            .and_then(|root| root.parent())
            .map(|path| path.display().to_string());
        if registry.active_app.is_none() {
            registry.active_app = startup_app.clone();
        }
        let current = registry.active_app.clone();
        if let Some(current) = &startup_app {
            let app = RegisteredApp {
                name: Path::new(current)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(current)
                    .to_string(),
                path: current.clone(),
                added_at: now_timestamp(),
                last_used_at: Some(now_timestamp()),
            };
            upsert_app(&mut registry, app, false);
            self.save(&registry)?;
        }
        let attached = current.is_some();
        let active_refine_root = current
            .as_ref()
            .map(|path| PathBuf::from(path).join(".refine"));
        Ok(ProjectStatus {
            attached,
            registry_enabled: true,
            client_repo: current.clone(),
            volume_root: active_refine_root
                .as_ref()
                .map(|path| path.display().to_string()),
            config_path: active_refine_root
                .as_ref()
                .map(|path| path.join("refine.json").display().to_string()),
            schema: crate::model::project::ProjectSchemaStatus {
                compatible: true,
                migration_required: false,
                schema_version: Some(1),
                current_schema_version: 1,
                reason: None,
                migration_id: None,
                migration_description: None,
                safe_auto: true,
                requires_cluster_quiescence: false,
                operator_instructions: None,
            },
            maintenance: None,
            apps: registry,
            active_node_id: Some("default".to_string()),
            active_node: Some("Default".to_string()),
            message: if attached {
                None
            } else {
                Some("No refine project is attached.".to_string())
            },
        })
    }
}

impl ProjectRegistryService for FileProjectRegistryService {
    fn register(&self, app: RegisteredApp) -> RefineResult<AppRegistry> {
        let mut registry = self.load()?;
        upsert_app(&mut registry, app, false);
        self.save(&registry)?;
        Ok(registry)
    }

    fn attach(&self, path: &str) -> RefineResult<ProjectStatus> {
        let app_path = normalize_app_path(path)?;
        let mut registry = self.load()?;
        upsert_app(
            &mut registry,
            RegisteredApp {
                name: app_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| app_path.display().to_string()),
                path: app_path.display().to_string(),
                added_at: now_timestamp(),
                last_used_at: Some(now_timestamp()),
            },
            true,
        );
        self.save(&registry)?;
        self.inspect(&app_path.display().to_string())
    }

    fn switch(&self, name: &str) -> RefineResult<ProjectStatus> {
        let mut registry = self.load()?;
        let Some(path) = registry
            .apps
            .values()
            .find(|app| app.name == name || app.path == name)
            .map(|app| app.path.clone())
        else {
            return Err(RefineError::NotFound(format!("App {name} was not found")));
        };
        registry.active_app = Some(path.clone());
        if let Some(app) = registry.apps.get_mut(&path) {
            app.last_used_at = Some(now_timestamp());
        }
        self.save(&registry)?;
        self.inspect(&path)
    }

    fn detach(&self) -> RefineResult<ProjectStatus> {
        let mut registry = self.load()?;
        registry.active_app = None;
        self.save(&registry)?;
        Ok(ProjectStatus {
            attached: false,
            registry_enabled: true,
            client_repo: None,
            volume_root: None,
            config_path: None,
            schema: crate::model::project::ProjectSchemaStatus {
                compatible: true,
                migration_required: false,
                schema_version: Some(1),
                current_schema_version: 1,
                reason: None,
                migration_id: None,
                migration_description: None,
                safe_auto: true,
                requires_cluster_quiescence: false,
                operator_instructions: None,
            },
            maintenance: None,
            apps: registry,
            active_node_id: None,
            active_node: None,
            message: Some("No refine project is attached.".to_string()),
        })
    }

    fn remove(&self, name: &str) -> RefineResult<AppRegistry> {
        let mut registry = self.load()?;
        let target = registry
            .apps
            .iter()
            .find(|(_, app)| app.name == name || app.path == name)
            .map(|(key, _)| key.clone());
        let Some(target) = target else {
            return Err(RefineError::NotFound(format!("App {name} was not found")));
        };
        registry.apps.remove(&target);
        if registry.active_app.as_deref() == Some(target.as_str()) {
            registry.active_app = None;
        }
        self.save(&registry)?;
        Ok(registry)
    }

    fn inspect(&self, path: &str) -> RefineResult<ProjectStatus> {
        let mut status = self.status()?;
        if !path.trim().is_empty() {
            let app_path = normalize_app_path(path)?;
            status.attached = true;
            status.client_repo = Some(app_path.display().to_string());
            status.volume_root = Some(app_path.join(".refine").display().to_string());
            status.config_path = Some(app_path.join(".refine/refine.json").display().to_string());
            status.message = None;
        }
        Ok(status)
    }
}

fn default_registry() -> AppRegistry {
    AppRegistry {
        version: 1,
        active_app: None,
        apps: BTreeMap::new(),
    }
}

fn upsert_app(registry: &mut AppRegistry, app: RegisteredApp, make_current: bool) {
    let key = app.path.clone();
    registry.apps.insert(key.clone(), app);
    if make_current {
        registry.active_app = Some(key);
    }
}

pub fn registry_apps_array(registry: &AppRegistry) -> Vec<serde_json::Value> {
    registry
        .apps
        .values()
        .map(|app| {
            serde_json::json!({
                "name": app.name,
                "path": app.path,
                "added_at": app.added_at,
                "last_used_at": app.last_used_at.clone().unwrap_or_default()
            })
        })
        .collect()
}

fn normalize_app_path(path: &str) -> RefineResult<PathBuf> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err(RefineError::InvalidInput("path is required".to_string()));
    }
    let path = PathBuf::from(raw);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| RefineError::Io(format!("failed to inspect cwd: {error}")))?
            .join(path)
    };
    Ok(path)
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_project_registry_persists_apps_and_active_status() {
        let temp_root = unique_temp_dir("project-registry");
        let runtime_root = temp_root.join("run/8080");
        let app_root = temp_root.join("app");
        fs::create_dir_all(app_root.join(".refine")).unwrap();
        let service =
            FileProjectRegistryService::new(&runtime_root, Some(app_root.join(".refine")));

        let status = service.status().unwrap();
        assert!(status.attached);
        assert_eq!(status.apps.apps.len(), 1);
        assert_eq!(
            status.apps.active_app.as_deref(),
            Some(app_root.to_str().unwrap())
        );
        assert!(service.path().exists());

        let listed = service.list_response().unwrap();
        assert_eq!(listed["apps"].as_array().unwrap().len(), 1);

        service.detach().unwrap();
        assert!(service.load().unwrap().active_app.is_none());

        fs::remove_dir_all(temp_root).unwrap();
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-native-{prefix}-{}-{nanos}",
            std::process::id()
        ))
    }
}

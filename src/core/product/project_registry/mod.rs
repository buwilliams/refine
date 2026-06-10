use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::core::host::process_supervision::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner,
};
use crate::core::product::project_migration::FileProjectMigrationService;
use crate::core::supervisor::errors::{RefineError, RefineResult};
use crate::model::project::{
    AppRegistry, ProjectMigrationReport, ProjectSchemaStatus, ProjectStatus, RegisteredApp,
};

pub const APP_REGISTRY_FILE: &str = "apps.json";

pub trait ProjectRegistryService {
    fn register(&self, app: RegisteredApp) -> RefineResult<AppRegistry>;
    fn create_local_project(
        &self,
        path: &str,
        name: Option<&str>,
        make_current: bool,
    ) -> RefineResult<ProjectStatus>;
    fn attach(&self, path: &str) -> RefineResult<ProjectStatus>;
    fn switch(&self, name: &str) -> RefineResult<ProjectStatus>;
    fn detach(&self) -> RefineResult<ProjectStatus>;
    fn clone_app(
        &self,
        source: &str,
        destination: &str,
        name: Option<&str>,
        make_current: bool,
    ) -> RefineResult<ProjectStatus>;
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
        if let Some(current) = &current {
            self.migrate_schema_if_safe(Path::new(current))?;
        }
        let attached = current.is_some();
        project_status_for(registry, current, attached)
    }

    pub fn register_path(
        &self,
        name: Option<&str>,
        path: &str,
        make_current: bool,
    ) -> RefineResult<AppRegistry> {
        let app_path = normalize_app_path(path)?;
        let display_path = app_path.display().to_string();
        let app_name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                app_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| display_path.clone())
            });
        let mut registry = self.load()?;
        upsert_app(
            &mut registry,
            RegisteredApp {
                name: app_name,
                path: display_path,
                added_at: now_timestamp(),
                last_used_at: None,
            },
            make_current,
        );
        if make_current {
            self.ensure_schema_ready(&app_path)?;
        }
        self.save(&registry)?;
        Ok(registry)
    }

    fn clone_repository(&self, source: &str, destination: &Path) -> RefineResult<()> {
        let source = source.trim();
        if source.is_empty() {
            return Err(RefineError::InvalidInput(
                "clone source is required".to_string(),
            ));
        }
        if destination.exists() {
            return Err(RefineError::Conflict(format!(
                "clone destination {} already exists",
                destination.display()
            )));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create clone destination parent {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let output = FileProcessSupervisor::new(&self.runtime_root).run_to_completion(
            ManagedProcessSpec {
                owner: ProcessOwner::Maintenance,
                command: "git".to_string(),
                args: vec![
                    "clone".to_string(),
                    source.to_string(),
                    destination.display().to_string(),
                ],
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: None,
                authorization_command: Some("git clone".to_string()),
                sensitive: false,
            },
        )?;
        if !output.success() {
            let stderr = output.stderr.trim().to_string();
            return Err(RefineError::Conflict(format!(
                "git clone failed{}",
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            )));
        }
        Ok(())
    }

    pub fn attach_with_migration(&self, path: &str) -> RefineResult<ProjectStatus> {
        let app_path = normalize_app_path(path)?;
        if should_create_local_project(&app_path)? {
            return self.create_local_project_at(&app_path, None, true);
        }
        let display_path = app_path.display().to_string();
        self.ensure_schema_ready(&app_path)?;
        let mut registry = self.load()?;
        upsert_app(
            &mut registry,
            RegisteredApp {
                name: app_path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| display_path.clone()),
                path: display_path.clone(),
                added_at: now_timestamp(),
                last_used_at: Some(now_timestamp()),
            },
            true,
        );
        self.save(&registry)?;
        project_status_for(registry, Some(display_path), true)
    }

    pub fn create_local_project_at(
        &self,
        app_path: &Path,
        name: Option<&str>,
        make_current: bool,
    ) -> RefineResult<ProjectStatus> {
        if app_path.exists() && !app_path.is_dir() {
            return Err(RefineError::Conflict(format!(
                "project destination {} exists and is not a directory",
                app_path.display()
            )));
        }
        if app_path.exists() && !should_create_local_project(app_path)? {
            return Err(RefineError::Conflict(format!(
                "project destination {} is not empty",
                app_path.display()
            )));
        }
        if let Some(parent) = app_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create project parent {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::create_dir_all(app_path).map_err(|error| {
            RefineError::Io(format!(
                "failed to create project directory {}: {error}",
                app_path.display()
            ))
        })?;
        self.git_init(app_path)?;
        FileProjectMigrationService::with_runtime_root(
            app_path.join(".refine"),
            self.runtime_root.clone(),
        )
        .initialize_current_schema()?;
        let display_path = app_path.display().to_string();
        self.register_path(name, &display_path, make_current)?;
        self.inspect(&display_path)
    }

    pub fn switch_with_migration(&self, name: &str) -> RefineResult<ProjectStatus> {
        let mut registry = self.load()?;
        let Some(path) = registry
            .apps
            .values()
            .find(|app| app.name == name || app.path == name)
            .map(|app| app.path.clone())
        else {
            return Err(RefineError::NotFound(format!("App {name} was not found")));
        };
        self.ensure_schema_ready(&PathBuf::from(&path))?;
        registry.active_app = Some(path.clone());
        if let Some(app) = registry.apps.get_mut(&path) {
            app.last_used_at = Some(now_timestamp());
        }
        self.save(&registry)?;
        project_status_for(registry, Some(path), true)
    }

    pub fn migrate_current(&self) -> RefineResult<ProjectMigrationReport> {
        let durable_root = self.current_durable_project_root()?;
        FileProjectMigrationService::with_runtime_root(durable_root, self.runtime_root.clone())
            .migrate()
    }

    fn ensure_schema_ready(&self, app_path: &Path) -> RefineResult<()> {
        let migrated = self.migrate_schema_if_safe(app_path)?;
        if migrated {
            return Ok(());
        }
        let durable_root = app_path.join(".refine");
        let service = FileProjectMigrationService::with_runtime_root(
            &durable_root,
            self.runtime_root.clone(),
        );
        let schema = service.status()?;
        if schema.compatible && !schema.migration_required {
            return Ok(());
        }
        if schema.migration_required {
            if schema.safe_auto && !schema.requires_cluster_quiescence {
                service.migrate()?;
                return Ok(());
            }
            return Err(RefineError::Conflict(
                schema
                    .operator_instructions
                    .clone()
                    .unwrap_or_else(|| "project migration required".to_string()),
            ));
        }
        Err(RefineError::Conflict(schema.reason.unwrap_or_else(|| {
            "project schema is not compatible".to_string()
        })))
    }

    fn migrate_schema_if_safe(&self, app_path: &Path) -> RefineResult<bool> {
        let durable_root = app_path.join(".refine");
        let service = FileProjectMigrationService::with_runtime_root(
            &durable_root,
            self.runtime_root.clone(),
        );
        let schema = service.status()?;
        if schema.migration_required && schema.safe_auto && !schema.requires_cluster_quiescence {
            service.migrate()?;
            return Ok(true);
        }
        Ok(false)
    }

    fn current_durable_project_root(&self) -> RefineResult<PathBuf> {
        if let Some(root) = &self.current_durable_root {
            return Ok(root.clone());
        }
        let registry = self.load()?;
        let Some(app) = registry.active_app else {
            return Err(RefineError::NotFound(
                "no active project is attached".to_string(),
            ));
        };
        Ok(PathBuf::from(app).join(".refine"))
    }

    fn git_init(&self, app_path: &Path) -> RefineResult<()> {
        if app_path.join(".git").exists() {
            return Ok(());
        }
        let output = FileProcessSupervisor::new(&self.runtime_root).run_to_completion(
            ManagedProcessSpec {
                owner: ProcessOwner::Maintenance,
                command: "git".to_string(),
                args: vec![
                    "init".to_string(),
                    "-b".to_string(),
                    "main".to_string(),
                    app_path.display().to_string(),
                ],
                cwd: None,
                env: Vec::new(),
                stdin: None,
                limits: None,
                authorization_command: Some("git init".to_string()),
                sensitive: false,
            },
        )?;
        if output.success() {
            Ok(())
        } else {
            let stderr = output.stderr.trim().to_string();
            Err(RefineError::Conflict(format!(
                "git init failed{}",
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            )))
        }
    }
}

impl ProjectRegistryService for FileProjectRegistryService {
    fn register(&self, app: RegisteredApp) -> RefineResult<AppRegistry> {
        let mut registry = self.load()?;
        upsert_app(&mut registry, app, false);
        self.save(&registry)?;
        Ok(registry)
    }

    fn create_local_project(
        &self,
        path: &str,
        name: Option<&str>,
        make_current: bool,
    ) -> RefineResult<ProjectStatus> {
        let app_path = normalize_app_path(path)?;
        self.create_local_project_at(&app_path, name, make_current)
    }

    fn attach(&self, path: &str) -> RefineResult<ProjectStatus> {
        self.attach_with_migration(path)
    }

    fn switch(&self, name: &str) -> RefineResult<ProjectStatus> {
        self.switch_with_migration(name)
    }

    fn detach(&self) -> RefineResult<ProjectStatus> {
        let mut registry = self.load()?;
        registry.active_app = None;
        self.save(&registry)?;
        project_status_for(registry, None, false)
    }

    fn clone_app(
        &self,
        source: &str,
        destination: &str,
        name: Option<&str>,
        make_current: bool,
    ) -> RefineResult<ProjectStatus> {
        let destination = normalize_app_path(destination)?;
        self.clone_repository(source, &destination)?;
        self.register_path(name, &destination.display().to_string(), make_current)?;
        self.inspect(&destination.display().to_string())
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
        let registry = self.load()?;
        if !path.trim().is_empty() {
            let app_path = normalize_app_path(path)?;
            return project_status_for(registry, Some(app_path.display().to_string()), true);
        }
        project_status_for(registry, None, false)
    }
}

fn project_status_for(
    registry: AppRegistry,
    current: Option<String>,
    attached: bool,
) -> RefineResult<ProjectStatus> {
    let active_refine_root = current
        .as_ref()
        .map(|path| PathBuf::from(path).join(".refine"));
    let schema = match &active_refine_root {
        Some(root) => FileProjectMigrationService::new(root).status()?,
        None => detached_schema_status(),
    };
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
        schema,
        maintenance: None,
        apps: registry,
        active_node_id: if attached {
            Some("default".to_string())
        } else {
            None
        },
        active_node: if attached {
            Some("Default".to_string())
        } else {
            None
        },
        message: if attached {
            None
        } else {
            Some("No refine project is attached.".to_string())
        },
    })
}

fn detached_schema_status() -> ProjectSchemaStatus {
    ProjectSchemaStatus {
        compatible: true,
        migration_required: false,
        schema_version: None,
        current_schema_version:
            crate::core::product::project_migration::CURRENT_PROJECT_SCHEMA_VERSION,
        reason: None,
        migration_id: None,
        migration_description: None,
        safe_auto: true,
        requires_cluster_quiescence: false,
        operator_instructions: None,
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

fn should_create_local_project(app_path: &Path) -> RefineResult<bool> {
    if !app_path.exists() {
        return Ok(true);
    }
    if !app_path.is_dir() || app_path.join(".git").exists() {
        return Ok(false);
    }
    let mut entries = fs::read_dir(app_path).map_err(|error| {
        RefineError::Io(format!(
            "failed to inspect project directory {}: {error}",
            app_path.display()
        ))
    })?;
    Ok(entries.next().is_none())
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
    let path = expand_home_path(raw);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| RefineError::Io(format!("failed to inspect cwd: {error}")))?
            .join(path)
    };
    Ok(path)
}

fn expand_home_path(raw: &str) -> PathBuf {
    if raw == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(raw)
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn normalize_app_path_expands_home_prefix() {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };

        assert_eq!(
            normalize_app_path("~/refine-test-app").unwrap(),
            home.join("refine-test-app")
        );
    }

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

    #[test]
    fn file_project_registry_clones_and_registers_app() {
        let temp_root = unique_temp_dir("project-registry-clone");
        let runtime_root = temp_root.join("run/8080");
        let source = temp_root.join("source");
        let destination = temp_root.join("cloned-app");
        fs::create_dir_all(&source).unwrap();
        let output = Command::new("git")
            .arg("init")
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let service = FileProjectRegistryService::new(&runtime_root, None);
        let status = service
            .clone_app(
                source.to_str().unwrap(),
                destination.to_str().unwrap(),
                Some("cloned"),
                true,
            )
            .unwrap();
        assert!(destination.join(".git").exists());
        assert_eq!(
            status.client_repo.as_deref(),
            Some(destination.to_str().unwrap())
        );
        let registry = service.load().unwrap();
        assert_eq!(
            registry.active_app.as_deref(),
            Some(destination.to_str().unwrap())
        );
        assert_eq!(
            registry
                .apps
                .get(destination.to_str().unwrap())
                .unwrap()
                .name,
            "cloned"
        );

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn file_project_registry_attach_creates_missing_local_project() {
        let temp_root = unique_temp_dir("project-registry-create-local");
        let runtime_root = temp_root.join("run/8080");
        let destination = temp_root.join("new-app");
        let service = FileProjectRegistryService::new(&runtime_root, None);

        let status = service.attach(destination.to_str().unwrap()).unwrap();

        assert_eq!(
            status.client_repo.as_deref(),
            Some(destination.to_str().unwrap())
        );
        assert!(destination.join(".git").exists());
        assert!(destination.join(".refine/refine.json").exists());
        assert!(runtime_root.join("processes").exists());
        assert!(!destination.join(".refine/runtime/processes").exists());
        let registry = service.load().unwrap();
        assert_eq!(
            registry.active_app.as_deref(),
            Some(destination.to_str().unwrap())
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

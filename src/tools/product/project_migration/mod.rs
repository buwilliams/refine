use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};

use crate::model::JsonObject;
use crate::model::project::{
    ProjectConfig, ProjectMigrationReport, ProjectSchemaStatus, RefineVersion,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::tools::product::project_state::PROJECTION_SNAPSHOT_FILE;

pub const CURRENT_PROJECT_SCHEMA_VERSION: u64 = 1;
const LEGACY_0_TO_1_ID: &str = "legacy-0-to-1";

#[derive(Clone, Debug)]
pub struct FileProjectMigrationService {
    pub durable_root: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

impl FileProjectMigrationService {
    pub fn new(durable_root: impl Into<PathBuf>) -> Self {
        Self {
            durable_root: durable_root.into(),
            runtime_root: None,
        }
    }

    pub fn with_runtime_root(
        durable_root: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            durable_root: durable_root.into(),
            runtime_root: Some(runtime_root.into()),
        }
    }

    pub fn status(&self) -> RefineResult<ProjectSchemaStatus> {
        schema_status(&self.durable_root)
    }

    pub fn initialize_current_schema(&self) -> RefineResult<()> {
        let schema = self.status()?;
        if schema.compatible && !schema.migration_required && schema.schema_version.is_some() {
            return Ok(());
        }
        if schema.migration_required {
            return Err(RefineError::Conflict(
                schema
                    .operator_instructions
                    .clone()
                    .or(schema.reason.clone())
                    .unwrap_or_else(|| "project migration required".to_string()),
            ));
        }
        if !schema.compatible {
            return Err(RefineError::Conflict(
                schema
                    .reason
                    .unwrap_or_else(|| "project schema is not compatible".to_string()),
            ));
        }
        fs::create_dir_all(&self.durable_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create durable root {}: {error}",
                self.durable_root.display()
            ))
        })?;
        write_json_atomic(&self.config_path(), &current_project_config())
    }

    pub fn migrate(&self) -> RefineResult<ProjectMigrationReport> {
        let before = self.status()?;
        if before.compatible && !before.migration_required {
            return Ok(ProjectMigrationReport {
                ok: true,
                migrated: false,
                from_version: before.schema_version,
                to_version: CURRENT_PROJECT_SCHEMA_VERSION,
                applied: Vec::new(),
                skipped: Vec::new(),
                warnings: Vec::new(),
                backup_path: None,
                schema: before,
            });
        }
        if !before.migration_required {
            return Err(RefineError::Conflict(
                before
                    .reason
                    .clone()
                    .unwrap_or_else(|| "project schema is not compatible".to_string()),
            ));
        }
        if !before.safe_auto || before.requires_cluster_quiescence {
            return Err(RefineError::Conflict(
                before
                    .operator_instructions
                    .clone()
                    .unwrap_or_else(|| "manual project migration required".to_string()),
            ));
        }

        let from_version = before.schema_version.unwrap_or(0);
        let backup_path = match from_version {
            0 => self.apply_legacy_0_to_1()?,
            version => {
                return Err(RefineError::Conflict(format!(
                    "no project migration is available from schema version {version}"
                )));
            }
        };
        self.invalidate_projection_cache()?;
        let after = self.status()?;
        Ok(ProjectMigrationReport {
            ok: true,
            migrated: true,
            from_version: Some(from_version),
            to_version: CURRENT_PROJECT_SCHEMA_VERSION,
            applied: vec![LEGACY_0_TO_1_ID.to_string()],
            skipped: Vec::new(),
            warnings: Vec::new(),
            backup_path: Some(backup_path.display().to_string()),
            schema: after,
        })
    }

    fn apply_legacy_0_to_1(&self) -> RefineResult<PathBuf> {
        fs::create_dir_all(&self.durable_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create durable root {}: {error}",
                self.durable_root.display()
            ))
        })?;
        let backup_dir = self.backup_dir(LEGACY_0_TO_1_ID);
        fs::create_dir_all(&backup_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create migration backup {}: {error}",
                backup_dir.display()
            ))
        })?;
        let config_path = self.config_path();
        if config_path.exists() {
            fs::copy(&config_path, backup_dir.join("refine.json")).map_err(|error| {
                RefineError::Io(format!(
                    "failed to back up {}: {error}",
                    config_path.display()
                ))
            })?;
        }
        let manifest = json!({
            "migration_id": LEGACY_0_TO_1_ID,
            "created_at": now_timestamp(),
            "changed_files": ["refine.json"]
        });
        write_json_atomic(&backup_dir.join("manifest.json"), &manifest)?;

        write_json_atomic(&config_path, &current_project_config())?;
        Ok(backup_dir)
    }

    fn backup_dir(&self, migration_id: &str) -> PathBuf {
        self.durable_root
            .join("backups")
            .join("migrations")
            .join(format!("{}-{migration_id}", backup_timestamp()))
    }

    fn config_path(&self) -> PathBuf {
        self.durable_root.join("refine.json")
    }

    fn invalidate_projection_cache(&self) -> RefineResult<()> {
        let Some(runtime_root) = &self.runtime_root else {
            return Ok(());
        };
        remove_file_if_exists(&runtime_root.join("cache").join(PROJECTION_SNAPSHOT_FILE))?;
        let Ok(entries) = fs::read_dir(runtime_root) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                remove_file_if_exists(&entry.path().join("cache").join(PROJECTION_SNAPSHOT_FILE))?;
            }
        }
        Ok(())
    }
}

pub fn schema_status(durable_root: &Path) -> RefineResult<ProjectSchemaStatus> {
    let config_path = durable_root.join("refine.json");
    if !config_path.exists() {
        if durable_state_exists(durable_root)? {
            return Ok(migration_required_status(
                Some(0),
                LEGACY_0_TO_1_ID,
                "Create project schema metadata for legacy .refine state.",
            ));
        }
        return Ok(ProjectSchemaStatus {
            compatible: true,
            migration_required: false,
            schema_version: None,
            current_schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
            reason: None,
            migration_id: None,
            migration_description: None,
            safe_auto: true,
            requires_cluster_quiescence: false,
            operator_instructions: None,
        });
    }

    let bytes = fs::read(&config_path).map_err(|error| {
        RefineError::Io(format!(
            "failed to read project config {}: {error}",
            config_path.display()
        ))
    })?;
    let value = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
        RefineError::Serialization(format!(
            "failed to parse project config {}: {error}",
            config_path.display()
        ))
    })?;
    let Some(version) = value.get("schema_version").and_then(Value::as_u64) else {
        return Ok(migration_required_status(
            Some(0),
            LEGACY_0_TO_1_ID,
            "Normalize project config to schema version 1.",
        ));
    };
    if version == CURRENT_PROJECT_SCHEMA_VERSION {
        return Ok(ProjectSchemaStatus {
            compatible: true,
            migration_required: false,
            schema_version: Some(version),
            current_schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
            reason: None,
            migration_id: None,
            migration_description: None,
            safe_auto: true,
            requires_cluster_quiescence: false,
            operator_instructions: None,
        });
    }
    if version < CURRENT_PROJECT_SCHEMA_VERSION {
        return Ok(migration_required_status(
            Some(version),
            LEGACY_0_TO_1_ID,
            "Update project schema metadata to the current Refine schema.",
        ));
    }
    Ok(ProjectSchemaStatus {
        compatible: false,
        migration_required: false,
        schema_version: Some(version),
        current_schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        reason: Some(format!(
            "project schema version {version} is newer than this Refine supports"
        )),
        migration_id: None,
        migration_description: None,
        safe_auto: false,
        requires_cluster_quiescence: false,
        operator_instructions: Some("Upgrade Refine before opening this app.".to_string()),
    })
}

fn migration_required_status(
    schema_version: Option<u64>,
    migration_id: &str,
    description: &str,
) -> ProjectSchemaStatus {
    ProjectSchemaStatus {
        compatible: false,
        migration_required: true,
        schema_version,
        current_schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        reason: Some("project migration required".to_string()),
        migration_id: Some(migration_id.to_string()),
        migration_description: Some(description.to_string()),
        safe_auto: true,
        requires_cluster_quiescence: false,
        operator_instructions: None,
    }
}

fn durable_state_exists(durable_root: &Path) -> RefineResult<bool> {
    let entries = match fs::read_dir(durable_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to inspect durable root {}: {error}",
                durable_root.display()
            )));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect durable root {}: {error}",
                durable_root.display()
            ))
        })?;
        if entry.file_name().to_str() == Some("backups") {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

fn write_json_atomic(path: &Path, value: &impl serde::Serialize) -> RefineResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RefineError::Io(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    let temp_path = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut encoded = serde_json::to_vec_pretty(value)
        .map_err(|error| RefineError::Serialization(format!("failed to encode JSON: {error}")))?;
    encoded.push(b'\n');
    fs::write(&temp_path, encoded).map_err(|error| {
        RefineError::Io(format!(
            "failed to write temp file {}: {error}",
            temp_path.display()
        ))
    })?;
    fs::rename(&temp_path, path).map_err(|error| {
        RefineError::Io(format!("failed to commit JSON {}: {error}", path.display()))
    })
}

fn current_project_config() -> ProjectConfig {
    ProjectConfig {
        schema_version: CURRENT_PROJECT_SCHEMA_VERSION,
        refine: RefineVersion {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        created_at: now_timestamp(),
        updated_at: now_timestamp(),
        settings: JsonObject::new(),
    }
}

fn remove_file_if_exists(path: &Path) -> RefineResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RefineError::Io(format!(
            "failed to remove cache file {}: {error}",
            path.display()
        ))),
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn backup_timestamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn schema_status_detects_current_legacy_empty_and_newer_projects() {
        let temp_root = unique_temp_dir("migration-status");
        let current = temp_root.join("current/.refine");
        fs::create_dir_all(&current).unwrap();
        write_json_atomic(
            &current.join("refine.json"),
            &json!({
                "schema_version": CURRENT_PROJECT_SCHEMA_VERSION,
                "refine": {"version": "test"},
                "created_at": "now",
                "updated_at": "now",
                "settings": {}
            }),
        )
        .unwrap();
        let current_status = schema_status(&current).unwrap();
        assert!(current_status.compatible);
        assert!(!current_status.migration_required);

        let legacy = temp_root.join("legacy/.refine");
        fs::create_dir_all(legacy.join("gaps")).unwrap();
        let legacy_status = schema_status(&legacy).unwrap();
        assert!(!legacy_status.compatible);
        assert!(legacy_status.migration_required);
        assert_eq!(legacy_status.schema_version, Some(0));
        assert_eq!(
            legacy_status.migration_id.as_deref(),
            Some(LEGACY_0_TO_1_ID)
        );

        let empty = temp_root.join("empty/.refine");
        let empty_status = schema_status(&empty).unwrap();
        assert!(empty_status.compatible);
        assert!(!empty_status.migration_required);
        assert_eq!(empty_status.schema_version, None);

        let newer = temp_root.join("newer/.refine");
        fs::create_dir_all(&newer).unwrap();
        write_json_atomic(
            &newer.join("refine.json"),
            &json!({"schema_version": CURRENT_PROJECT_SCHEMA_VERSION + 1}),
        )
        .unwrap();
        let newer_status = schema_status(&newer).unwrap();
        assert!(!newer_status.compatible);
        assert!(!newer_status.migration_required);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn migration_creates_config_backup_and_invalidates_projection_cache() {
        let temp_root = unique_temp_dir("migration-run");
        let durable_root = temp_root.join("app/.refine");
        let runtime_root = temp_root.join("run");
        fs::create_dir_all(durable_root.join("gaps/GA")).unwrap();
        fs::write(durable_root.join("gaps/GA/gap.json"), "{}").unwrap();
        fs::create_dir_all(runtime_root.join("cache")).unwrap();
        fs::write(
            runtime_root.join("cache").join(PROJECTION_SNAPSHOT_FILE),
            "{}",
        )
        .unwrap();

        let service = FileProjectMigrationService::with_runtime_root(&durable_root, &runtime_root);
        let report = service.migrate().unwrap();
        assert!(report.ok);
        assert!(report.migrated);
        assert_eq!(report.from_version, Some(0));
        assert_eq!(report.to_version, CURRENT_PROJECT_SCHEMA_VERSION);
        assert_eq!(report.applied, vec![LEGACY_0_TO_1_ID]);
        assert!(durable_root.join("refine.json").exists());
        assert!(
            PathBuf::from(report.backup_path.unwrap())
                .join("manifest.json")
                .exists()
        );
        assert!(
            !runtime_root
                .join("cache")
                .join(PROJECTION_SNAPSHOT_FILE)
                .exists()
        );

        let second = service.migrate().unwrap();
        assert!(second.ok);
        assert!(!second.migrated);
        assert_eq!(second.from_version, Some(CURRENT_PROJECT_SCHEMA_VERSION));

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

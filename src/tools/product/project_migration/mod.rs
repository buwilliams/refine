use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::Value;

use crate::model::JsonObject;
use crate::model::project::{
    ProjectConfig, ProjectMigrationReport, ProjectSchemaStatus, RefineVersion,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};
pub const CURRENT_PROJECT_SCHEMA_VERSION: u64 = 2;
const LEGACY_0_TO_1_ID: &str = "legacy-0-to-1";
const GOALS_PROMPT_1_TO_2_ID: &str = "goals-prompt-1-to-2";
const GAP_TO_GOAL_RUNBOOK: &str = "docs/runbooks/migrate-gap-state.md";

#[derive(Clone, Debug)]
pub struct FileProjectMigrationService {
    pub refine_dir: PathBuf,
    pub runtime_root: Option<PathBuf>,
}

impl FileProjectMigrationService {
    pub fn new(refine_dir: impl Into<PathBuf>) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: None,
        }
    }

    pub fn with_runtime_root(
        refine_dir: impl Into<PathBuf>,
        runtime_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            refine_dir: refine_dir.into(),
            runtime_root: Some(runtime_root.into()),
        }
    }

    pub fn status(&self) -> RefineResult<ProjectSchemaStatus> {
        schema_status(&self.refine_dir)
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
        fs::create_dir_all(&self.refine_dir).map_err(|error| {
            RefineError::Io(format!(
                "failed to create refine dir {}: {error}",
                self.refine_dir.display()
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

        Err(RefineError::Conflict(
            before.operator_instructions.unwrap_or_else(|| {
                format!("An agent must migrate this project using {GAP_TO_GOAL_RUNBOOK}")
            }),
        ))
    }

    fn config_path(&self) -> PathBuf {
        self.refine_dir.join("refine.json")
    }
}

pub fn schema_status(refine_dir: &Path) -> RefineResult<ProjectSchemaStatus> {
    let config_path = refine_dir.join("refine.json");
    if !config_path.exists() {
        if refine_state_exists(refine_dir)? {
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
            if version == 1 {
                GOALS_PROMPT_1_TO_2_ID
            } else {
                LEGACY_0_TO_1_ID
            },
            if version == 1 {
                "Rename Gap records to Goals and consolidate round actual/target fields into prompt."
            } else {
                "Update project schema metadata to the current Refine schema."
            },
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
        safe_auto: false,
        requires_cluster_quiescence: true,
        operator_instructions: Some(format!(
            "Use a migration agent and follow {GAP_TO_GOAL_RUNBOOK}; this semantic migration is not performed by deterministic application code."
        )),
    }
}

fn refine_state_exists(refine_dir: &Path) -> RefineResult<bool> {
    let entries = match fs::read_dir(refine_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to inspect refine dir {}: {error}",
                refine_dir.display()
            )));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            RefineError::Io(format!(
                "failed to inspect refine dir {}: {error}",
                refine_dir.display()
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
    project_config(CURRENT_PROJECT_SCHEMA_VERSION)
}

fn project_config(schema_version: u64) -> ProjectConfig {
    ProjectConfig {
        schema_version,
        refine: RefineVersion {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        created_at: now_timestamp(),
        updated_at: now_timestamp(),
        settings: JsonObject::new(),
    }
}

fn now_timestamp() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
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
            &serde_json::json!({
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
        fs::create_dir_all(legacy.join("goals")).unwrap();
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
            &serde_json::json!({"schema_version": CURRENT_PROJECT_SCHEMA_VERSION + 1}),
        )
        .unwrap();
        let newer_status = schema_status(&newer).unwrap();
        assert!(!newer_status.compatible);
        assert!(!newer_status.migration_required);

        fs::remove_dir_all(temp_root).unwrap();
    }

    #[test]
    fn gap_to_goal_migration_requires_an_agent() {
        let temp_root = unique_temp_dir("migration-requires-agent");
        let refine_dir = temp_root.join("app/.refine");
        fs::create_dir_all(refine_dir.join("gaps/GA/P1")).unwrap();
        write_json_atomic(
            &refine_dir.join("refine.json"),
            &serde_json::json!({"schema_version": 1}),
        )
        .unwrap();
        let service = FileProjectMigrationService::new(&refine_dir);
        let status = service.status().unwrap();
        assert!(status.migration_required);
        assert!(!status.safe_auto);
        assert!(status.requires_cluster_quiescence);
        assert!(
            status
                .operator_instructions
                .unwrap()
                .contains(GAP_TO_GOAL_RUNBOOK)
        );
        let error = service.migrate().unwrap_err().to_string();
        assert!(error.contains("migration agent"));
        assert!(refine_dir.join("gaps/GA/P1").exists());
        assert!(!refine_dir.join("goals").exists());

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

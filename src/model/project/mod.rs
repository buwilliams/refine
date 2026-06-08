use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{JsonObject, Timestamp};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectConfig {
    pub schema_version: u64,
    pub refine: RefineVersion,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub settings: JsonObject,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RefineVersion {
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectStatus {
    pub attached: bool,
    pub registry_enabled: bool,
    pub client_repo: Option<String>,
    pub volume_root: Option<String>,
    pub config_path: Option<String>,
    pub schema: ProjectSchemaStatus,
    pub maintenance: Option<ProjectMaintenance>,
    pub apps: AppRegistry,
    pub active_node_id: Option<String>,
    pub active_node: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectSchemaStatus {
    pub compatible: bool,
    pub migration_required: bool,
    pub schema_version: Option<u64>,
    pub current_schema_version: u64,
    pub reason: Option<String>,
    pub migration_id: Option<String>,
    pub migration_description: Option<String>,
    pub safe_auto: bool,
    pub requires_cluster_quiescence: bool,
    pub operator_instructions: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectMigrationReport {
    pub ok: bool,
    pub migrated: bool,
    pub from_version: Option<u64>,
    pub to_version: u64,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    pub backup_path: Option<String>,
    pub schema: ProjectSchemaStatus,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProjectMaintenance {
    pub active: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub reason: Option<String>,
    pub details: Option<JsonObject>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AppRegistry {
    pub version: u64,
    pub active_app: Option<String>,
    pub apps: BTreeMap<String, RegisteredApp>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RegisteredApp {
    pub name: String,
    pub path: String,
    pub added_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PrimaryRuntime {
    pub port: u16,
    pub active_node_id: Option<String>,
}

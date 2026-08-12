use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::process::subprocess::FileProcessSupervisor;
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::BackgroundDaemonConfig;
use crate::process::supervisor::operations::{FileOperationRegistry, OperationRegistry};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::process::supervisor::security::{NativeSecretStore, SecretStore};
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::tools::host::daemon_lifecycle::{
    DaemonLifecycleAction, FileDaemonLifecycleOperationService, FileHostDaemonLifecycleService,
    execute_daemon_lifecycle,
};
use crate::tools::host::deployed_update::{discover_refine_checkout, is_refine_checkout};
use crate::tools::host::installation::{FileInstallationService, InstallationService};
use crate::tools::host::release::{FileReleaseService, ReleaseBump};
use crate::tools::host::source_promotion::{
    FileSourcePromotionService, source_promotion_affordance,
};
use crate::tools::observability::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::tools::observability::processes::FileProcessStatusService;
use crate::tools::observability::support_bundle::{FileSupportBundleService, SupportBundleService};
use crate::tools::product::process_control::FileProcessControlService;
use crate::workflow::WorkflowEngine;

use super::support::*;
use super::*;

#[derive(Clone, Debug)]
struct DiagnosticsCacheEntry {
    value: Value,
}

static DIAGNOSTICS_CACHE: OnceLock<Mutex<BTreeMap<String, DiagnosticsCacheEntry>>> =
    OnceLock::new();

mod agents;
mod diagnostics;
mod helpers;
mod installation;
mod operations;
mod pause;
mod processes;
mod releases;

use helpers::*;

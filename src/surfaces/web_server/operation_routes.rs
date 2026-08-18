use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde_json::{Value, json};

use crate::application::diagnostics::processes::{
    FileProcessStatusService, enrich_process_resource_usage, repository_disk_usage_value,
};
use crate::application::diagnostics::support_bundle::{
    FileSupportBundleService, SupportBundleService,
};
use crate::application::diagnostics::{DiagnosticsService, FileDiagnosticsService};
use crate::application::operations::process_control::FileProcessControlService;
use crate::application::system::daemon_lifecycle::{
    DaemonLifecycleAction, FileDaemonLifecycleOperationService, FileHostDaemonLifecycleService,
    execute_daemon_lifecycle,
};
use crate::application::system::installation::{FileInstallationService, InstallationService};
use crate::application::system::release::{FileReleaseService, ReleaseBump};
use crate::application::system::source_promotion::{
    FileSourcePromotionService, source_promotion_affordance,
};
use crate::application::workers::{BackgroundWorkerEnsure, FileRunnerWorkerService};
use crate::application::workflow::WorkflowEngine;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::infrastructure::process::subprocess::FileProcessSupervisor;
use crate::infrastructure::process::supervisor::lifecycle::BackgroundDaemonConfig;
use crate::infrastructure::process::supervisor::operations::{
    FileOperationRegistry, OperationRegistry,
};
use crate::infrastructure::process::supervisor::runtime::RuntimeRoot;
use crate::infrastructure::process::supervisor::security::{NativeSecretStore, SecretStore};
use crate::infrastructure::runtime::checkout::is_refine_checkout;

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

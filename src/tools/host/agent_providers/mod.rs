use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::process::launch_environment::EffectiveLaunchEnvironment;
use crate::process::subprocess::{
    FileProcessSupervisor, ManagedProcessOutputStream, ManagedProcessSpec, ProcessOwner,
    ProcessResourceLimits,
};
use crate::process::supervisor::errors::{RefineError, RefineResult};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderCapability {
    pub name: String,
    pub display_name: String,
    pub binary: String,
    pub installed: bool,
    pub path: Option<String>,
    pub supports_resume: bool,
    pub supports_direct_api: bool,
    pub supports_cli: bool,
    pub output_format: String,
    #[serde(default)]
    pub prompt_transport: ProviderPromptCapability,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPromptCapability {
    NativeStdin,
    #[default]
    InlineOrFile,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderInvocation {
    pub provider: String,
    pub prompt: String,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub process_metadata: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderInvocationResult {
    pub output: String,
    pub provider_session_id: Option<String>,
    pub raw_output: String,
}

#[derive(Clone, Debug)]
pub struct PreparedProviderLaunch {
    pub provider: String,
    pub display_name: String,
    pub binary: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub prompt_transport: PromptTransportMetadata,
    pub prompt_artifact: Option<PromptArtifactLease>,
    pub authorization_command: String,
    pub launch_environment: EffectiveLaunchEnvironment,
}

impl PreparedProviderLaunch {
    pub fn validate_prompt_artifact(&self) -> RefineResult<()> {
        if let Some(artifact) = &self.prompt_artifact {
            artifact.validate()
        } else {
            Ok(())
        }
    }
}

pub type InteractiveProviderCommand = PreparedProviderLaunch;

pub trait AgentProviderService {
    fn detect(&self) -> RefineResult<Vec<ProviderCapability>>;
    fn configure(&self, provider: &str) -> RefineResult<()>;
    fn authenticate(&self, provider: &str) -> RefineResult<()>;
    fn invoke(&self, invocation: ProviderInvocation) -> RefineResult<String>;
    fn resume(&self, provider: &str, session_id: &str) -> RefineResult<String>;
    fn diagnose(&self, provider: &str) -> RefineResult<Vec<String>>;
}

#[cfg(test)]
pub fn smoke_ai_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

mod activity;
mod output_parser;
mod prompt_transport;
mod service;
mod spec;

pub use prompt_transport::{PromptArtifactLease, PromptTransportKind, PromptTransportMetadata};
pub use service::HostAgentProviderService;

use activity::*;
use output_parser::*;
use prompt_transport::*;
use spec::*;

#[cfg(test)]
mod tests;

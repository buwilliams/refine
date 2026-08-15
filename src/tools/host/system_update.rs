//! The one-command update path: `./r system update` drives the same
//! restart-safe source promotion the web "Update Refine" control queues, adds
//! a blocking wait with progress reporting, and escalates blockers that
//! deterministic logic cannot clear to the configured agent CLI exactly once.

use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};

use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{BackgroundDaemonConfig, http_probe};
use crate::process::supervisor::runtime::RuntimeRoot;
use crate::tools::host::agent_providers::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation, resolve_agent_provider,
};
use crate::tools::host::checkout::RefineCheckoutPaths;
use crate::tools::host::daemon_lifecycle::{
    DaemonLifecycleAction, FileHostDaemonLifecycleService, execute_daemon_lifecycle,
};
use crate::tools::host::source_promotion::{
    FileSourcePromotionService, SourcePromotionOperation, SourcePromotionSnapshot,
};

const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(180);
const UPDATE_WAIT_TIMEOUT: Duration = Duration::from_secs(900);
const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(30);
const RESCUE_OUTPUT_TAIL_CHARS: usize = 2000;

#[derive(Clone, Debug)]
pub struct SystemUpdateOptions {
    pub provider: Option<String>,
    pub port: u16,
    pub rescue: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemUpdateReport {
    pub ok: bool,
    /// "updated" | "already_current" | "failed" | "timed_out" | "blocked"
    pub outcome: String,
    pub checkout_path: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_commit: Option<String>,
    pub daemon_started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stashed_changes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<SourcePromotionOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescue: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

impl SystemUpdateReport {
    fn new(paths: &RefineCheckoutPaths, port: u16) -> Self {
        Self {
            ok: false,
            outcome: "failed".to_string(),
            checkout_path: paths.checkout.display().to_string(),
            port,
            from_commit: None,
            to_commit: None,
            daemon_started: false,
            stashed_changes: None,
            operation: None,
            rescue: None,
            blocker: None,
        }
    }

    fn blocked(mut self, blocker: String) -> Self {
        self.outcome = "blocked".to_string();
        self.blocker = Some(blocker);
        self
    }

    fn failed(mut self, blocker: String) -> Self {
        self.outcome = "failed".to_string();
        self.blocker = Some(blocker);
        self
    }
}

pub fn run_system_update(
    paths: &RefineCheckoutPaths,
    options: SystemUpdateOptions,
    progress: &mut dyn FnMut(&str),
) -> SystemUpdateReport {
    let mut report = SystemUpdateReport::new(paths, options.port);

    if let Err(error) = paths.require_same_source_checkout(&paths.checkout) {
        return report.blocked(format!(
            "{error}; for gitless published installations follow the manual steps in docs/runbooks/install.md \"Update Refine\""
        ));
    }
    if !paths.binary.is_file() {
        return report.blocked(format!(
            "deployed binary {} is missing; run `./r system build` first (or `./r system install --port {}` for a fresh install)",
            paths.binary.display(),
            options.port
        ));
    }

    match ensure_daemon_running(paths, options.port, progress) {
        Ok(started) => report.daemon_started = started,
        Err(error) => return report.failed(error.to_string()),
    }

    let provider = match resolve_agent_provider(&paths.runtime_root, options.provider.clone()) {
        Ok(provider) => provider,
        Err(error) => return report.failed(error.to_string()),
    };
    let service = FileSourcePromotionService::new(
        paths.checkout.clone(),
        paths.port_runtime_root(options.port),
        options.port,
    );

    let mut rescue_budget: usize = if options.rescue { 1 } else { 0 };
    loop {
        progress("fetching the configured upstream");
        let snapshot = match service.refresh_cached_check_blocking(UPDATE_CHECK_TIMEOUT) {
            Ok(status) => status.source,
            Err(error) => return report.failed(error.to_string()),
        };
        report.from_commit = Some(snapshot.current_commit.clone());
        report.to_commit = Some(snapshot.available_commit.clone());

        if !snapshot.update_available {
            progress(&format!(
                "already up to date at {}",
                short_commit(&snapshot.current_commit)
            ));
            report.ok = true;
            report.outcome = "already_current".to_string();
            return report;
        }

        if !snapshot.fast_forward {
            let divergence_blocker = format!(
                "the checkout and upstream diverged (local commits are not fast-forwardable); inspect with `git log --oneline @{{upstream}}..HEAD` in {} and push or branch the local commits, then re-run `./r system update`",
                paths.checkout.display()
            );
            if rescue_budget == 0 {
                return report.blocked(divergence_blocker);
            }
            rescue_budget -= 1;
            progress("checkout diverged from upstream; asking the configured agent to resolve it");
            match run_agent_rescue(
                &RescueTrigger::Divergence(&snapshot),
                &provider,
                &paths.checkout,
                &paths.port_runtime_root(options.port),
            ) {
                Ok(output) => {
                    report.rescue = Some(json!({
                        "trigger": "divergence",
                        "provider": provider,
                        "output_tail": tail(&output),
                    }));
                    progress("agent finished; re-checking the checkout");
                    continue;
                }
                Err(error) => {
                    return report
                        .blocked(format!("{divergence_blocker}; agent rescue failed: {error}"));
                }
            }
        }

        progress(&format!(
            "update available: {} -> {}",
            short_commit(&snapshot.current_commit),
            short_commit(&snapshot.available_commit)
        ));
        let operation = match service.queue_agent(&provider) {
            Ok(operation) => operation,
            Err(error) => return report.failed(error.to_string()),
        };
        if operation.status == "running" {
            progress(&format!(
                "an update is already in progress (operation {}); attaching to it",
                operation.id
            ));
        } else {
            progress(&format!("update queued as operation {}", operation.id));
        }
        if let Some(reference) = &operation.stashed_changes {
            progress(&format!(
                "your uncommitted changes were preserved in stash {reference}; they will NOT be reapplied automatically"
            ));
            report.stashed_changes = Some(reference.clone());
        }

        let terminal = match service.wait_for_terminal(&operation.id, UPDATE_WAIT_TIMEOUT, &mut |
            observed: &SourcePromotionOperation,
        | {
            progress(&format!("[{}] {}", observed.stage, observed.message));
        }) {
            Ok(terminal) => terminal,
            Err(RefineError::Degraded(message)) => {
                // The operation is durable and continues in the detached
                // helper; cancellation is fenced around the binary-swap
                // window, so a slow update is left running, never killed.
                report.outcome = "timed_out".to_string();
                report.blocker = Some(message);
                report.operation = service
                    .reconcile_interrupted_agent()
                    .ok()
                    .flatten()
                    .or(report.operation);
                return report;
            }
            Err(error) => return report.failed(error.to_string()),
        };
        report.operation = Some(terminal.clone());
        if terminal.stashed_changes.is_some() {
            report.stashed_changes = terminal.stashed_changes.clone();
        }

        match terminal.status.as_str() {
            "succeeded" => {
                progress("update complete; Refine restarted and verified");
                report.ok = true;
                report.outcome = "updated".to_string();
                return report;
            }
            "cancelled" => {
                return report.failed("the update was cancelled".to_string());
            }
            _ => {
                let failure = terminal
                    .error
                    .clone()
                    .unwrap_or_else(|| terminal.message.clone());
                if rescue_budget == 0 {
                    report.outcome = "failed".to_string();
                    report.blocker = Some(failure);
                    return report;
                }
                rescue_budget -= 1;
                progress(&format!(
                    "update {} during {}; asking the configured agent to diagnose and clear it",
                    terminal.status, terminal.stage
                ));
                match run_agent_rescue(
                    &RescueTrigger::TerminalFailure(&terminal),
                    &provider,
                    &paths.checkout,
                    &paths.port_runtime_root(options.port),
                ) {
                    Ok(output) => {
                        report.rescue = Some(json!({
                            "trigger": "terminal_failure",
                            "provider": provider,
                            "failed_operation": terminal.id,
                            "output_tail": tail(&output),
                        }));
                        progress("agent finished; retrying the update once");
                        continue;
                    }
                    Err(error) => {
                        report.outcome = "failed".to_string();
                        report.blocker = Some(format!("{failure}; agent rescue failed: {error}"));
                        return report;
                    }
                }
            }
        }
    }
}

/// Probe the daemon and start it through the installed service manager when it
/// is down: the promotion engine requires a live daemon to hand off from, and
/// the end state of an update is a running, verified daemon either way.
fn ensure_daemon_running(
    paths: &RefineCheckoutPaths,
    port: u16,
    progress: &mut dyn FnMut(&str),
) -> RefineResult<bool> {
    if http_probe(port).is_ok() {
        return Ok(false);
    }
    progress(&format!(
        "Refine daemon on port {port} is not running; starting it"
    ));
    let lifecycle = FileHostDaemonLifecycleService::new(
        RuntimeRoot {
            root: paths.runtime_root.clone(),
        },
        env!("CARGO_PKG_VERSION"),
    );
    let start_result = execute_daemon_lifecycle(
        &lifecycle,
        DaemonLifecycleAction::Start,
        BackgroundDaemonConfig {
            port,
            ..Default::default()
        },
    );
    let deadline = std::time::Instant::now() + DAEMON_START_TIMEOUT;
    loop {
        if http_probe(port).is_ok() {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Err(RefineError::Degraded(match start_result {
                Ok(status) => format!(
                    "the Refine daemon on port {port} did not become reachable within {}s of starting (reported state: healthy={}, web={})",
                    DAEMON_START_TIMEOUT.as_secs(),
                    status.daemon_healthy,
                    status.web_available
                ),
                Err(error) => format!(
                    "the Refine daemon on port {port} could not be started: {error}"
                ),
            }));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub(crate) enum RescueTrigger<'a> {
    Divergence(&'a SourcePromotionSnapshot),
    TerminalFailure(&'a SourcePromotionOperation),
}

fn run_agent_rescue(
    trigger: &RescueTrigger<'_>,
    provider: &str,
    checkout: &Path,
    port_runtime_root: &Path,
) -> RefineResult<String> {
    attempt_agent_rescue(trigger, provider, checkout, port_runtime_root, |invocation| {
        HostAgentProviderService::with_runtime_root(port_runtime_root).invoke(invocation)
    })
}

/// Write a durable rescue context and invoke the configured provider once to
/// clear a blocker that deterministic logic cannot. The invocation is bounded
/// by the caller's single-shot rescue budget.
pub(crate) fn attempt_agent_rescue(
    trigger: &RescueTrigger<'_>,
    provider: &str,
    checkout: &Path,
    port_runtime_root: &Path,
    invoke: impl FnOnce(ProviderInvocation) -> RefineResult<String>,
) -> RefineResult<String> {
    let (kind, context) = match trigger {
        RescueTrigger::Divergence(snapshot) => (
            "divergence",
            json!({
                "kind": "divergence",
                "source": snapshot,
                "guidance": "The checkout has local commits that are not fast-forwardable from the configured upstream.",
            }),
        ),
        RescueTrigger::TerminalFailure(operation) => (
            "terminal_failure",
            json!({
                "kind": "terminal_failure",
                "operation": operation,
            }),
        ),
    };
    std::fs::create_dir_all(port_runtime_root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create rescue context directory {}: {error}",
            port_runtime_root.display()
        ))
    })?;
    let timestamp = Utc::now()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        .replace(':', "-");
    let context_path = port_runtime_root.join(format!("update-rescue-context-{timestamp}.json"));
    std::fs::write(
        &context_path,
        serde_json::to_string_pretty(&context).unwrap_or_default(),
    )
    .map_err(|error| {
        RefineError::Io(format!(
            "failed to write rescue context {}: {error}",
            context_path.display()
        ))
    })?;
    let prompt = rescue_prompt(kind, &context_path, checkout);
    let mut metadata = serde_json::Map::new();
    metadata.insert("kind".to_string(), json!("system_update_rescue"));
    metadata.insert("trigger".to_string(), json!(kind));
    invoke(ProviderInvocation {
        provider: provider.to_string(),
        prompt,
        session_id: None,
        cwd: Some(checkout.display().to_string()),
        process_metadata: metadata,
    })
}

fn rescue_prompt(kind: &str, context_path: &Path, checkout: &Path) -> String {
    format!(
        "Refine's `./r system update` hit a {kind} blocker it cannot clear deterministically. \
         Read the JSON context at {context}. Diagnose and clear the exact blocker so the update \
         succeeds when the caller re-runs it. Constraints: never discard or reset user work — \
         git stashes and local commits included. For upstream divergence, make the checkout at \
         {checkout} fast-forwardable, for example by pushing local commits to the configured \
         upstream or moving them to a branch. You may inspect state with git, `./r system \
         source-status`, and `./r system source-upgrade-capability --action inspect`. Do NOT run \
         `./r system update` yourself, and do not start, stop, or restart Refine — the caller \
         manages the update lifecycle. When you finish, report the exact fix you applied or the \
         precise remaining blocker.",
        context = context_path.display(),
        checkout = checkout.display(),
    )
}

fn short_commit(commit: &str) -> &str {
    commit.get(..commit.len().min(12)).unwrap_or(commit)
}

fn tail(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.chars().count() <= RESCUE_OUTPUT_TAIL_CHARS {
        return trimmed.to_string();
    }
    let skip = trimmed.chars().count() - RESCUE_OUTPUT_TAIL_CHARS;
    trimmed.chars().skip(skip).collect()
}

#[cfg(test)]
mod tests;

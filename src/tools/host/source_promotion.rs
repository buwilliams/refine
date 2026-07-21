use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::process::subprocess::{FileProcessSupervisor, ProcessOwner};
use crate::process::supervisor::errors::{RefineError, RefineResult};
use crate::process::supervisor::lifecycle::{
    DaemonLifecycleService, FileDaemonLifecycleService, http_probe,
};
use crate::process::supervisor::runtime::RuntimeRoot;

pub const SOURCE_PROMOTION_STATE_FILE: &str = "source-promotion.json";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionSnapshot {
    pub checkout_path: String,
    pub current_commit: String,
    pub remote: String,
    pub local_branch: String,
    pub branch: String,
    pub available_commit: String,
    pub clean: bool,
    pub fast_forward: bool,
    pub update_available: bool,
    #[serde(default)]
    pub active_work: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<SourcePromotionOperation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourcePromotionOperation {
    pub id: String,
    pub status: String,
    pub stage: String,
    pub message: String,
    pub checkout_path: String,
    pub from_commit: String,
    pub to_commit: String,
    pub started_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub rollback_attempted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_succeeded: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

impl SourcePromotionOperation {
    fn queued(snapshot: &SourcePromotionSnapshot) -> Self {
        let now = now_timestamp();
        Self {
            id: format!("source-{}", Uuid::new_v4()),
            status: "queued".to_string(),
            stage: "queued".to_string(),
            message: "Source promotion queued".to_string(),
            checkout_path: snapshot.checkout_path.clone(),
            from_commit: snapshot.current_commit.clone(),
            to_commit: snapshot.available_commit.clone(),
            started_at: now.clone(),
            updated_at: now,
            error: None,
            rollback_attempted: false,
            rollback_succeeded: None,
            recovery: None,
        }
    }
}

pub trait SourcePromotionHost {
    fn build_candidate(&mut self, commit: &str) -> RefineResult<PathBuf>;
    fn verify_preconditions(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()>;
    fn stop_daemon(&mut self) -> RefineResult<()>;
    fn activate(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()>;
    fn restart_daemon(&mut self, executable: &Path) -> RefineResult<()>;
    fn verify_daemon(&mut self, expected_commit: &str) -> RefineResult<()>;
    fn rollback(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()>;
    fn restart_previous_daemon(&mut self) -> RefineResult<()>;
}

#[derive(Clone, Debug)]
pub struct FileSourcePromotionService {
    pub checkout_path: PathBuf,
    pub port_runtime_root: PathBuf,
    pub port: u16,
}

impl FileSourcePromotionService {
    pub fn new(
        checkout_path: impl Into<PathBuf>,
        port_runtime_root: impl Into<PathBuf>,
        port: u16,
    ) -> Self {
        Self {
            checkout_path: checkout_path.into(),
            port_runtime_root: port_runtime_root.into(),
            port,
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.port_runtime_root.join(SOURCE_PROMOTION_STATE_FILE)
    }

    pub fn inspect(&self, fetch: bool) -> RefineResult<SourcePromotionSnapshot> {
        ensure_checkout(&self.checkout_path)?;
        let local_branch = git_text(&self.checkout_path, &["symbolic-ref", "--short", "HEAD"])?;
        let current_commit = git_text(&self.checkout_path, &["rev-parse", "HEAD"])?;
        let remote = git_optional_text(
            &self.checkout_path,
            &["config", "--get", &format!("branch.{local_branch}.remote")],
        )?
        .filter(|value| value != ".")
        .unwrap_or_else(|| "origin".to_string());
        let merge_ref = git_optional_text(
            &self.checkout_path,
            &["config", "--get", &format!("branch.{local_branch}.merge")],
        )?
        .unwrap_or_else(|| format!("refs/heads/{local_branch}"));
        let remote_branch = merge_ref
            .strip_prefix("refs/heads/")
            .unwrap_or(&local_branch)
            .to_string();
        if fetch {
            git_ok(&self.checkout_path, &["fetch", "--prune", &remote])?;
        }
        let available_ref = format!("{remote}/{remote_branch}");
        let available_commit = git_text(&self.checkout_path, &["rev-parse", &available_ref])?;
        let clean = git_text(&self.checkout_path, &["status", "--porcelain"])?.is_empty();
        let fast_forward = git_status(
            &self.checkout_path,
            &[
                "merge-base",
                "--is-ancestor",
                &current_commit,
                &available_commit,
            ],
        )?;
        let active_work = self.active_work()?;
        Ok(SourcePromotionSnapshot {
            checkout_path: self.checkout_path.display().to_string(),
            update_available: current_commit != available_commit,
            current_commit,
            remote,
            local_branch,
            branch: remote_branch,
            available_commit,
            clean,
            fast_forward,
            active_work,
            operation: self.load_operation()?,
        })
    }

    pub fn check(&self) -> RefineResult<SourcePromotionSnapshot> {
        self.inspect(true)
    }

    pub fn queue(&self) -> RefineResult<SourcePromotionOperation> {
        http_probe(self.port).map_err(|_| {
            RefineError::Conflict(format!(
                "source promotion requires a healthy running Refine daemon on port {}",
                self.port
            ))
        })?;
        let snapshot = self.check()?;
        validate_promotion(&snapshot)?;
        let operation = SourcePromotionOperation::queued(&snapshot);
        self.save_operation(&operation)?;
        let executable = std::env::current_exe().map_err(|error| {
            RefineError::Io(format!(
                "failed to locate source-promotion helper executable: {error}"
            ))
        })?;
        let mut command = Command::new(&executable);
        command
            .args([
                "system",
                "source-promote-helper",
                "--checkout",
                &snapshot.checkout_path,
                "--port-runtime-root",
                &self.port_runtime_root.display().to_string(),
                "--port",
                &self.port.to_string(),
                "--operation-id",
                &operation.id,
            ])
            .current_dir(&self.checkout_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Err(error) = command.spawn() {
            let error = RefineError::Io(format!(
                "failed to launch restart-safe source-promotion helper {}: {error}",
                executable.display()
            ));
            let mut failed = operation.clone();
            failed.status = "failed".to_string();
            failed.stage = "launch_helper".to_string();
            failed.message = "Source promotion helper could not start".to_string();
            failed.error = Some(error.to_string());
            failed.recovery = Some(
                "No checkout or daemon changes were made; resolve the launch failure and retry"
                    .to_string(),
            );
            failed.updated_at = now_timestamp();
            self.save_operation(&failed)?;
            return Err(error);
        }
        Ok(operation)
    }

    pub fn run_helper(&self, operation_id: &str) -> RefineResult<SourcePromotionOperation> {
        let mut operation = self.load_operation()?.ok_or_else(|| {
            RefineError::NotFound("source-promotion operation state was not found".to_string())
        })?;
        if operation.id != operation_id {
            return Err(RefineError::Conflict(format!(
                "source-promotion operation {} is no longer current",
                operation_id
            )));
        }
        // Allow the initiating HTTP response to leave the daemon before the
        // helper marks it unhealthy and waits for shutdown.
        thread::sleep(Duration::from_millis(750));
        let mut snapshot = match self.check() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                operation.status = "failed".to_string();
                operation.stage = "preflight".to_string();
                operation.message = "Source promotion failed during preflight".to_string();
                operation.error = Some(error.to_string());
                operation.recovery = Some(
                    "Check remote connectivity and source state, then check again; no checkout or daemon changes were made"
                        .to_string(),
                );
                operation.updated_at = now_timestamp();
                self.save_operation(&operation)?;
                return Err(error);
            }
        };
        snapshot.active_work.retain(|item| {
            item != &format!("source promotion {} is {}", operation.id, operation.status)
        });
        if snapshot.current_commit != operation.from_commit
            || snapshot.available_commit != operation.to_commit
        {
            let error = RefineError::Conflict(
                "source commits changed after promotion was queued; check again before retrying"
                    .to_string(),
            );
            operation.status = "failed".to_string();
            operation.stage = "preflight".to_string();
            operation.message = "Source promotion failed during preflight".to_string();
            operation.error = Some(error.to_string());
            operation.recovery = Some(
                "Check for source updates again; no checkout or daemon changes were made"
                    .to_string(),
            );
            operation.updated_at = now_timestamp();
            self.save_operation(&operation)?;
            return Err(error);
        }
        if let Err(error) = validate_promotion(&snapshot) {
            operation.status = "failed".to_string();
            operation.stage = "preflight".to_string();
            operation.message = "Source promotion failed during preflight".to_string();
            operation.error = Some(error.to_string());
            operation.recovery = Some(
                "Resolve the preflight condition and check for source updates again; no checkout or daemon changes were made"
                    .to_string(),
            );
            operation.updated_at = now_timestamp();
            self.save_operation(&operation)?;
            return Err(error);
        }
        let mut host = FileSourcePromotionHost::new(self.clone());
        run_source_promotion(&mut host, &mut operation, |operation| {
            self.save_operation(operation)
        })?;
        Ok(operation)
    }

    pub fn load_operation(&self) -> RefineResult<Option<SourcePromotionOperation>> {
        match fs::read(self.state_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
                RefineError::Serialization(format!(
                    "failed to parse source-promotion state {}: {error}",
                    self.state_path().display()
                ))
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(RefineError::Io(format!(
                "failed to read source-promotion state {}: {error}",
                self.state_path().display()
            ))),
        }
    }

    fn save_operation(&self, operation: &SourcePromotionOperation) -> RefineResult<()> {
        fs::create_dir_all(&self.port_runtime_root).map_err(|error| {
            RefineError::Io(format!(
                "failed to create port runtime root {}: {error}",
                self.port_runtime_root.display()
            ))
        })?;
        let encoded = serde_json::to_vec_pretty(operation).map_err(|error| {
            RefineError::Serialization(format!("failed to encode source-promotion state: {error}"))
        })?;
        let pending = self.state_path().with_extension("json.pending");
        fs::write(&pending, encoded).map_err(|error| {
            RefineError::Io(format!(
                "failed to write source-promotion state {}: {error}",
                pending.display()
            ))
        })?;
        fs::rename(&pending, self.state_path()).map_err(|error| {
            RefineError::Io(format!(
                "failed to publish source-promotion state {}: {error}",
                self.state_path().display()
            ))
        })
    }

    fn active_work(&self) -> RefineResult<Vec<String>> {
        let mut active = Vec::new();
        let workflow_path = self
            .port_runtime_root
            .join("workflow-automation-state.json");
        if workflow_path.is_file() {
            let value: Value =
                serde_json::from_slice(&fs::read(&workflow_path).map_err(|error| {
                    RefineError::Io(format!(
                        "failed to read {}: {error}",
                        workflow_path.display()
                    ))
                })?)
                .map_err(|error| {
                    RefineError::Serialization(format!(
                        "failed to parse {}: {error}",
                        workflow_path.display()
                    ))
                })?;
            if let Some(claims) = value.get("claims").and_then(Value::as_array) {
                for claim in claims {
                    let state = claim.get("state").and_then(Value::as_str).unwrap_or("");
                    if matches!(state, "claimed" | "running") {
                        let goal = claim
                            .get("goal_id")
                            .or_else(|| claim.get("gap_id"))
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        active.push(format!("active Goal claim {goal}"));
                    }
                }
            }
        }
        let supervisor = FileProcessSupervisor::new(&self.port_runtime_root);
        let pause_state = supervisor.pause_state()?;
        if !pause_state.agents_paused || !pause_state.background_processes_stopped {
            active.push("workflow automation is not paused".to_string());
        }
        for process in supervisor.list()? {
            if process.state == "running" && process.owner != ProcessOwner::Daemon {
                active.push(format!(
                    "running {} process {}",
                    process.owner.as_kind(),
                    process.id
                ));
            }
        }
        if let Some(operation) = self.load_operation()?
            && matches!(operation.status.as_str(), "queued" | "running")
        {
            active.push(format!(
                "source promotion {} is {}",
                operation.id, operation.status
            ));
        }
        active.sort();
        active.dedup();
        Ok(active)
    }
}

pub fn validate_promotion(snapshot: &SourcePromotionSnapshot) -> RefineResult<()> {
    if !snapshot.clean {
        return Err(RefineError::Conflict(
            "source promotion requires a clean controller checkout; dirty work was left untouched"
                .to_string(),
        ));
    }
    if !snapshot.fast_forward {
        return Err(RefineError::Conflict(
            "source promotion requires fast-forward-only ancestry; the checkout and remote diverged"
                .to_string(),
        ));
    }
    if !snapshot.update_available {
        return Err(RefineError::Conflict(
            "the running checkout is already at the latest fetched source commit".to_string(),
        ));
    }
    if !snapshot.active_work.is_empty() {
        return Err(RefineError::Conflict(format!(
            "source promotion requires an idle Refine runtime: {}",
            snapshot.active_work.join(", ")
        )));
    }
    Ok(())
}

pub fn run_source_promotion<H, F>(
    host: &mut H,
    operation: &mut SourcePromotionOperation,
    mut persist: F,
) -> RefineResult<()>
where
    H: SourcePromotionHost,
    F: FnMut(&SourcePromotionOperation) -> RefineResult<()>,
{
    update_operation(
        operation,
        "running",
        "build_candidate",
        "Building the fetched source candidate before activation",
    );
    persist(operation)?;
    let candidate = match host.build_candidate(&operation.to_commit) {
        Ok(candidate) => candidate,
        Err(error) => return fail_operation(operation, "build_candidate", error, &mut persist),
    };

    update_operation(
        operation,
        "running",
        "verify_idle",
        "Candidate built; rechecking checkout safety and runtime quiescence",
    );
    persist(operation)?;
    if let Err(error) = host.verify_preconditions(&operation.from_commit, &operation.to_commit) {
        return fail_operation(operation, "verify_idle", error, &mut persist);
    }

    update_operation(
        operation,
        "running",
        "stop_daemon",
        "Candidate built; stopping the Refine daemon",
    );
    persist(operation)?;
    if let Err(error) = host.stop_daemon() {
        return fail_operation(operation, "stop_daemon", error, &mut persist);
    }

    update_operation(
        operation,
        "running",
        "activate_source",
        "Daemon stopped; activating the fast-forward source commit",
    );
    persist(operation)?;
    if let Err(error) = host.activate(&operation.from_commit, &operation.to_commit) {
        let _ = host.restart_previous_daemon();
        return fail_operation(operation, "activate_source", error, &mut persist);
    }

    update_operation(
        operation,
        "running",
        "restart_daemon",
        "Source activated; restarting Refine from the candidate binary",
    );
    persist(operation)?;
    let restart_result = host
        .restart_daemon(&candidate)
        .and_then(|_| host.verify_daemon(&operation.to_commit));
    if let Err(error) = restart_result {
        operation.rollback_attempted = true;
        let rollback = host
            .rollback(&operation.from_commit, &operation.to_commit)
            .and_then(|_| host.restart_previous_daemon());
        operation.rollback_succeeded = Some(rollback.is_ok());
        operation.recovery = Some(if rollback.is_ok() {
            format!(
                "Refine was restored to {}; inspect the restart failure before retrying",
                operation.from_commit
            )
        } else {
            format!(
                "Automatic rollback failed; from {} restore ref {} to {} and run `./r system start --port <port>`",
                operation.checkout_path, operation.from_commit, operation.from_commit
            )
        });
        return fail_operation(operation, "restart_daemon", error, &mut persist);
    }

    update_operation(
        operation,
        "succeeded",
        "complete",
        "Latest source promoted and Refine is healthy",
    );
    operation.recovery = None;
    persist(operation)
}

fn fail_operation<F>(
    operation: &mut SourcePromotionOperation,
    stage: &str,
    error: RefineError,
    persist: &mut F,
) -> RefineResult<()>
where
    F: FnMut(&SourcePromotionOperation) -> RefineResult<()>,
{
    operation.status = "failed".to_string();
    operation.stage = stage.to_string();
    operation.message = format!("Source promotion failed during {stage}");
    operation.error = Some(error.to_string());
    operation.updated_at = now_timestamp();
    if operation.recovery.is_none() {
        operation.recovery = Some(
            "Resolve the reported stage failure, then check for source updates again".to_string(),
        );
    }
    persist(operation)?;
    Err(error)
}

fn update_operation(
    operation: &mut SourcePromotionOperation,
    status: &str,
    stage: &str,
    message: &str,
) {
    operation.status = status.to_string();
    operation.stage = stage.to_string();
    operation.message = message.to_string();
    operation.error = None;
    operation.updated_at = now_timestamp();
}

#[derive(Clone, Debug)]
struct FileSourcePromotionHost {
    service: FileSourcePromotionService,
    previous_executable: PathBuf,
}

impl FileSourcePromotionHost {
    fn new(service: FileSourcePromotionService) -> Self {
        let previous_executable =
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("refine"));
        Self {
            service,
            previous_executable,
        }
    }

    fn runtime_root(&self) -> RefineResult<PathBuf> {
        self.service
            .port_runtime_root
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| RefineError::InvalidInput("port runtime root has no parent".to_string()))
    }

    fn launch(&self, executable: &Path) -> RefineResult<()> {
        let runtime_root = self.runtime_root()?;
        let output = Command::new(executable)
            .args([
                "system",
                "start",
                "--port",
                &self.service.port.to_string(),
                "--runtime-root",
                &runtime_root.display().to_string(),
            ])
            .current_dir(&self.service.checkout_path)
            .output()
            .map_err(|error| {
                RefineError::Io(format!(
                    "failed to launch Refine from {}: {error}",
                    executable.display()
                ))
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RefineError::Degraded(format!(
                "Refine restart failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}

impl SourcePromotionHost for FileSourcePromotionHost {
    fn build_candidate(&mut self, commit: &str) -> RefineResult<PathBuf> {
        let root = self.service.port_runtime_root.join("source-promotion");
        let worktree = root.join(format!("candidate-{}", &commit[..commit.len().min(12)]));
        let binary = root.join(format!("refine-{}", &commit[..commit.len().min(12)]));
        fs::create_dir_all(&root).map_err(|error| {
            RefineError::Io(format!("failed to create {}: {error}", root.display()))
        })?;
        if worktree.exists() {
            return Err(RefineError::Conflict(format!(
                "candidate worktree already exists: {}",
                worktree.display()
            )));
        }
        git_ok(
            &self.service.checkout_path,
            &[
                "worktree",
                "add",
                "--detach",
                &worktree.display().to_string(),
                commit,
            ],
        )?;
        let build = Command::new("cargo")
            .args(["build", "--release", "--locked"])
            .current_dir(&worktree)
            .output()
            .map_err(|error| {
                RefineError::Io(format!("failed to launch candidate build: {error}"))
            })?;
        let built = worktree.join("target/release/refine");
        let candidate_result = if build.status.success() {
            fs::copy(&built, &binary).map(|_| ()).map_err(|error| {
                RefineError::Io(format!(
                    "failed to preserve candidate binary {} as {}: {error}",
                    built.display(),
                    binary.display()
                ))
            })
        } else {
            Err(RefineError::Degraded(format!(
                "candidate build failed with status {}: {}",
                build.status,
                String::from_utf8_lossy(&build.stderr).trim()
            )))
        };
        let target_dir = worktree.join("target");
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir).map_err(|error| {
                RefineError::Io(format!(
                    "failed to clean generated candidate build directory {}: {error}",
                    target_dir.display()
                ))
            })?;
        }
        git_ok(
            &self.service.checkout_path,
            &["worktree", "remove", &worktree.display().to_string()],
        )?;
        candidate_result?;
        Ok(binary)
    }

    fn verify_preconditions(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()> {
        let mut snapshot = self.service.inspect(false)?;
        snapshot
            .active_work
            .retain(|item| !item.starts_with("source promotion "));
        if snapshot.current_commit != from_commit || snapshot.available_commit != to_commit {
            return Err(RefineError::Conflict(
                "source commits changed while the candidate was building; activation was aborted"
                    .to_string(),
            ));
        }
        validate_promotion(&snapshot)
    }

    fn stop_daemon(&mut self) -> RefineResult<()> {
        let runtime_root = self.runtime_root()?;
        FileDaemonLifecycleService::new(RuntimeRoot { root: runtime_root })
            .stop(self.service.port)?;
        for _ in 0..50 {
            if http_probe(self.service.port).is_err() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(RefineError::Degraded(format!(
            "Refine daemon on port {} did not stop",
            self.service.port
        )))
    }

    fn activate(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()> {
        let snapshot = self.service.inspect(false)?;
        if !snapshot.clean || snapshot.current_commit != from_commit {
            return Err(RefineError::Conflict(
                "controller checkout changed after candidate build; source activation was aborted"
                    .to_string(),
            ));
        }
        if !git_status(
            &self.service.checkout_path,
            &["merge-base", "--is-ancestor", from_commit, to_commit],
        )? {
            return Err(RefineError::Conflict(
                "fetched source is no longer a fast-forward of the controller checkout".to_string(),
            ));
        }
        git_ok(&self.service.checkout_path, &["read-tree", "-u", to_commit])?;
        let reference = format!("refs/heads/{}", snapshot.local_branch);
        if let Err(error) = git_ok(
            &self.service.checkout_path,
            &["update-ref", &reference, to_commit, from_commit],
        ) {
            let _ = git_ok(
                &self.service.checkout_path,
                &["read-tree", "-u", from_commit],
            );
            return Err(error);
        }
        Ok(())
    }

    fn restart_daemon(&mut self, executable: &Path) -> RefineResult<()> {
        self.launch(executable)
    }

    fn verify_daemon(&mut self, expected_commit: &str) -> RefineResult<()> {
        http_probe(self.service.port)?;
        let actual = git_text(&self.service.checkout_path, &["rev-parse", "HEAD"])?;
        if actual == expected_commit {
            Ok(())
        } else {
            Err(RefineError::Degraded(format!(
                "daemon restarted but checkout commit is {actual}, expected {expected_commit}"
            )))
        }
    }

    fn rollback(&mut self, from_commit: &str, to_commit: &str) -> RefineResult<()> {
        git_ok(
            &self.service.checkout_path,
            &["read-tree", "-u", from_commit],
        )?;
        let branch = git_text(
            &self.service.checkout_path,
            &["symbolic-ref", "--short", "HEAD"],
        )?;
        git_ok(
            &self.service.checkout_path,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                from_commit,
                to_commit,
            ],
        )
    }

    fn restart_previous_daemon(&mut self) -> RefineResult<()> {
        let executable = self.previous_executable.clone();
        self.launch(&executable)
    }
}

fn ensure_checkout(checkout: &Path) -> RefineResult<()> {
    if checkout.join(".git").exists() && checkout.join("Cargo.toml").is_file() {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(format!(
            "source promotion requires a Refine Git checkout: {}",
            checkout.display()
        )))
    }
}

fn git_text(checkout: &Path, args: &[&str]) -> RefineResult<String> {
    let output = git_output(checkout, args)?;
    if !output.status.success() {
        return Err(git_failure(args, &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_optional_text(checkout: &Path, args: &[&str]) -> RefineResult<Option<String>> {
    let output = git_output(checkout, args)?;
    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!value.is_empty()).then_some(value))
    } else if output.status.code() == Some(1) {
        Ok(None)
    } else {
        Err(git_failure(args, &output))
    }
}

fn git_ok(checkout: &Path, args: &[&str]) -> RefineResult<()> {
    let output = git_output(checkout, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(git_failure(args, &output))
    }
}

fn git_status(checkout: &Path, args: &[&str]) -> RefineResult<bool> {
    let output = git_output(checkout, args)?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(git_failure(args, &output)),
    }
}

fn git_output(checkout: &Path, args: &[&str]) -> RefineResult<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(checkout)
        .output()
        .map_err(|error| RefineError::Io(format!("failed to run git {}: {error}", args.join(" "))))
}

fn git_failure(args: &[&str], output: &std::process::Output) -> RefineError {
    RefineError::Conflict(format!(
        "git {} failed with status {}: {}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeHost {
        fail: Option<&'static str>,
        calls: Vec<String>,
    }

    impl FakeHost {
        fn call(&mut self, stage: &str) -> RefineResult<()> {
            self.calls.push(stage.to_string());
            if self.fail == Some(stage) {
                Err(RefineError::Degraded(format!("{stage} failed")))
            } else {
                Ok(())
            }
        }
    }

    impl SourcePromotionHost for FakeHost {
        fn build_candidate(&mut self, _commit: &str) -> RefineResult<PathBuf> {
            self.call("build")?;
            Ok(PathBuf::from("/candidate/refine"))
        }
        fn verify_preconditions(&mut self, _from: &str, _to: &str) -> RefineResult<()> {
            self.call("preflight")
        }
        fn stop_daemon(&mut self) -> RefineResult<()> {
            self.call("stop")
        }
        fn activate(&mut self, _from: &str, _to: &str) -> RefineResult<()> {
            self.call("activate")
        }
        fn restart_daemon(&mut self, _executable: &Path) -> RefineResult<()> {
            self.call("restart")
        }
        fn verify_daemon(&mut self, _expected: &str) -> RefineResult<()> {
            self.call("verify")
        }
        fn rollback(&mut self, _from: &str, _to: &str) -> RefineResult<()> {
            self.call("rollback")
        }
        fn restart_previous_daemon(&mut self) -> RefineResult<()> {
            self.call("restart_previous")
        }
    }

    fn operation() -> SourcePromotionOperation {
        SourcePromotionOperation {
            id: "source-test".to_string(),
            status: "queued".to_string(),
            stage: "queued".to_string(),
            message: String::new(),
            checkout_path: "/refine".to_string(),
            from_commit: "aaa".to_string(),
            to_commit: "bbb".to_string(),
            started_at: now_timestamp(),
            updated_at: now_timestamp(),
            error: None,
            rollback_attempted: false,
            rollback_succeeded: None,
            recovery: None,
        }
    }

    #[test]
    fn source_promotion_builds_before_stopping_and_verifies_restart() {
        let mut host = FakeHost::default();
        let mut operation = operation();
        let mut states = Vec::new();
        run_source_promotion(&mut host, &mut operation, |state| {
            states.push((state.status.clone(), state.stage.clone()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            host.calls,
            [
                "build",
                "preflight",
                "stop",
                "activate",
                "restart",
                "verify"
            ]
        );
        assert_eq!(operation.status, "succeeded");
        assert_eq!(operation.stage, "complete");
        assert_eq!(states.first().unwrap().1, "build_candidate");
    }

    #[test]
    fn source_promotion_build_failure_never_stops_or_activates() {
        let mut host = FakeHost {
            fail: Some("build"),
            ..Default::default()
        };
        let mut operation = operation();
        assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
        assert_eq!(host.calls, ["build"]);
        assert_eq!(operation.stage, "build_candidate");
        assert_eq!(operation.status, "failed");
    }

    #[test]
    fn source_promotion_restart_failure_rolls_back_and_recovers_previous_daemon() {
        let mut host = FakeHost {
            fail: Some("restart"),
            ..Default::default()
        };
        let mut operation = operation();
        assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
        assert_eq!(
            host.calls,
            [
                "build",
                "preflight",
                "stop",
                "activate",
                "restart",
                "rollback",
                "restart_previous"
            ]
        );
        assert_eq!(operation.rollback_succeeded, Some(true));
        assert!(operation.recovery.as_deref().unwrap().contains("restored"));
    }

    #[test]
    fn source_promotion_active_work_after_build_never_stops_or_activates() {
        let mut host = FakeHost {
            fail: Some("preflight"),
            ..Default::default()
        };
        let mut operation = operation();
        assert!(run_source_promotion(&mut host, &mut operation, |_| Ok(())).is_err());
        assert_eq!(host.calls, ["build", "preflight"]);
        assert_eq!(operation.stage, "verify_idle");
    }

    #[test]
    fn validation_rejects_dirty_diverged_active_and_current_snapshots() {
        let base = SourcePromotionSnapshot {
            checkout_path: "/refine".to_string(),
            current_commit: "aaa".to_string(),
            remote: "origin".to_string(),
            local_branch: "main".to_string(),
            branch: "main".to_string(),
            available_commit: "bbb".to_string(),
            clean: true,
            fast_forward: true,
            update_available: true,
            active_work: Vec::new(),
            operation: None,
        };
        let mut dirty = base.clone();
        dirty.clean = false;
        assert!(
            validate_promotion(&dirty)
                .unwrap_err()
                .to_string()
                .contains("clean")
        );
        let mut diverged = base.clone();
        diverged.fast_forward = false;
        assert!(
            validate_promotion(&diverged)
                .unwrap_err()
                .to_string()
                .contains("fast-forward")
        );
        let mut active = base.clone();
        active.active_work.push("active Goal claim G1".to_string());
        assert!(
            validate_promotion(&active)
                .unwrap_err()
                .to_string()
                .contains("idle")
        );
        let mut current = base;
        current.update_available = false;
        assert!(
            validate_promotion(&current)
                .unwrap_err()
                .to_string()
                .contains("already")
        );
    }
}

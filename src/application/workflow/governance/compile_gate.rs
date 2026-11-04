//! Governance compile gate.
//!
//! Runs the operator-configured `governance_compile_command` against a pristine
//! detached checkout of the candidate commit before the Skill-based Governance
//! review runs. A non-zero exit is turned into a `GovernanceEvaluation { failed:
//! true, ... }` by the caller so the existing Failed-goal recovery path (which
//! records a `recovery_round_prompt` and transitions the goal) can surface a
//! build failure the same way it surfaces a Skill finding. No LLM tokens are
//! spent when the tree does not build.
//!
//! Isolation: the build runs in `.git/refine-compile-gate/target`, a
//! refine-owned detached worktree reserved for this gate — deliberately
//! separate from the integration worktree at `.git/refine-integration/target`,
//! because a compile command leaves build artifacts (dist/, target/, bin/)
//! that would defeat the integration worktree's between-integrations
//! persistence. The candidate worktree the user's Implement phase ran in is
//! not touched, so `verify_reviewed_candidate_before_publication`'s working-
//! tree equality check keeps holding.

use std::path::Path;

use serde_json::{Map, Value, json};

use crate::error::RefineResult;
use crate::infrastructure::git::worktrees::FileGitWorktreeService;
use crate::infrastructure::process::subprocess::{
    FileProcessSupervisor, ManagedProcessSpec, ProcessOwner, ProcessResourceLimits,
};
use crate::infrastructure::process::supervisor::coordination::with_record_lock;
use crate::infrastructure::process::supervisor::security::{
    FileSecurityService, parse_allowed_commands,
};
use crate::model::JsonObject;

use super::integration::ensure_compile_gate_worktree;

/// Outcome of the compile gate. The caller maps this onto a GovernanceEvaluation;
/// the gate itself never returns Err for a *build* result — the two failure
/// variants let the caller distinguish a real build failure (actionable: "fix
/// your code") from an infrastructure failure (actionable: "fix your refine
/// config / allowlist / disk"). Authority-loss from the `heartbeat` closure is
/// the one case that surfaces as an `Err` from `run_compile_gate`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CompileGateOutcome {
    /// `governance_compile_command` is empty — gate is opt-in and not configured.
    Skipped,
    /// Build exited 0 against the detached candidate worktree.
    /// `candidate_commit` echoes the commit the gate actually built against,
    /// so an audit entry does not have to re-derive it from caller-side state
    /// that may have changed. `log_bytes` is the size of the *captured* build
    /// output before truncation — audit callers typically log the size only.
    Passed {
        candidate_commit: String,
        log_bytes: usize,
    },
    /// Build ran and exited non-zero. `summary` is a short label; `log_tail`
    /// is a sanitized head+tail excerpt of the untrusted build output, bounded
    /// by `MAX_LOG_TAIL_BYTES`; `original_bytes` is the full captured size
    /// *before* truncation so the caller can honestly report how much was
    /// elided. Callers that feed `log_tail` to an LLM MUST fence it.
    BuildFailed {
        summary: String,
        log_tail: String,
        original_bytes: usize,
    },
    /// Something outside the build failed — allowlist rejection, worktree prep
    /// failure, spawn error, settings error. The user's code is not necessarily
    /// broken; the operator's configuration or the host is. Caller should NOT
    /// phrase this as "fix your build".
    InfrastructureFailed { summary: String },
}

const MAX_LOG_TAIL_BYTES: usize = 8 * 1024;
/// Global lock key: intentionally shared across all goals on the host. Every
/// compile gate on this runtime root operates against the same worktree path
/// (`.git/refine-compile-gate/target` — see `ensure_compile_gate_worktree`),
/// so concurrent gates from different goals *would* collide on that path.
/// Per-goal isolation would require per-goal worktree paths, which multiplies
/// disk usage (a `target/` or `node_modules/` per in-flight goal). The chosen
/// trade-off: serialize gate runs; accept that one goal's slow build blocks
/// another goal's gate for up to `governance_compile_timeout_seconds`.
const LOCK_KEY: &str = "governance-compile-gate";
const DEFAULT_TIMEOUT_SECONDS: u64 = 600;
const AUDIT_ACTOR: &str = "governance_compile";

/// Run the configured compile command against a pristine detached checkout of
/// `candidate_commit`. See module docs for isolation and failure semantics.
///
/// `heartbeat` is called before the build, on every stdout/stderr chunk, and
/// after the build. It's the caller's mechanism for revalidating the workflow
/// authority (e.g. `ctx.revalidate_authority(GoalStatus::Governance)`); if the
/// caller returns `Err` from one of those calls, that error is surfaced to the
/// caller unchanged — authority loss is not a compile failure.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_compile_gate(
    repo_git: &FileGitWorktreeService,
    runtime_root: &Path,
    settings: &JsonObject,
    goal_id: &str,
    round_idx: usize,
    candidate_commit: &str,
    agent_subpath: &str,
    heartbeat: &dyn Fn() -> RefineResult<()>,
) -> RefineResult<CompileGateOutcome> {
    let command = setting_trimmed(settings, "governance_compile_command");
    if command.is_empty() {
        return Ok(CompileGateOutcome::Skipped);
    }
    let timeout_seconds = setting_positive_u64(
        settings,
        "governance_compile_timeout_seconds",
        DEFAULT_TIMEOUT_SECONDS,
    );
    with_record_lock(runtime_root, LOCK_KEY, || {
        heartbeat()?;
        execute_locked(
            repo_git,
            runtime_root,
            settings,
            goal_id,
            round_idx,
            candidate_commit,
            agent_subpath,
            &command,
            timeout_seconds,
            heartbeat,
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_locked(
    repo_git: &FileGitWorktreeService,
    runtime_root: &Path,
    settings: &JsonObject,
    goal_id: &str,
    round_idx: usize,
    candidate_commit: &str,
    agent_subpath: &str,
    command: &str,
    timeout_seconds: u64,
    heartbeat: &dyn Fn() -> RefineResult<()>,
) -> RefineResult<CompileGateOutcome> {
    // Infrastructure checks first — authorize before paying for the worktree,
    // and never report an auth/prep failure as "the build broke". The one
    // error that escapes as Err is heartbeat() failure, which means workflow
    // authority was lost and the caller must terminate the operation.
    if let Err(error) = authorize(runtime_root, settings, command) {
        return Ok(infrastructure(format!(
            "compile command rejected by security policy: {error}"
        )));
    }
    let worktree = match ensure_compile_gate_worktree(repo_git, runtime_root, candidate_commit) {
        Ok(worktree) => worktree,
        Err(error) => {
            return Ok(infrastructure(format!(
                "could not prepare build worktree: {error}"
            )));
        }
    };
    // Reclaim the worktree when we're done — a large build (Rust `target/`,
    // node_modules) can occupy multiple GB, and the shared path otherwise
    // persists until the next gate run. Scoped with a RAII guard so we
    // purge even if the heartbeat below returns Err.
    let _cleanup = CompileGateWorktreeGuard {
        repo_git,
        path: &worktree.path,
    };
    let cwd = match resolve_cwd(&worktree.path, agent_subpath) {
        Ok(cwd) => cwd,
        Err(error) => return Ok(infrastructure(error)),
    };
    let spec = build_spec(cwd, command, timeout_seconds, goal_id, round_idx, candidate_commit);
    let supervisor = FileProcessSupervisor::new(runtime_root);
    let result = supervisor.run_to_completion_with_output(spec, |_, _| {
        // Tick on every output chunk — authority revocation surfaces quickly.
        // on_output cannot cancel the child, so authority loss is re-checked
        // after the build via the trailing heartbeat() below.
        let _ = heartbeat();
    });
    heartbeat()?;
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            return Ok(infrastructure(format!(
                "compile command failed to launch: {error}"
            )));
        }
    };
    let original_bytes = output.stdout.len() + output.stderr.len();
    if output.success() {
        Ok(CompileGateOutcome::Passed {
            candidate_commit: candidate_commit.to_string(),
            log_bytes: original_bytes,
        })
    } else {
        let log_tail = combine_log_tail(&output.stdout, &output.stderr);
        let exit = output
            .process
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signaled".into());
        Ok(CompileGateOutcome::BuildFailed {
            summary: format!("Compile command exited {exit}"),
            log_tail,
            original_bytes,
        })
    }
}

fn infrastructure(summary: String) -> CompileGateOutcome {
    CompileGateOutcome::InfrastructureFailed { summary }
}

/// RAII cleanup for the compile-gate worktree. Purges on drop so build
/// artifacts never outlive the gate call, even when the gate returns early
/// (heartbeat error, panic) and the worktree ends up multi-GB.
struct CompileGateWorktreeGuard<'a> {
    repo_git: &'a FileGitWorktreeService,
    path: &'a std::path::Path,
}

impl Drop for CompileGateWorktreeGuard<'_> {
    fn drop(&mut self) {
        // Best effort — a purge failure here is diagnostic noise at most; the
        // next run will re-purge before it starts. Printing once is kinder
        // than silently swallowing.
        if let Err(error) = self.repo_git.purge_worktree(self.path) {
            eprintln!(
                "refine: compile-gate worktree purge failed at {}: {error}",
                self.path.display()
            );
        }
    }
}

fn resolve_cwd(worktree_root: &Path, agent_subpath: &str) -> Result<std::path::PathBuf, String> {
    let trimmed = agent_subpath.trim();
    if trimmed.is_empty() {
        return Ok(worktree_root.to_path_buf());
    }
    let relative = std::path::Path::new(trimmed);
    if relative.is_absolute() {
        return Err(format!(
            "agent_subpath must be relative to the worktree root; got absolute path {trimmed}"
        ));
    }
    if relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(format!(
            "agent_subpath must not contain .. components; got {trimmed}"
        ));
    }
    Ok(worktree_root.join(relative))
}

fn authorize(
    runtime_root: &Path,
    settings: &JsonObject,
    command: &str,
) -> RefineResult<()> {
    // Use the in-memory settings the caller handed us rather than re-reading
    // from disk: the two should normally match, but the caller already loaded
    // settings once and this keeps the authorization decision consistent with
    // the same snapshot used to pick the command and timeout. Reuse the
    // security module's canonical grammar parser so a future allowed_commands
    // grammar change (new separator, quoting, comments) flows here for free.
    let allowed = settings
        .get("allowed_commands")
        .and_then(Value::as_str)
        .map(parse_allowed_commands)
        .unwrap_or_default();
    FileSecurityService::with_allowed_commands(runtime_root, allowed)
        .authorize_host_command(AUDIT_ACTOR, command)
}

fn build_spec(
    cwd: std::path::PathBuf,
    command: &str,
    timeout_seconds: u64,
    goal_id: &str,
    round_idx: usize,
    candidate_commit: &str,
) -> ManagedProcessSpec {
    // Fresh, minimal metadata. The compile gate is a leaf operation; forwarding
    // workflow metadata (managed_worktree, workflow_revision, etc.) would leak
    // Governance plumbing into the process registry and any downstream prompt.
    // `completion_timeout_seconds` is the wall-clock cap enforced by the
    // subprocess runtime (execution.rs); `stall_timeout_seconds` on `limits`
    // would only cover idle-output stalls — a chatty infinite loop would evade
    // it — so we rely on the hard wall-clock cap here.
    let mut metadata = Map::new();
    metadata.insert("goal_id".into(), json!(goal_id));
    metadata.insert("round_idx".into(), json!(round_idx));
    metadata.insert("candidate_commit".into(), json!(candidate_commit));
    metadata.insert("kind".into(), json!("governance_compile"));
    metadata.insert("completion_timeout_seconds".into(), json!(timeout_seconds));
    let (shell, args) = shell_program_args(command);
    ManagedProcessSpec {
        owner: ProcessOwner::Runner,
        command: shell,
        args,
        cwd: Some(cwd.display().to_string()),
        env: Vec::new(),
        stdin: None,
        limits: Some(ProcessResourceLimits {
            max_memory_bytes: None,
            cpu_priority: None,
            kill_on_parent_exit: true,
            // No stall timeout: a tight idle budget would kill legitimate
            // long-quiet build phases (linker, large optimizer passes), and a
            // stall budget equal to the wall-clock cap is a no-op. Rely on
            // `completion_timeout_seconds` (set via metadata above) as the
            // single hard cap.
            stall_timeout_seconds: None,
        }),
        authorization_command: Some(command.to_string()),
        // Operators embed tokens/secrets in free-form shell strings
        // (DOTNET_NUGET_SIGNATURE_VERIFICATION, ARTIFACTORY_TOKEN, etc.).
        // Default-redact so misconfigured commands don't leak to the process
        // registry — the audit trail still carries goal/round/commit metadata
        // for correlation.
        sensitive: true,
        metadata,
    }
}

fn combine_log_tail(stdout: &str, stderr: &str) -> String {
    let half = MAX_LOG_TAIL_BYTES / 2;
    let mut combined = String::new();
    let cleaned_stdout = strip_control_noise(stdout.trim_end());
    if !cleaned_stdout.is_empty() {
        combined.push_str("stdout:\n");
        combined.push_str(&head_and_tail(&cleaned_stdout, half));
    }
    let cleaned_stderr = strip_control_noise(stderr.trim_end());
    if !cleaned_stderr.is_empty() {
        if !combined.is_empty() {
            combined.push_str("\n\n");
        }
        combined.push_str("stderr:\n");
        combined.push_str(&head_and_tail(&cleaned_stderr, half));
    }
    combined
}

/// Keep both the start of the output (where the actual error line often lives,
/// followed by pages of notes) and the end (where the final diagnostic and
/// process exit summary sit). A pure tail-only strategy loses `error[E0599]`
/// at the top of a 100KB stderr dump to backtrace noise.
fn head_and_tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let head_bytes = max_bytes / 3;
    let tail_bytes = max_bytes.saturating_sub(head_bytes).saturating_sub(64);
    let head_end = align_char_boundary(text, head_bytes, true);
    let tail_start = align_char_boundary(text, text.len().saturating_sub(tail_bytes), false);
    let elided = text.len().saturating_sub(head_end).saturating_sub(text.len() - tail_start);
    format!(
        "{}\n\n... [{} bytes elided] ...\n\n{}",
        &text[..head_end],
        elided,
        &text[tail_start..]
    )
}

fn align_char_boundary(text: &str, idx: usize, forward: bool) -> usize {
    let mut i = idx.min(text.len());
    if forward {
        while i < text.len() && !text.is_char_boundary(i) {
            i += 1;
        }
    } else {
        while i > 0 && !text.is_char_boundary(i) {
            i -= 1;
        }
    }
    i
}

/// Strip ANSI escape sequences, standalone carriage returns, and other C0
/// control bytes from build output before truncation. The audit log and any
/// LLM prompt need to contain readable text rather than terminal control
/// bytes, and NUL/BEL/VT/FF would otherwise break JSON audit tooling
/// (NUL is not legal in a JSON string) or inject beeps/formfeeds into the
/// LLM's view of the diagnostic. Keeps TAB, LF, and CRLF — the normal
/// line-structure characters a build log needs.
fn strip_control_noise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1B}' {
            // Skip the escape and its parameters. CSI: ESC '[' <params> <final 0x40..=0x7E>.
            // Bound the inner scan so a malformed/truncated CSI (no terminator)
            // can't swallow the rest of the log and hide the actual error line.
            // Real CSI sequences are tiny — SGR colors are <16 bytes; anything
            // beyond ~32 is almost certainly garbage or a buffer boundary.
            if chars.peek() == Some(&'[') {
                chars.next();
                for _ in 0..64 {
                    match chars.next() {
                        Some(inner) if matches!(inner, '\u{40}'..='\u{7E}') => break,
                        Some(_) => {}
                        None => break,
                    }
                }
            } else {
                // Non-CSI escape — drop the single following byte.
                chars.next();
            }
            continue;
        }
        if ch == '\r' && chars.peek() != Some(&'\n') {
            continue;
        }
        // Drop other C0 controls (0x00..0x1F except TAB/LF/CR handled above)
        // and DEL (0x7F). Preserves ordinary text, UTF-8 multibyte chars, and
        // the three structural whitespace characters.
        if matches!(ch, '\u{00}'..='\u{08}' | '\u{0B}' | '\u{0C}' | '\u{0E}'..='\u{1F}' | '\u{7F}') {
            continue;
        }
        out.push(ch);
    }
    out
}

fn setting_trimmed(settings: &JsonObject, key: &str) -> String {
    settings
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

fn setting_positive_u64(settings: &JsonObject, key: &str, fallback: u64) -> u64 {
    settings
        .get(key)
        .and_then(Value::as_str)
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(fallback)
}

fn shell_program_args(command: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd".to_string(),
            vec!["/C".to_string(), command.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        (
            "sh".to_string(),
            vec!["-c".to_string(), command.to_string()],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RefineError;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct GitFixture {
        temp_root: PathBuf,
        repo: PathBuf,
        runtime_root: PathBuf,
        refine_dir: PathBuf,
        candidate_commit: String,
    }

    impl GitFixture {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let temp_root = std::env::temp_dir().join(format!(
                "refine-compile-gate-{name}-{}-{nanos}",
                std::process::id()
            ));
            let repo = temp_root.join("repo");
            let runtime_root = temp_root.join("run/8080");
            let refine_dir = temp_root.join("state");
            fs::create_dir_all(&repo).unwrap();
            fs::create_dir_all(&runtime_root).unwrap();
            fs::create_dir_all(&refine_dir).unwrap();
            run_git(&repo, &["init", "-b", "main"]);
            run_git(&repo, &["config", "user.email", "test@example.com"]);
            run_git(&repo, &["config", "user.name", "Test User"]);
            fs::write(repo.join("README"), "root\n").unwrap();
            run_git(&repo, &["add", "README"]);
            run_git(&repo, &["commit", "-m", "init"]);
            let candidate_commit = git_stdout(&repo, &["rev-parse", "HEAD"]);
            Self {
                temp_root,
                repo,
                runtime_root,
                refine_dir,
                candidate_commit,
            }
        }

        fn repo_git(&self) -> FileGitWorktreeService {
            FileGitWorktreeService::with_runtime_root(&self.repo, &self.runtime_root)
        }
    }

    impl Drop for GitFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.temp_root);
        }
    }

    fn run_git(repo: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("git");
        assert!(status.success(), "git {} failed", args.join(" "));
    }

    fn git_stdout(repo: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .expect("git");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn settings_with(entries: &[(&str, &str)]) -> JsonObject {
        let mut map = JsonObject::new();
        for (key, value) in entries {
            map.insert((*key).into(), Value::String((*value).into()));
        }
        map
    }

    fn no_heartbeat() -> impl Fn() -> RefineResult<()> {
        || Ok(())
    }

    #[test]
    fn empty_command_is_skipped() {
        let raw = setting_trimmed(&settings_with(&[("governance_compile_command", "   ")]), "governance_compile_command");
        assert!(raw.is_empty());
    }

    #[test]
    fn positive_timeout_is_respected_zero_falls_back() {
        let s = settings_with(&[("governance_compile_timeout_seconds", "900")]);
        assert_eq!(setting_positive_u64(&s, "governance_compile_timeout_seconds", 60), 900);
        let z = settings_with(&[("governance_compile_timeout_seconds", "0")]);
        assert_eq!(setting_positive_u64(&z, "governance_compile_timeout_seconds", 60), 60);
        let empty = settings_with(&[]);
        assert_eq!(setting_positive_u64(&empty, "governance_compile_timeout_seconds", 60), 60);
    }

    #[test]
    fn resolve_cwd_rejects_absolute_subpath() {
        let root = std::path::Path::new("/tmp/wt");
        assert!(resolve_cwd(root, "/etc").is_err());
    }

    #[test]
    fn resolve_cwd_rejects_parent_traversal() {
        let root = std::path::Path::new("/tmp/wt");
        assert!(resolve_cwd(root, "../escape").is_err());
    }

    #[test]
    fn resolve_cwd_joins_relative_subpath() {
        let root = std::path::Path::new("/tmp/wt");
        assert_eq!(resolve_cwd(root, "apps/web").unwrap(), root.join("apps/web"));
    }

    #[test]
    fn resolve_cwd_empty_returns_root() {
        let root = std::path::Path::new("/tmp/wt");
        assert_eq!(resolve_cwd(root, "   ").unwrap(), root.to_path_buf());
    }

    #[test]
    fn combine_log_tail_truncates_long_output() {
        let long = "x".repeat(MAX_LOG_TAIL_BYTES);
        let combined = combine_log_tail(&long, "");
        // Tail label + up to half the budget worth of content; at most one
        // "stdout:\n" prefix.
        assert!(combined.len() <= MAX_LOG_TAIL_BYTES / 2 + 16);
        assert!(combined.starts_with("stdout:\n"));
    }

    #[test]
    fn combine_log_tail_handles_both_streams() {
        let combined = combine_log_tail("compiling...", "warning: something");
        assert!(combined.contains("stdout:"));
        assert!(combined.contains("stderr:"));
        assert!(combined.contains("compiling..."));
        assert!(combined.contains("warning: something"));
    }

    #[test]
    fn head_and_tail_preserves_both_ends_on_long_input() {
        let head = "error[E0599]: no method named foo".repeat(1);
        let noise = "note: backtrace\n".repeat(1000);
        let tail = "process didn't exit successfully".to_string();
        let text = format!("{head}\n{noise}\n{tail}");
        let out = head_and_tail(&text, 2048);
        assert!(out.contains("error[E0599]"), "lost the actual error: {out}");
        assert!(out.contains("didn't exit successfully"), "lost the trailing diagnostic");
        assert!(out.contains("bytes elided"), "no elision marker: {out}");
    }

    #[test]
    fn strip_control_noise_removes_ansi_and_lone_cr() {
        let colored = "\x1b[32mok\x1b[0m: compiled\n\rprogress: 50%\r\x1b[2Kdone";
        let clean = strip_control_noise(colored);
        assert!(!clean.contains('\x1b'), "ANSI not stripped: {clean:?}");
        assert!(!clean.contains('\r'), "lone CR not stripped: {clean:?}");
        assert!(clean.contains("ok"));
        assert!(clean.contains("done"));
    }

    #[test]
    fn strip_control_noise_preserves_crlf_line_endings() {
        let text = "line one\r\nline two\r\n";
        assert_eq!(strip_control_noise(text), text);
    }

    #[test]
    fn strip_control_noise_preserves_utf8() {
        let text = "café → naïve ✓";
        assert_eq!(strip_control_noise(text), text);
    }

    #[test]
    fn run_compile_gate_does_not_touch_the_integration_worktree_path() {
        // Regression guard for the persistence design: the compile gate owns
        // its own worktree path; the integration worktree must remain
        // untouched so the integrate step keeps its between-integration
        // persistence optimization.
        let fx = GitFixture::new("iso-path");
        let settings = settings_with(&[("governance_compile_command", "true")]);
        let _ = run_compile_gate(
            &fx.repo_git(),
            &fx.runtime_root,
            &settings,
            "GOAL1",
            0,
            &fx.candidate_commit,
            "",
            &no_heartbeat(),
        )
        .unwrap();
        // After the gate, both the integration worktree path AND the gate path
        // are absent: integration is never created by the gate, and the gate
        // cleans itself up when done so build artifacts don't accumulate.
        assert!(!fx.repo.join(".git/refine-integration/target").exists());
        assert!(
            !fx.repo.join(".git/refine-compile-gate/target").exists(),
            "gate worktree must be purged after the run to reclaim disk"
        );
    }

    #[test]
    fn run_compile_gate_skips_when_no_command_configured() {
        let fx = GitFixture::new("skip");
        let settings = settings_with(&[]);
        let outcome = run_compile_gate(
            &fx.repo_git(),
            &fx.runtime_root,
            &settings,
            "GOAL1",
            0,
            &fx.candidate_commit,
            "",
            &no_heartbeat(),
        )
        .unwrap();
        assert!(matches!(outcome, CompileGateOutcome::Skipped));
        // No integration worktree was created.
        assert!(!fx.repo.join(".git/refine-compile-gate/target").exists());
    }

    #[test]
    fn run_compile_gate_passes_on_zero_exit() {
        let fx = GitFixture::new("pass");
        let settings = settings_with(&[("governance_compile_command", "true")]);
        let outcome = run_compile_gate(
            &fx.repo_git(),
            &fx.runtime_root,
            &settings,
            "GOAL1",
            0,
            &fx.candidate_commit,
            "",
            &no_heartbeat(),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            CompileGateOutcome::Passed { candidate_commit, log_bytes: _ }
                if candidate_commit == fx.candidate_commit
        ));
    }

    #[test]
    fn run_compile_gate_fails_on_nonzero_exit_with_log_tail() {
        let fx = GitFixture::new("fail");
        let settings = settings_with(&[(
            "governance_compile_command",
            "echo 'building app...'; echo 'E: boom' 1>&2; exit 7",
        )]);
        let outcome = run_compile_gate(
            &fx.repo_git(),
            &fx.runtime_root,
            &settings,
            "GOAL1",
            0,
            &fx.candidate_commit,
            "",
            &no_heartbeat(),
        )
        .unwrap();
        match outcome {
            CompileGateOutcome::BuildFailed {
                summary,
                log_tail,
                original_bytes,
            } => {
                assert!(summary.contains("exited 7"), "summary was {summary:?}");
                assert!(log_tail.contains("building app..."));
                assert!(log_tail.contains("E: boom"));
                assert!(log_tail.contains("stdout:"));
                assert!(log_tail.contains("stderr:"));
                assert!(
                    original_bytes > 0,
                    "original_bytes must report captured size"
                );
            }
            other => panic!("expected BuildFailed, got {other:?}"),
        }
    }

    #[test]
    fn run_compile_gate_auth_rejection_short_circuits_before_worktree_prep() {
        let fx = GitFixture::new("auth");
        // Activate the allowlist with a different command so our target is
        // denied — proves auth runs before ensure_integration_worktree.
        let settings = settings_with(&[
            ("governance_compile_command", "true"),
            ("allowed_commands", "cargo build"),
        ]);
        let outcome = run_compile_gate(
            &fx.repo_git(),
            &fx.runtime_root,
            &settings,
            "GOAL1",
            0,
            &fx.candidate_commit,
            "",
            &no_heartbeat(),
        )
        .unwrap();
        match outcome {
            CompileGateOutcome::InfrastructureFailed { summary } => {
                assert!(
                    summary.contains("security policy"),
                    "summary was {summary:?}"
                );
            }
            other => panic!("expected InfrastructureFailed, got {other:?}"),
        }
        // No worktree should have been created — the auth check runs first.
        assert!(!fx.repo.join(".git/refine-compile-gate/target").exists());
    }

    #[test]
    fn run_compile_gate_runs_in_integration_worktree_not_main_repo() {
        let fx = GitFixture::new("iso");
        // Write a sentinel in the main repo. The command checks for a file that
        // only exists in a pristine detached checkout of candidate_commit (our
        // just-committed README) — if the build ran in the dirty main repo, it
        // would still pass, so also assert it did NOT see a file we add to the
        // main repo AFTER the commit (which the worktree, pinned at
        // candidate_commit, must not have).
        fs::write(fx.repo.join("ONLY_IN_MAIN"), "x\n").unwrap();
        let settings = settings_with(&[(
            "governance_compile_command",
            "test -f README && ! test -f ONLY_IN_MAIN",
        )]);
        let outcome = run_compile_gate(
            &fx.repo_git(),
            &fx.runtime_root,
            &settings,
            "GOAL1",
            0,
            &fx.candidate_commit,
            "",
            &no_heartbeat(),
        )
        .unwrap();
        assert!(
            matches!(outcome, CompileGateOutcome::Passed { .. }),
            "build must see candidate_commit's tree (README) and not the main-repo-only file; outcome={outcome:?}"
        );
    }

    #[test]
    fn run_compile_gate_surfaces_heartbeat_authority_loss() {
        let fx = GitFixture::new("heartbeat");
        let settings = settings_with(&[("governance_compile_command", "true")]);
        let calls = AtomicUsize::new(0);
        let heartbeat = || -> RefineResult<()> {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            // First call is the pre-build tick; fail on the post-build tick to
            // simulate authority revocation during the build.
            if n >= 1 {
                Err(RefineError::Conflict("authority lost".into()))
            } else {
                Ok(())
            }
        };
        let result = run_compile_gate(
            &fx.repo_git(),
            &fx.runtime_root,
            &settings,
            "GOAL1",
            0,
            &fx.candidate_commit,
            "",
            &heartbeat,
        );
        assert!(matches!(result, Err(RefineError::Conflict(_))));
    }
}

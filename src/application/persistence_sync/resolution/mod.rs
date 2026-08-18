//! The agent call-out core for conflicted merges: workspace preparation,
//! bounded resolver attempts, deterministic acceptance gates, and durable
//! publication of the result as a ref. Engine-agnostic — state-file syncs and
//! conflicted rebase workspaces share the same `ConflictResolver` trait.
//!
//! Locking is the caller's job and the agent NEVER runs inside the repository
//! lock. The protocol is two short holds around an unlocked middle:
//!
//! - *Hold A* (seconds): the caller pins the merge operands as
//!   `refs/refine/resolve/<id>/{base,ours,theirs}` and materializes an
//!   isolated, Refine-owned workspace — never the human checkout.
//! - *Unlocked* (minutes): [`resolve_conflict`] runs bounded resolver attempts
//!   with acceptance gates between, then commits the accepted result as
//!   `refs/refine/resolve/<id>/result`.
//! - *Hold B* (seconds): the caller re-verifies the pinned inputs and
//!   publishes via CAS, then retires the resolve refs.
//!
//! Crash-only: every operand is a ref, so a rerun with the same `<id>` finds
//! the pins and either a result ref (skip straight to publish) or re-derives
//! the workspace. Stale pins — the heads changed — are discarded and
//! re-pinned by [`pin_resolution_inputs`]. Nothing here writes a side file.
//!
//! The middle is unlocked, so two operations — the daemon's sync worker and an
//! operator-triggered sync — can reach the same divergence at once. A
//! [`ResolutionLock`] keyed by the resolution id makes one of them the
//! resolver and lets the other defer, so one workspace is never shared and a
//! gated result is never overwritten by a second attempt at the same id.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use fs2::FileExt;
use serde_json::json;

use crate::application::agent_io::prompts::PromptTemplate;
use crate::application::persistence_sync::state::FileGitSyncService;
use crate::error::{RefineError, RefineResult};
use crate::infrastructure::agents::invocation::{
    AgentProviderService, HostAgentProviderService, ProviderInvocation,
};
use crate::infrastructure::git::merge::{TreeOperation, build_tree, commit_tree, write_blob};
use crate::infrastructure::git::worktrees::FileGitWorktreeService;
use crate::infrastructure::process::supervisor::config::{ConfigService, FileSettingsService};
use crate::model::fleet::valid_node_id;
use crate::model::goal::Goal;
use crate::model::node::NodeRegistry;

pub(crate) mod state;

/// Refs namespace holding a resolution's entire durable state. Bounded:
/// entries exist between pinning and the retire that follows publication or
/// escalation, and an interruption in between is cleared by
/// [`sweep_abandoned_resolutions`] on the next divergence.
const RESOLVE_REF_NAMESPACE: &str = "refs/refine/resolve";

/// Resolution attempts before escalating to `NeedsDecision`. A rejected
/// output re-prompts with feedback; it never fences.
pub const RESOLUTION_ATTEMPT_LIMIT: u32 = 2;

/// Refs namespace holding each contended record's resolution budget: one ref
/// per attempt bought, targeting the remote head it was bought against.
/// Bounded by construction — at most [`CONTENTION_ATTEMPT_LIMIT`] refs per
/// contended record, and every ref bought against a superseded remote head is
/// swept the next time that record is contended.
const CONTENTION_REF_NAMESPACE: &str = "refs/refine/contention";

/// How many consecutive resolution engagements one contended record may buy
/// while the side that actually needs deciding — the remote head — has not
/// moved. Agent calls cost real money and a resolution that answered nothing
/// will answer nothing again, so a node under sustained contention holds here
/// instead of paying per pass. The hold is never a fence: see
/// [`buy_contention_attempt`].
pub const CONTENTION_ATTEMPT_LIMIT: u32 = 2;

/// The ownership doctrine handed to the state resolver as guidance, quoted
/// from `docs/intent/02-foundation/04-fleet.md` (a test pins the quote to the
/// intent doc so the two cannot drift apart).
pub const OWNERSHIP_DOCTRINE: &str = "Reconciliation never guesses a winner from circumstance: \
timestamps, recency, and which node happens to run the merge decide nothing. Ownership is \
declared doctrine, never circumstance: the node that owned a record at the merge base is \
authoritative for contested members, and staleness alone never discards work only the owning \
node could produce — a stale local understanding is not a wrong one. Round evidence and the \
workflow authority that produced it (status, assignment, branch) move as one coupled unit: \
Rounds and other identity-free ordered arrays are atomic and never split from that authority. \
Nothing is silently destroyed: every losing side is retained as a ref before publication.";

/// One conflicted path with its domain-terms summary (which goal, which
/// members each side changed) — the vocabulary escalation speaks in.
#[derive(Clone, Debug)]
pub struct ConflictedPath {
    pub path: String,
    pub summary: String,
}

/// The three sides of one conflicted path, read from the pinned refs.
/// `None` means the path does not exist on that side.
#[derive(Clone, Debug)]
pub struct PathSides {
    pub path: String,
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
}

/// What a resolver receives: a materialized conflicted checkout with markers
/// in place, the per-path operands, and caller-rendered domain context. The
/// resolver edits the workspace in place; the core re-reads and gates it.
#[derive(Debug)]
pub struct ResolutionRequest<'a> {
    pub workspace_dir: &'a Path,
    pub conflicts: &'a [ConflictedPath],
    pub sides: &'a [PathSides],
    pub ancestry: &'a str,
    pub context: &'a str,
    /// 1-based attempt number, at most [`RESOLUTION_ATTEMPT_LIMIT`].
    pub attempt: u32,
    /// Why the previous attempt was rejected, when this is a re-prompt.
    pub feedback: Option<&'a str>,
}

/// What one resolver attempt reported. `Completed` claims the workspace now
/// holds a resolution — the acceptance gates decide whether that is true.
#[derive(Clone, Debug)]
pub enum ResolverOutcome {
    Completed,
    /// The resolver read both sides and cannot choose: the question is its
    /// own words, naming what it could not decide and why. Escalation carries
    /// this verbatim instead of asking again.
    NeedsDecision {
        question: String,
    },
    /// No agent is installed or enabled; the caller falls back exactly to the
    /// behavior it had before agent resolution existed.
    Unavailable,
    Failed(String),
}

/// A conflict-resolution engine. State files and conflicted rebase
/// workspaces implement policy differences above this trait, never below it.
pub trait ConflictResolver {
    fn resolve(&self, request: &ResolutionRequest<'_>) -> RefineResult<ResolverOutcome>;
}

type SharedResolver = Arc<dyn ConflictResolver + Send + Sync>;

fn resolver_overrides() -> &'static Mutex<BTreeMap<PathBuf, SharedResolver>> {
    static OVERRIDES: OnceLock<Mutex<BTreeMap<PathBuf, SharedResolver>>> = OnceLock::new();
    OVERRIDES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn resolver_override_key(target_root: &Path) -> PathBuf {
    target_root
        .canonicalize()
        .unwrap_or_else(|_| target_root.to_path_buf())
}

/// Uninstalls its repository's resolver override when dropped.
pub struct ResolverOverrideGuard {
    key: PathBuf,
}

impl Drop for ResolverOverrideGuard {
    fn drop(&mut self) {
        if let Ok(mut overrides) = resolver_overrides().lock() {
            overrides.remove(&self.key);
        }
    }
}

/// Route this repository's conflict resolutions to the given resolver instead
/// of the installed-agent default — the injection point the sync simulation
/// harness and direct-mode tests use to script deterministic outcomes. Keyed
/// by target root so concurrent tests never observe each other's resolver.
pub fn install_resolver_override(
    target_root: &Path,
    resolver: SharedResolver,
) -> ResolverOverrideGuard {
    let key = resolver_override_key(target_root);
    resolver_overrides()
        .lock()
        .expect("resolver override registry poisoned")
        .insert(key.clone(), resolver);
    ResolverOverrideGuard { key }
}

/// The resolver override installed for this repository, if any.
pub fn resolver_override(target_root: &Path) -> Option<SharedResolver> {
    resolver_overrides()
        .lock()
        .ok()?
        .get(&resolver_override_key(target_root))
        .cloned()
}

/// Verdict of the deterministic acceptance gates over a resolver's output.
#[derive(Clone, Debug)]
pub enum GateVerdict {
    Accepted,
    /// Feedback appended to the next attempt's context.
    Rejected(String),
}

/// Deterministic acceptance gates over the edited workspace. Rejection means
/// re-prompt; it never fences.
pub trait AcceptanceGates {
    fn review(
        &self,
        workspace_dir: &Path,
        conflicts: &[ConflictedPath],
    ) -> RefineResult<GateVerdict>;
}

/// Everything the caller prepared under lock Hold A: the claim on this
/// divergence, pinned operands, the materialized workspace, the conflicted
/// merge tree the result builds on, and the rendered domain context.
#[derive(Debug)]
pub struct PreparedResolution {
    /// Held from Hold A until the resolution is published or retired, so no
    /// second operation edits this workspace or overwrites this result.
    pub lock: ResolutionLock,
    pub pinned: PinnedResolution,
    pub workspace: PathBuf,
    /// The in-memory merge's tree with conflicted paths still carrying
    /// placeholder content; the accepted resolution replaces exactly those.
    pub merged_tree: String,
    pub conflicts: Vec<ConflictedPath>,
    pub sides: Vec<PathSides>,
    pub ancestry: String,
    pub context: String,
}

/// Terminal outcome of one resolution run.
#[derive(Clone, Debug)]
pub enum ResolutionOutcome {
    Resolved {
        result_commit: String,
        result_tree: String,
    },
    /// Genuine ambiguity: a domain-terms question naming the goal, the
    /// contested members, and what must be chosen.
    NeedsDecision {
        question: String,
    },
    Unavailable,
}

/// The pinned operands of one resolution, all of them refs.
#[derive(Clone, Debug)]
pub struct PinnedResolution {
    pub id: String,
    pub base: String,
    pub ours: String,
    pub theirs: String,
    /// A surviving result from an interrupted earlier run of the same
    /// divergence: skip straight to publish.
    pub result: Option<String>,
}

pub fn result_ref(id: &str) -> String {
    resolution_ref(id, "result")
}

/// An operation's exclusive claim on one resolution id, taken under lock Hold
/// A and kept until the resolution is published or retired.
///
/// The agent runs unlocked, so the daemon's sync worker and an
/// operator-triggered sync can otherwise reach the same divergence at once:
/// they derive the same id, so the second one's workspace re-derivation would
/// erase the first one's in-flight edits, and its result would overwrite a
/// gated one. Crash-only: the claim is a file lock, so it dies with the
/// process and a rerun re-takes it.
pub struct ResolutionLock {
    id: String,
    file: File,
}

impl std::fmt::Debug for ResolutionLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolutionLock")
            .field("id", &self.id)
            .finish()
    }
}

impl Drop for ResolutionLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Claim a resolution id for this operation, or `None` when another operation
/// is already resolving it and this one should defer. Never blocks: an agent
/// resolution takes minutes, and a waiter would hold its own caller for all
/// of them.
pub fn try_claim_resolution(
    worktrees: &FileGitWorktreeService,
    id: &str,
) -> RefineResult<Option<ResolutionLock>> {
    validate_resolution_id(id)?;
    let path = resolution_lock_path(worktrees, id)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to open resolution lock {}: {error}",
                path.display()
            ))
        })?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(ResolutionLock {
            id: id.to_string(),
            file,
        })),
        Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(RefineError::Io(format!(
            "failed to claim resolution lock {}: {error}",
            path.display()
        ))),
    }
}

/// Where every resolution keeps its workspace and its lock, inside the
/// repository's own Git directory rather than any user-visible tree.
fn resolution_root(worktrees: &FileGitWorktreeService) -> RefineResult<PathBuf> {
    let root = worktrees.git_path("refine-resolve")?;
    fs::create_dir_all(&root).map_err(|error| {
        RefineError::Io(format!(
            "failed to create resolve workspace root {}: {error}",
            root.display()
        ))
    })?;
    Ok(root)
}

fn resolution_workspace(worktrees: &FileGitWorktreeService, id: &str) -> RefineResult<PathBuf> {
    Ok(resolution_root(worktrees)?.join(id))
}

fn resolution_lock_path(worktrees: &FileGitWorktreeService, id: &str) -> RefineResult<PathBuf> {
    Ok(resolution_root(worktrees)?.join(format!("{id}.lock")))
}

fn resolution_ref(id: &str, leaf: &str) -> String {
    format!("{RESOLVE_REF_NAMESPACE}/{id}/{leaf}")
}

fn validate_resolution_id(id: &str) -> RefineResult<()> {
    if !id.is_empty()
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        Ok(())
    } else {
        Err(RefineError::InvalidInput(format!(
            "resolution id {id:?} must be non-empty and use only alphanumerics, '-', or '_'"
        )))
    }
}

/// Pin the merge operands as refs under `refs/refine/resolve/<id>/` (lock
/// Hold A). Idempotent: pins matching the operands are kept along with any
/// surviving result ref; stale pins — any head changed — are discarded and
/// re-pinned, and the stale result ref is dropped with them.
pub fn pin_resolution_inputs(
    repo: &FileGitSyncService,
    root: &Path,
    id: &str,
    base: &str,
    ours: &str,
    theirs: &str,
) -> RefineResult<PinnedResolution> {
    validate_resolution_id(id)?;
    let pins = [("base", base), ("ours", ours), ("theirs", theirs)];
    let unchanged = pins.iter().try_fold(true, |unchanged, (leaf, operand)| {
        Ok::<_, RefineError>(
            unchanged
                && ref_target(repo, root, &resolution_ref(id, leaf))?.as_deref() == Some(*operand),
        )
    })?;
    if !unchanged {
        for (leaf, operand) in pins {
            repo.git_at_checked(root, &["update-ref", &resolution_ref(id, leaf), operand])?;
        }
        if ref_target(repo, root, &result_ref(id))?.is_some() {
            repo.git_at_checked(root, &["update-ref", "-d", &result_ref(id)])?;
        }
    }
    Ok(PinnedResolution {
        id: id.to_string(),
        base: base.to_string(),
        ours: ours.to_string(),
        theirs: theirs.to_string(),
        result: ref_target(repo, root, &result_ref(id))?,
    })
}

/// Retire one resolution once it is published, superseded, or escalated:
/// delete its refs, remove its workspace worktree, and drop its lock file —
/// in that order, so an operation that claims the id after the unlink finds
/// nothing of the old one left behind. Idempotent.
///
/// `held` is the caller's own claim on the id; without one the retire claims
/// the id itself and leaves an id another operation is actively resolving to
/// that operation (`Ok(false)`). The workspace is a full checkout of the
/// state tree, so skipping this leaks one per divergence forever.
pub fn retire_resolution(
    worktrees: &FileGitWorktreeService,
    repo: &FileGitSyncService,
    root: &Path,
    id: &str,
    held: Option<&ResolutionLock>,
) -> RefineResult<bool> {
    validate_resolution_id(id)?;
    let _claim = match held.filter(|lock| lock.id == id) {
        Some(_) => None,
        None => match try_claim_resolution(worktrees, id)? {
            Some(claim) => Some(claim),
            None => return Ok(false),
        },
    };
    let refs = repo.git_at_stdout(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("{RESOLVE_REF_NAMESPACE}/{id}"),
        ],
    )?;
    for reference in refs.lines().map(str::trim).filter(|line| !line.is_empty()) {
        repo.git_at_checked(root, &["update-ref", "-d", reference])?;
    }
    let workspace = resolution_workspace(worktrees, id)?;
    if workspace.exists() {
        worktrees.purge_worktree(&workspace)?;
    }
    let lock_path = resolution_lock_path(worktrees, id)?;
    match fs::remove_file(&lock_path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(RefineError::Io(format!(
                "failed to remove resolution lock {}: {error}",
                lock_path.display()
            )));
        }
    }
    Ok(true)
}

/// Retire every resolution except `keep` that no operation still holds.
///
/// Orphans are ordinary rather than exceptional: a crash after publication, a
/// divergence another node converged first, or a head that moved so the
/// re-derivation ran under a new id. Each one otherwise keeps a locked
/// worktree — a full checkout of the state tree — and its refs forever, which
/// is how a persistent conflict on a live node turns into unbounded disk
/// growth. Run this under lock Hold A, after the current resolution is
/// claimed; ids another operation holds are left to that operation.
pub fn sweep_abandoned_resolutions(
    worktrees: &FileGitWorktreeService,
    repo: &FileGitSyncService,
    root: &Path,
    keep: &str,
) -> RefineResult<Vec<String>> {
    let mut ids = BTreeSet::new();
    let refs = repo.git_at_stdout(
        root,
        &["for-each-ref", "--format=%(refname)", RESOLVE_REF_NAMESPACE],
    )?;
    for reference in refs.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(rest) = reference.strip_prefix(&format!("{RESOLVE_REF_NAMESPACE}/"))
            && let Some(id) = rest.split('/').next()
        {
            ids.insert(id.to_string());
        }
    }
    let workspace_root = resolution_root(worktrees)?;
    if let Ok(entries) = fs::read_dir(&workspace_root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            ids.insert(name.strip_suffix(".lock").unwrap_or(&name).to_string());
        }
    }
    let mut retired = Vec::new();
    for id in ids {
        if id == keep || validate_resolution_id(&id).is_err() {
            continue;
        }
        if retire_resolution(worktrees, repo, root, &id, None)? {
            retired.push(id);
        }
    }
    Ok(retired)
}

/// The gated result an interrupted earlier run of this divergence left
/// behind, if any. Publishing it costs no agent — it is the crash-only rerun
/// path — so it is never charged against a contention budget.
pub fn surviving_resolution_result(
    repo: &FileGitSyncService,
    root: &Path,
    id: &str,
) -> RefineResult<Option<String>> {
    validate_resolution_id(id)?;
    ref_target(repo, root, &result_ref(id))
}

/// The durable identity of one contended record's contention: the record and
/// the remote head that must be reconciled with it.
///
/// The local side is deliberately absent. A node that keeps working snapshots
/// live state onto its branch every pass, so its local head — and with it the
/// divergence's id — moves constantly while nothing that actually needs
/// deciding has changed. Charging an agent for that churn is exactly how a
/// busy node under one standing contention bought unbounded resolution
/// attempts.
fn contention_key(remote_head: &str, path: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(remote_head.as_bytes());
    digest.update([0]);
    digest.update(path.as_bytes());
    format!("{:x}", digest.finalize())
}

/// What each contended record has already spent against this remote head,
/// after dropping every attempt bought against a head that has since been
/// superseded.
fn spent_contention(
    repo: &FileGitSyncService,
    root: &Path,
    remote_head: &str,
    paths: &[String],
) -> RefineResult<Vec<(String, u32)>> {
    sweep_superseded_contention(repo, root, remote_head)?;
    let mut spent = Vec::new();
    for path in paths {
        let key = contention_key(remote_head, path);
        let attempts = contention_attempts(repo, root, &key)?;
        spent.push((key, attempts));
    }
    Ok(spent)
}

/// Whether any contended record still has budget to engage a resolver against
/// this remote head. Reading the budget is separate from spending it so a
/// caller can decide to hold BEFORE doing the preparation an engagement needs:
/// preparation that fails the same way every pass would otherwise spend the
/// budget without an agent ever seeing the contention.
///
/// The budget is per contended RECORD, so a record newly dragged into the
/// contention brings its own attempts with it and is decided at once; only
/// records that already had their turn are held.
pub fn contention_budget_available(
    repo: &FileGitSyncService,
    root: &Path,
    remote_head: &str,
    paths: &[String],
) -> RefineResult<bool> {
    Ok(spent_contention(repo, root, remote_head, paths)?
        .iter()
        .any(|(_, attempts)| *attempts < CONTENTION_ATTEMPT_LIMIT))
}

/// Buy one resolution attempt for each contended record against this remote
/// head, returning the highest attempt number bought — or `None` when every
/// contended record has spent its budget and resolution must hold.
///
/// A moved remote head mints new keys, which is why the attempt refs also
/// carry it: every ref bought against a superseded head is swept here, so the
/// namespace holds at most [`CONTENTION_ATTEMPT_LIMIT`] refs per currently
/// contended record.
///
/// Crash-only: the attempt is bought under lock Hold A, BEFORE the unlocked
/// agent call, so a process that dies mid-resolution has already paid for the
/// attempt it started and the rerun spends what is left rather than starting
/// over. Nothing here fences: a held contention still reports, still answers
/// to `--authority`, and re-engages the moment the remote side moves.
pub fn buy_contention_attempt(
    repo: &FileGitSyncService,
    root: &Path,
    remote_head: &str,
    paths: &[String],
) -> RefineResult<Option<u32>> {
    let spent = spent_contention(repo, root, remote_head, paths)?;
    if spent
        .iter()
        .all(|(_, attempts)| *attempts >= CONTENTION_ATTEMPT_LIMIT)
    {
        return Ok(None);
    }
    let mut bought = None;
    for (key, attempts) in spent {
        if attempts >= CONTENTION_ATTEMPT_LIMIT {
            continue;
        }
        let attempt = attempts + 1;
        repo.git_at_checked(
            root,
            &[
                "update-ref",
                &format!("{CONTENTION_REF_NAMESPACE}/{key}/{attempt}"),
                remote_head,
            ],
        )?;
        bought = Some(bought.unwrap_or(0).max(attempt));
    }
    Ok(bought)
}

/// How many attempts this contended record has already bought.
fn contention_attempts(repo: &FileGitSyncService, root: &Path, key: &str) -> RefineResult<u32> {
    let refs = repo.git_at_stdout(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("{CONTENTION_REF_NAMESPACE}/{key}"),
        ],
    )?;
    let attempts = refs.lines().filter(|line| !line.trim().is_empty()).count();
    Ok(u32::try_from(attempts).unwrap_or(u32::MAX))
}

/// Drop every attempt bought against a remote head that is no longer the one
/// needing a decision. The ref's target IS the head it was bought against, so
/// the sweep needs no bookkeeping beyond the refs themselves.
fn sweep_superseded_contention(
    repo: &FileGitSyncService,
    root: &Path,
    remote_head: &str,
) -> RefineResult<()> {
    let listing = repo.git_at_stdout(
        root,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            CONTENTION_REF_NAMESPACE,
        ],
    )?;
    for line in listing.lines() {
        let Some((target, reference)) = line.trim().split_once(' ') else {
            continue;
        };
        if target == remote_head {
            continue;
        }
        repo.git_at_checked(root, &["update-ref", "-d", reference])?;
    }
    Ok(())
}

/// Read the three sides of each conflicted path from the pinned refs.
pub fn conflict_sides(
    repo: &FileGitSyncService,
    root: &Path,
    pinned: &PinnedResolution,
    paths: &[String],
) -> RefineResult<Vec<PathSides>> {
    paths
        .iter()
        .map(|path| {
            Ok(PathSides {
                path: path.clone(),
                base: bytes_at(repo, root, &pinned.base, path)?,
                ours: bytes_at(repo, root, &pinned.ours, path)?,
                theirs: bytes_at(repo, root, &pinned.theirs, path)?,
            })
        })
        .collect()
}

/// Materialize the isolated, Refine-owned workspace for a resolution (lock
/// Hold A): a locked detached worktree under the repository's Git directory,
/// reset pristine at `ours` with diff3-style conflict markers written into
/// every conflicted path. Reruns re-derive the same workspace, discarding any
/// partial edits a crashed attempt left behind — which is exactly why the
/// caller must hold this id's [`ResolutionLock`] first: the re-derivation
/// cannot tell a crashed attempt's leftovers from a live one's edits.
pub fn materialize_conflicted_workspace(
    worktrees: &FileGitWorktreeService,
    repo: &FileGitSyncService,
    pinned: &PinnedResolution,
    sides: &[PathSides],
) -> RefineResult<PathBuf> {
    validate_resolution_id(&pinned.id)?;
    let workspace = resolution_workspace(worktrees, &pinned.id)?;
    worktrees.ensure_detached_worktree(&workspace, &pinned.ours)?;
    // An existing registration is kept as-is by `ensure`, so force the tree
    // pristine before laying markers down: crash-only means re-derive.
    repo.git_at_checked(&workspace, &["reset", "--hard", &pinned.ours])?;
    repo.git_at_checked(&workspace, &["clean", "-fdq"])?;
    for side in sides {
        let destination = workspace.join(&side.path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                RefineError::Io(format!(
                    "failed to create resolve workspace directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        fs::write(
            &destination,
            marker_file_bytes(
                side.base.as_deref(),
                side.ours.as_deref(),
                side.theirs.as_deref(),
            ),
        )
        .map_err(|error| {
            RefineError::Io(format!(
                "failed to write conflicted workspace file {}: {error}",
                destination.display()
            ))
        })?;
    }
    Ok(workspace)
}

/// Run bounded resolver attempts with acceptance gates between (unlocked),
/// under the caller's claim on this id.
///
/// A surviving result ref short-circuits to `Resolved` — the crash-only
/// rerun path. An accepted output is committed with both heads as parents
/// and recorded as `refs/refine/resolve/<id>/result`; publication itself is
/// the caller's Hold B. A resolver that declares it cannot choose escalates
/// immediately with its own question: re-prompting an agent that already
/// stated what it needs decided buys nothing.
pub fn resolve_conflict(
    repo: &FileGitSyncService,
    prepared: &PreparedResolution,
    resolver: &dyn ConflictResolver,
    gates: &dyn AcceptanceGates,
) -> RefineResult<ResolutionOutcome> {
    let id = &prepared.pinned.id;
    validate_resolution_id(id)?;
    let existing = match &prepared.pinned.result {
        Some(result) => Some(result.clone()),
        None => ref_target(repo, &prepared.workspace, &result_ref(id))?,
    };
    if let Some(result_commit) = existing {
        let result_tree = repo.git_at_stdout(
            &prepared.workspace,
            &["rev-parse", &format!("{result_commit}^{{tree}}")],
        )?;
        return Ok(ResolutionOutcome::Resolved {
            result_commit,
            result_tree,
        });
    }

    let mut feedback: Option<String> = None;
    for attempt in 1..=RESOLUTION_ATTEMPT_LIMIT {
        let request = ResolutionRequest {
            workspace_dir: &prepared.workspace,
            conflicts: &prepared.conflicts,
            sides: &prepared.sides,
            ancestry: &prepared.ancestry,
            context: &prepared.context,
            attempt,
            feedback: feedback.as_deref(),
        };
        match resolver.resolve(&request)? {
            ResolverOutcome::Unavailable => return Ok(ResolutionOutcome::Unavailable),
            ResolverOutcome::NeedsDecision { question } => {
                return Ok(ResolutionOutcome::NeedsDecision { question });
            }
            ResolverOutcome::Failed(reason) => {
                feedback = Some(format!("the previous resolution attempt failed: {reason}"));
            }
            ResolverOutcome::Completed => {
                match gates.review(&prepared.workspace, &prepared.conflicts)? {
                    GateVerdict::Accepted => {
                        return commit_resolution(repo, prepared);
                    }
                    GateVerdict::Rejected(why) => feedback = Some(why),
                }
            }
        }
    }
    Ok(ResolutionOutcome::NeedsDecision {
        question: needs_decision_question(&prepared.conflicts, feedback.as_deref()),
    })
}

/// Commit the gated workspace as the resolution result: conflicted paths are
/// re-read from the workspace (a missing file adopts a deletion), applied
/// over the merged tree, committed with both heads as parents, and recorded
/// under the result ref. Idempotent per divergence: recreating the same
/// content yields the same outcome.
fn commit_resolution(
    repo: &FileGitSyncService,
    prepared: &PreparedResolution,
) -> RefineResult<ResolutionOutcome> {
    let workspace = &prepared.workspace;
    let mut operations = Vec::new();
    for conflict in &prepared.conflicts {
        match fs::read(workspace.join(&conflict.path)) {
            Ok(bytes) => operations.push(TreeOperation::set(
                conflict.path.clone(),
                write_blob(repo, workspace, &bytes)?,
            )),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                operations.push(TreeOperation::Remove {
                    path: conflict.path.clone(),
                });
            }
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read resolved workspace file {}: {error}",
                    workspace.join(&conflict.path).display()
                )));
            }
        }
    }
    let result_tree = build_tree(repo, workspace, &prepared.merged_tree, &operations)?;
    let result_commit = commit_tree(
        repo,
        workspace,
        &result_tree,
        &[&prepared.pinned.ours, &prepared.pinned.theirs],
        &format!("Resolve synchronized conflict {}", prepared.pinned.id),
    )?;
    repo.git_at_checked(
        workspace,
        &[
            "update-ref",
            &result_ref(&prepared.pinned.id),
            &result_commit,
        ],
    )?;
    Ok(ResolutionOutcome::Resolved {
        result_commit,
        result_tree,
    })
}

/// The question escalation carries when the budget ran out without the
/// resolver ever authoring one — every attempt was rejected by the gates or
/// failed outright, so there is no agent statement to quote. A resolver that
/// declares [`ResolverOutcome::NeedsDecision`] escalates with its own words
/// instead and never reaches this.
fn needs_decision_question(conflicts: &[ConflictedPath], feedback: Option<&str>) -> String {
    let contested = conflicts
        .iter()
        .map(|conflict| format!("- {} — {}", conflict.path, conflict.summary))
        .collect::<Vec<_>>()
        .join("\n");
    let rejection = feedback
        .map(|why| format!(" (last attempt was rejected: {why})"))
        .unwrap_or_default();
    format!(
        "Automatic resolution could not produce one valid record{rejection}. Contested:\n{contested}\nChoose which side each contested record should take: rerun sync with `--authority live` or `--authority remote` (add `--path` for per-path exceptions), or edit the records and sync again."
    )
}

/// Render the state-conflict domain context from the per-path block.
pub fn state_conflict_context(conflicts_block: &str) -> String {
    crate::application::agent_io::prompts::render(
        PromptTemplate::ResolveStateConflict,
        &[
            ("conflicts", conflicts_block),
            ("doctrine", OWNERSHIP_DOCTRINE),
        ],
    )
}

/// Render the candidate-conflict domain context: the goal prompt, the round
/// intent, the implementation reports of other goals whose commits conflict
/// (so the resolver understands *both intents*, not just both diffs), and the
/// conflicted file list.
pub fn candidate_conflict_context(
    goal_prompt: &str,
    round_intent: &str,
    conflicted_files: &str,
    other_goal_reports: &str,
) -> String {
    crate::application::agent_io::prompts::render(
        PromptTemplate::ResolveCandidateConflict,
        &[
            ("goal_prompt", goal_prompt),
            ("round_intent", round_intent),
            ("conflicted_files", conflicted_files),
            ("other_goal_reports", other_goal_reports),
        ],
    )
}

/// The paths that still carry conflict markers after a resolver claimed
/// completion — the shared no-markers gate primitive. A missing file adopts a
/// deletion and is never marked.
pub fn paths_with_conflict_markers(dir: &Path, paths: &[String]) -> RefineResult<Vec<String>> {
    let mut marked = Vec::new();
    for path in paths {
        match fs::read(dir.join(path)) {
            Ok(bytes) => {
                if has_conflict_markers(&bytes) {
                    marked.push(path.clone());
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(RefineError::Io(format!(
                    "failed to read resolved workspace file {}: {error}",
                    dir.join(path).display()
                )));
            }
        }
    }
    Ok(marked)
}

/// Per-path block for the state prompt: the domain summary of what each side
/// changed, then both record renderings.
pub fn state_conflict_block(conflicts: &[ConflictedPath], sides: &[PathSides]) -> String {
    conflicts
        .iter()
        .map(|conflict| {
            let side = sides.iter().find(|side| side.path == conflict.path);
            let render = |bytes: Option<&Vec<u8>>| {
                bytes
                    .map(|bytes| String::from_utf8_lossy(bytes).trim_end().to_string())
                    .unwrap_or_else(|| "(absent)".to_string())
            };
            format!(
                "## {}\n{}\n\nThis node's record:\n{}\n\nOther node's record:\n{}",
                conflict.path,
                conflict.summary,
                render(side.and_then(|side| side.ours.as_ref())),
                render(side.and_then(|side| side.theirs.as_ref())),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The state-file acceptance gates: no conflict markers remain, the bytes
/// parse as JSON, and the record schema-validates — the same Goal
/// deserialization and record invariants that used to veto the merge,
/// repurposed as an acceptance gate. A path deleted by the resolver adopts a
/// deletion and passes. Rejection is feedback, never a fence.
pub struct StateFileGates;

impl AcceptanceGates for StateFileGates {
    fn review(
        &self,
        workspace_dir: &Path,
        conflicts: &[ConflictedPath],
    ) -> RefineResult<GateVerdict> {
        let mut problems = Vec::new();
        for conflict in conflicts {
            let path = workspace_dir.join(&conflict.path);
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(RefineError::Io(format!(
                        "failed to read resolved workspace file {}: {error}",
                        path.display()
                    )));
                }
            };
            if has_conflict_markers(&bytes) {
                problems.push(format!("{} still contains conflict markers", conflict.path));
                continue;
            }
            if let Err(error) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                problems.push(format!("{} is not valid JSON: {error}", conflict.path));
                continue;
            }
            if let Err(problem) = state_schema_gate(&conflict.path, &bytes) {
                problems.push(problem);
            }
        }
        Ok(if problems.is_empty() {
            GateVerdict::Accepted
        } else {
            GateVerdict::Rejected(problems.join("; "))
        })
    }
}

fn has_conflict_markers(bytes: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        line.starts_with(b"<<<<<<<")
            || line.starts_with(b"|||||||")
            || line.starts_with(b"=======")
            || line.starts_with(b">>>>>>>")
    })
}

fn state_schema_gate(tree_path: &str, bytes: &[u8]) -> Result<(), String> {
    let relative = tree_path.strip_prefix(".refine/").unwrap_or(tree_path);
    let relative_path = Path::new(relative);
    if relative_path.file_name().and_then(|name| name.to_str()) == Some("goal.json") {
        let goal = serde_json::from_slice::<Goal>(bytes)
            .map_err(|error| format!("{tree_path} does not deserialize as a Goal: {error}"))?;
        if goal.id.trim().is_empty() {
            return Err(format!("{tree_path} carries an empty goal id"));
        }
        if let Some(expected) = goal_id_from_record_path(relative_path)
            && goal.id != expected
        {
            return Err(format!(
                "{tree_path} carries goal id {} but the record path names {expected}",
                goal.id
            ));
        }
        return Ok(());
    }
    if relative == "nodes.json" {
        return node_registry_gate(bytes).map_err(|problem| format!("{tree_path} {problem}"));
    }
    Ok(())
}

fn node_registry_gate(bytes: &[u8]) -> Result<(), String> {
    let registry = serde_json::from_slice::<NodeRegistry>(bytes)
        .map_err(|error| format!("does not deserialize as the node registry: {error}"))?;
    let mut seen = std::collections::BTreeSet::new();
    for node in &registry.nodes {
        if node.id != node.id.trim() || !valid_node_id(&node.id) {
            return Err(format!("names an invalid node id {:?}", node.id));
        }
        if !seen.insert(node.id.as_str()) {
            return Err(format!("repeats node id {}", node.id));
        }
        chrono::DateTime::parse_from_rfc3339(&node.updated_at)
            .map_err(|error| format!("node {} has an unparseable updated_at: {error}", node.id))?;
    }
    Ok(())
}

/// `goals/<shard>/<rest>/goal.json` identifies Goal `<shard><rest>`.
fn goal_id_from_record_path(relative: &Path) -> Option<String> {
    let mut components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    components.pop()?;
    let rest = components.pop()?;
    let shard = components.pop()?;
    if components.last().map(String::as_str) != Some("goals") {
        return None;
    }
    Some(format!("{shard}{rest}"))
}

/// Resolves conflicts through Refine's installed-agent CLI machinery — the
/// same `HostAgentProviderService` invocation path goal workflow steps use,
/// so the login-shell environment capture, prompt transport, supervision,
/// stall budget, and transcript handling are all inherited rather than
/// reimplemented. The `agent_idle_timeout_seconds` convention bounds it.
pub struct InstalledAgentResolver {
    pub provider: String,
    /// The port-scoped agents runtime root (`<runtime>/agents`), matching the
    /// workflow's `ctx.runtime_root.join("agents")` convention.
    pub agents_runtime_root: PathBuf,
    pub stall_timeout_seconds: Option<u64>,
}

impl InstalledAgentResolver {
    /// Build from project settings: `agent_cli` picks the provider and
    /// `agent_idle_timeout_seconds` supplies the supervised stall budget,
    /// exactly as unattended workflow verdict invocations derive theirs.
    pub fn from_settings(refine_dir: &Path, runtime_root: &Path) -> Self {
        let settings = FileSettingsService::with_active_root(refine_dir, runtime_root)
            .load()
            .unwrap_or_default();
        let provider = settings
            .get("agent_cli")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .unwrap_or("claude")
            .to_string();
        let stall_timeout_seconds = settings
            .get("agent_idle_timeout_seconds")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(900);
        Self {
            provider,
            agents_runtime_root: runtime_root.join("agents"),
            stall_timeout_seconds: Some(stall_timeout_seconds),
        }
    }
}

impl ConflictResolver for InstalledAgentResolver {
    fn resolve(&self, request: &ResolutionRequest<'_>) -> RefineResult<ResolverOutcome> {
        let service = HostAgentProviderService::with_runtime_root(&self.agents_runtime_root);
        // Not installed means unavailable, never failed: the caller falls
        // back exactly to pre-agent behavior.
        if service.authenticate(&self.provider).is_err() {
            return Ok(ResolverOutcome::Unavailable);
        }
        let invocation = ProviderInvocation {
            provider: self.provider.clone(),
            prompt: resolution_prompt(request),
            session_id: None,
            cwd: Some(request.workspace_dir.display().to_string()),
            stall_timeout_seconds: self.stall_timeout_seconds,
            process_metadata: serde_json::from_value(json!({
                "kind": "conflict_resolution",
                "workspace": request.workspace_dir.display().to_string(),
                "attempt": request.attempt,
            }))
            .unwrap_or_default(),
        };
        match service.invoke(invocation) {
            Ok(output) => Ok(match authored_question(&output) {
                Some(question) => ResolverOutcome::NeedsDecision { question },
                None => ResolverOutcome::Completed,
            }),
            Err(error) => Ok(ResolverOutcome::Failed(error.to_string())),
        }
    }
}

/// Everything the request carries that the agent cannot read off the
/// workspace: the caller's domain context, how the two lines relate, and why
/// a previous attempt was rejected.
fn resolution_prompt(request: &ResolutionRequest<'_>) -> String {
    let mut prompt = request.context.trim_end().to_string();
    let ancestry = request.ancestry.trim();
    if !ancestry.is_empty() {
        prompt.push_str(&format!("\n\nHow the two lines relate: {ancestry}."));
    }
    if let Some(feedback) = request.feedback {
        prompt.push_str(&format!(
            "\n\nYour previous attempt was rejected: {feedback}\nEdit the conflicted files again and correct this."
        ));
    }
    prompt
}

/// The line every resolution prompt tells the agent to use when it cannot
/// choose. Escalation must carry the agent's own open question — what it
/// could not decide and why — so the agent needs a channel back that is not
/// "edit the files"; its reply is that channel.
pub const NEEDS_DECISION_MARKER: &str = "NEEDS DECISION:";

/// The question the resolver authored in its reply, if it declared one.
/// Everything after the last marker is the agent's own statement, quoted
/// verbatim into the escalation. Lenient about case and surrounding prose,
/// like every other reply Refine reads back from an agent.
fn authored_question(output: &str) -> Option<String> {
    // ASCII-only case folding keeps byte offsets aligned with `output`.
    let index = output.to_ascii_uppercase().rfind(NEEDS_DECISION_MARKER)?;
    let question = output[index + NEEDS_DECISION_MARKER.len()..].trim();
    (!question.is_empty()).then(|| question.to_string())
}

/// Table-driven resolver for tests and the sync simulation harness: each
/// attempt consumes the next scripted step, which may edit the workspace.
/// Exhausted scripts fail deterministically instead of pretending success.
pub struct ScriptedResolver {
    steps: Mutex<VecDeque<ScriptedStep>>,
}

pub type ScriptedStep =
    Box<dyn FnMut(&ResolutionRequest<'_>) -> RefineResult<ResolverOutcome> + Send>;

impl ScriptedResolver {
    pub fn new(steps: Vec<ScriptedStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into_iter().collect()),
        }
    }
}

impl ConflictResolver for ScriptedResolver {
    fn resolve(&self, request: &ResolutionRequest<'_>) -> RefineResult<ResolverOutcome> {
        let mut steps = self
            .steps
            .lock()
            .map_err(|_| RefineError::Conflict("scripted resolver was poisoned".to_string()))?;
        match steps.pop_front() {
            Some(mut step) => step(request),
            None => Ok(ResolverOutcome::Failed(
                "scripted resolver has no remaining steps".to_string(),
            )),
        }
    }
}

fn ref_target(
    repo: &FileGitSyncService,
    root: &Path,
    reference: &str,
) -> RefineResult<Option<String>> {
    let output = repo.git_at(root, &["rev-parse", "--verify", "--quiet", reference])?;
    if !output.success {
        return Ok(None);
    }
    let target = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!target.is_empty()).then_some(target))
}

fn bytes_at(
    repo: &FileGitSyncService,
    root: &Path,
    commitish: &str,
    path: &str,
) -> RefineResult<Option<Vec<u8>>> {
    let output = repo.git_at(root, &["show", &format!("{commitish}:{path}")])?;
    Ok(output.success.then_some(output.stdout))
}

/// Diff3-style whole-file conflict markers over the three sides. An absent
/// side renders as an empty section, so delete/modify conflicts stay visible.
fn marker_file_bytes(base: Option<&[u8]>, ours: Option<&[u8]>, theirs: Option<&[u8]>) -> Vec<u8> {
    fn push_side(buffer: &mut Vec<u8>, side: Option<&[u8]>) {
        if let Some(bytes) = side {
            buffer.extend_from_slice(bytes);
            if !bytes.is_empty() && !bytes.ends_with(b"\n") {
                buffer.push(b'\n');
            }
        }
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"<<<<<<< ours\n");
    push_side(&mut bytes, ours);
    bytes.extend_from_slice(b"||||||| base\n");
    push_side(&mut bytes, base);
    bytes.extend_from_slice(b"=======\n");
    push_side(&mut bytes, theirs);
    bytes.extend_from_slice(b">>>>>>> theirs\n");
    bytes
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::infrastructure::git::ancestry::{Ancestry, classify};
    use crate::infrastructure::git::merge::merge_commits;

    const GOAL_PATH: &str = ".refine/goals/GO/ALA/goal.json";

    #[test]
    fn ownership_doctrine_is_quoted_from_the_fleet_intent() {
        let intent = include_str!("../../../../docs/intent/02-foundation/04-fleet.md");
        let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalize(intent).contains(&normalize(OWNERSHIP_DOCTRINE)),
            "the doctrine quote drifted from docs/intent/02-foundation/04-fleet.md"
        );
    }

    #[test]
    fn state_prompt_carries_the_doctrine_and_the_conflicts() {
        let rendered = state_conflict_context("## goal GOALA\nboth nodes changed status");
        assert!(rendered.contains("goal GOALA"));
        assert!(rendered.contains("never guesses a winner from circumstance"));
        assert!(rendered.contains("prefer the side that owns workflow authority"));
        // The channel the agent authors its own question through, spelled the
        // same way the reply is read back.
        assert!(rendered.contains(NEEDS_DECISION_MARKER));
    }

    #[test]
    fn candidate_prompt_carries_both_intents_and_the_file_list() {
        let rendered = candidate_conflict_context(
            "add rounding",
            "fix currency",
            "src/rater.rs",
            "Goal GOALB — normalize currency:\nRewrote PolicyRater.calculate for currency.",
        );
        assert!(rendered.contains("add rounding"));
        assert!(rendered.contains("fix currency"));
        assert!(rendered.contains("src/rater.rs"));
        assert!(rendered.contains("Rewrote PolicyRater.calculate for currency."));
        assert!(rendered.contains("both intents survive"));
        assert!(rendered.contains(NEEDS_DECISION_MARKER));
    }

    struct ResolveFixture {
        root: PathBuf,
        base: String,
        ours: String,
        theirs: String,
    }

    impl ResolveFixture {
        fn new(name: &str) -> Self {
            let root = unique_temp_dir(name);
            fs::create_dir_all(&root).unwrap();
            git(&root, &["init", "-q", "-b", "main"]);
            git(&root, &["config", "user.email", "resolve@test"]);
            git(&root, &["config", "user.name", "Resolve Test"]);
            let base = commit_goal(&root, goal_record("GOALA", "todo", "2026-08-17T08:00:00Z"));
            let ours = commit_goal(
                &root,
                goal_record("GOALA", "review", "2026-08-17T09:00:00Z"),
            );
            git(&root, &["checkout", "-q", &base]);
            let theirs = commit_goal(&root, goal_record("GOALA", "done", "2026-08-17T09:30:00Z"));
            git(&root, &["checkout", "-q", &ours]);
            Self {
                root,
                base,
                ours,
                theirs,
            }
        }

        fn repo(&self) -> FileGitSyncService {
            FileGitSyncService::new(&self.root, self.root.join("run"))
        }

        fn worktrees(&self) -> FileGitWorktreeService {
            FileGitWorktreeService::with_runtime_root(&self.root, self.root.join("run"))
        }

        fn prepare(&self, id: &str) -> PreparedResolution {
            let repo = self.repo();
            let ancestry = classify(&repo, &self.ours, &self.theirs).unwrap();
            assert_eq!(
                ancestry,
                Ancestry::Diverged {
                    merge_base: self.base.clone()
                }
            );
            let merged = merge_commits(&repo, &self.root, &self.ours, &self.theirs).unwrap();
            assert_eq!(merged.conflicted_paths, vec![GOAL_PATH.to_string()]);
            let lock = try_claim_resolution(&self.worktrees(), id)
                .unwrap()
                .expect("the resolution id is unclaimed");
            let pinned =
                pin_resolution_inputs(&repo, &self.root, id, &self.base, &self.ours, &self.theirs)
                    .unwrap();
            let sides =
                conflict_sides(&repo, &self.root, &pinned, &merged.conflicted_paths).unwrap();
            let workspace =
                materialize_conflicted_workspace(&self.worktrees(), &repo, &pinned, &sides)
                    .unwrap();
            let conflicts = vec![ConflictedPath {
                path: GOAL_PATH.to_string(),
                summary: "goal GOALA: both nodes changed status, updated".to_string(),
            }];
            let context = state_conflict_context(&state_conflict_block(&conflicts, &sides));
            PreparedResolution {
                lock,
                pinned,
                workspace,
                merged_tree: merged.tree,
                conflicts,
                sides,
                ancestry: format!("diverged from merge base {}", self.base),
                context,
            }
        }
    }

    impl ResolveFixture {
        /// A resolution whose operation is gone: pinned refs and a
        /// materialized workspace with no live claim — what a crash between
        /// Hold A and publication leaves behind.
        fn prepare_abandoned(&self, id: &str) -> PathBuf {
            let prepared = self.prepare(id);
            prepared.workspace.clone()
        }
    }

    impl Drop for ResolveFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn goal_record(id: &str, status: &str, updated: &str) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "id": id,
            "name": id,
            "status": status,
            "priority": "low",
            "reporter": null,
            "branch_name": null,
            "feature_id": null,
            "feature_order": null,
            "node_id": "node-a",
            "created": "2026-08-17T07:00:00Z",
            "updated": updated,
            "notes": [],
            "rounds": []
        }))
        .unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn commit_goal(root: &Path, bytes: Vec<u8>) -> String {
        let path = root.join(GOAL_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        git(root, &["add", "-f", GOAL_PATH]);
        git(root, &["commit", "-q", "-m", "state"]);
        git_stdout(root, &["rev-parse", "HEAD"])
    }

    fn write_resolution(request: &ResolutionRequest<'_>, bytes: Vec<u8>) {
        fs::write(request.workspace_dir.join(GOAL_PATH), bytes).unwrap();
    }

    fn completing(edit: impl FnMut(&ResolutionRequest<'_>) + Send + 'static) -> ScriptedStep {
        let mut edit = edit;
        Box::new(move |request| {
            edit(request);
            Ok(ResolverOutcome::Completed)
        })
    }

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "refine-git-resolve-{name}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn a_clean_agent_edit_resolves_and_records_the_result_ref() {
        let fixture = ResolveFixture::new("clean");
        let prepared = fixture.prepare("clean-1");
        let resolver = ScriptedResolver::new(vec![completing(|request| {
            write_resolution(
                request,
                goal_record("GOALA", "done", "2026-08-17T09:30:00Z"),
            );
        })]);

        let outcome =
            resolve_conflict(&fixture.repo(), &prepared, &resolver, &StateFileGates).unwrap();

        let ResolutionOutcome::Resolved { result_commit, .. } = outcome else {
            panic!("expected Resolved, got {outcome:?}");
        };
        assert_eq!(
            git_stdout(
                &fixture.root,
                &["rev-parse", "refs/refine/resolve/clean-1/result"]
            ),
            result_commit
        );
        let parents = git_stdout(
            &fixture.root,
            &["rev-list", "--parents", "-n", "1", &result_commit],
        );
        assert!(parents.contains(&fixture.ours) && parents.contains(&fixture.theirs));
        let resolved = git_stdout(
            &fixture.root,
            &["show", &format!("{result_commit}:{GOAL_PATH}")],
        );
        assert!(resolved.contains("\"done\""));
        assert!(!resolved.contains("<<<<<<<"));

        // Retiring after publish empties the bounded namespace and takes the
        // workspace with it — one full checkout of the state tree.
        let workspace = prepared.workspace.clone();
        assert!(workspace.exists());
        assert!(
            retire_resolution(
                &fixture.worktrees(),
                &fixture.repo(),
                &fixture.root,
                "clean-1",
                Some(&prepared.lock),
            )
            .unwrap()
        );
        assert!(
            git_stdout(
                &fixture.root,
                &["for-each-ref", "refs/refine/resolve/clean-1"]
            )
            .is_empty()
        );
        assert!(!workspace.exists(), "{}", workspace.display());
    }

    #[test]
    fn a_claimed_resolution_id_is_never_resolved_twice_at_once() {
        let fixture = ResolveFixture::new("claimed");
        let prepared = fixture.prepare("claim-1");

        // A second operation reaching the same divergence derives the same
        // id, and must defer rather than re-derive the workspace underneath
        // the running attempt.
        assert!(
            try_claim_resolution(&fixture.worktrees(), "claim-1")
                .unwrap()
                .is_none()
        );
        // A different divergence is unaffected.
        assert!(
            try_claim_resolution(&fixture.worktrees(), "claim-2")
                .unwrap()
                .is_some()
        );
        // Retiring an id someone else holds is left to that operation.
        assert!(
            !retire_resolution(
                &fixture.worktrees(),
                &fixture.repo(),
                &fixture.root,
                "claim-1",
                None,
            )
            .unwrap()
        );

        // The claim dies with the operation, so a rerun re-takes it.
        drop(prepared);
        assert!(
            try_claim_resolution(&fixture.worktrees(), "claim-1")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn abandoned_resolutions_are_swept_except_the_claimed_one() {
        let fixture = ResolveFixture::new("sweep");
        let prepared = fixture.prepare("sweep-current");
        let abandoned = fixture.prepare_abandoned("sweep-orphan");
        assert!(abandoned.exists());

        let retired = sweep_abandoned_resolutions(
            &fixture.worktrees(),
            &fixture.repo(),
            &fixture.root,
            "sweep-current",
        )
        .unwrap();

        assert_eq!(retired, vec!["sweep-orphan".to_string()]);
        assert!(!abandoned.exists(), "{}", abandoned.display());
        assert!(
            git_stdout(
                &fixture.root,
                &["for-each-ref", "refs/refine/resolve/sweep-orphan"]
            )
            .is_empty()
        );
        // The claimed resolution is untouched.
        assert!(prepared.workspace.exists());
        assert!(
            !git_stdout(
                &fixture.root,
                &["for-each-ref", "refs/refine/resolve/sweep-current"]
            )
            .is_empty()
        );
    }

    #[test]
    fn a_resolver_that_declares_a_question_escalates_with_its_own_words() {
        let fixture = ResolveFixture::new("authored-question");
        let prepared = fixture.prepare("authored-1");
        let resolver = ScriptedResolver::new(vec![
            Box::new(|_request: &ResolutionRequest<'_>| {
                Ok(ResolverOutcome::NeedsDecision {
                    question: "Goal GOALA is in review on this node and done on the other; which \
                               workflow authority holds?"
                        .to_string(),
                })
            }) as ScriptedStep,
            Box::new(
                |_request: &ResolutionRequest<'_>| -> RefineResult<ResolverOutcome> {
                    panic!("a declared decision must not be re-prompted");
                },
            ) as ScriptedStep,
        ]);

        let outcome =
            resolve_conflict(&fixture.repo(), &prepared, &resolver, &StateFileGates).unwrap();

        let ResolutionOutcome::NeedsDecision { question } = outcome else {
            panic!("expected NeedsDecision, got {outcome:?}");
        };
        assert!(
            question.contains("which workflow authority holds?"),
            "{question}"
        );
        assert!(
            !question.contains("Automatic resolution could not produce"),
            "the agent's own question must not be replaced by the template: {question}"
        );
    }

    #[test]
    fn the_agent_prompt_carries_the_context_the_ancestry_and_the_rejection() {
        let workspace = PathBuf::from("/tmp/refine-resolve-prompt");
        let conflicts = Vec::new();
        let sides = Vec::new();
        let request = ResolutionRequest {
            workspace_dir: &workspace,
            conflicts: &conflicts,
            sides: &sides,
            ancestry: "diverged from merge base abc123",
            context: "both nodes changed goal GOALA\n",
            attempt: 2,
            feedback: Some("GOALA is not valid JSON"),
        };

        let prompt = resolution_prompt(&request);

        assert!(prompt.contains("both nodes changed goal GOALA"), "{prompt}");
        assert!(
            prompt.contains("diverged from merge base abc123"),
            "{prompt}"
        );
        assert!(prompt.contains("GOALA is not valid JSON"), "{prompt}");
    }

    #[test]
    fn an_agent_reply_declaring_a_decision_is_read_back_as_its_question() {
        assert_eq!(
            authored_question(
                "I read both records and cannot choose.\nNEEDS DECISION: goal GOALA changed \
                 status on both nodes; which one holds?"
            )
            .as_deref(),
            Some("goal GOALA changed status on both nodes; which one holds?")
        );
        // The ordinary reply of a resolver that did the work claims nothing.
        assert_eq!(authored_question("Resolved both records."), None);
        // A marker with nothing after it is not a question.
        assert_eq!(authored_question("needs decision:   "), None);
    }

    #[test]
    fn an_invalid_first_attempt_is_reprompted_with_feedback_and_the_second_lands() {
        let fixture = ResolveFixture::new("invalid-then-valid");
        let prepared = fixture.prepare("retry-1");
        let seen_feedback = Arc::new(Mutex::new(None::<String>));
        let observed = Arc::clone(&seen_feedback);
        let resolver = ScriptedResolver::new(vec![
            completing(|request| write_resolution(request, b"{not json".to_vec())),
            completing(move |request| {
                *observed.lock().unwrap() = request.feedback.map(str::to_string);
                write_resolution(
                    request,
                    goal_record("GOALA", "done", "2026-08-17T09:30:00Z"),
                );
            }),
        ]);

        let outcome =
            resolve_conflict(&fixture.repo(), &prepared, &resolver, &StateFileGates).unwrap();

        assert!(matches!(outcome, ResolutionOutcome::Resolved { .. }));
        let feedback = seen_feedback.lock().unwrap().clone().unwrap();
        assert!(feedback.contains("not valid JSON"), "{feedback}");
    }

    #[test]
    fn two_invalid_attempts_escalate_with_a_domain_terms_question() {
        let fixture = ResolveFixture::new("invalid-twice");
        let prepared = fixture.prepare("escalate-1");
        let resolver = ScriptedResolver::new(vec![
            completing(|request| write_resolution(request, b"{not json".to_vec())),
            completing(|request| write_resolution(request, b"still not json".to_vec())),
        ]);

        let outcome =
            resolve_conflict(&fixture.repo(), &prepared, &resolver, &StateFileGates).unwrap();

        let ResolutionOutcome::NeedsDecision { question } = outcome else {
            panic!("expected NeedsDecision, got {outcome:?}");
        };
        assert!(question.contains("GOALA"), "{question}");
        assert!(question.contains("status"), "{question}");
        assert!(question.contains("--authority"), "{question}");
        assert!(
            git_stdout(
                &fixture.root,
                &["for-each-ref", "refs/refine/resolve/escalate-1/result"]
            )
            .is_empty()
        );
    }

    #[test]
    fn an_unavailable_resolver_passes_through_without_consuming_attempts() {
        let fixture = ResolveFixture::new("unavailable");
        let prepared = fixture.prepare("unavailable-1");
        let calls = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&calls);
        let resolver =
            ScriptedResolver::new(vec![Box::new(move |_request: &ResolutionRequest<'_>| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(ResolverOutcome::Unavailable)
            }) as ScriptedStep]);

        let outcome =
            resolve_conflict(&fixture.repo(), &prepared, &resolver, &StateFileGates).unwrap();

        assert!(matches!(outcome, ResolutionOutcome::Unavailable));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(
            git_stdout(
                &fixture.root,
                &["for-each-ref", "refs/refine/resolve/unavailable-1/result"]
            )
            .is_empty()
        );
    }

    #[test]
    fn a_rerun_after_a_crash_reuses_pinned_inputs_and_skips_to_the_result() {
        let fixture = ResolveFixture::new("rerun");
        let prepared = fixture.prepare("rerun-1");

        // Crash before any resolver ran: re-pinning the same operands keeps
        // the pins and still reports no result.
        let repinned = pin_resolution_inputs(
            &fixture.repo(),
            &fixture.root,
            "rerun-1",
            &fixture.base,
            &fixture.ours,
            &fixture.theirs,
        )
        .unwrap();
        assert_eq!(repinned.ours, prepared.pinned.ours);
        assert!(repinned.result.is_none());

        let resolver = ScriptedResolver::new(vec![completing(|request| {
            write_resolution(
                request,
                goal_record("GOALA", "done", "2026-08-17T09:30:00Z"),
            );
        })]);
        let outcome =
            resolve_conflict(&fixture.repo(), &prepared, &resolver, &StateFileGates).unwrap();
        let ResolutionOutcome::Resolved { result_commit, .. } = outcome else {
            panic!("expected Resolved, got {outcome:?}");
        };

        // Crash after the result committed: the claim dies with the process,
        // and the rerun finds the pinned result without invoking anything.
        drop(prepared);
        let rerun = fixture.prepare("rerun-1");
        assert_eq!(rerun.pinned.result.as_deref(), Some(result_commit.as_str()));
        let untouched = ScriptedResolver::new(vec![Box::new(
            |_request: &ResolutionRequest<'_>| -> RefineResult<ResolverOutcome> {
                panic!("a surviving result must skip straight to publish");
            },
        ) as ScriptedStep]);
        let outcome =
            resolve_conflict(&fixture.repo(), &rerun, &untouched, &StateFileGates).unwrap();
        let ResolutionOutcome::Resolved {
            result_commit: rerun_commit,
            ..
        } = outcome
        else {
            panic!("expected Resolved on rerun, got {outcome:?}");
        };
        assert_eq!(rerun_commit, result_commit);

        // Stale pins — a head moved — are discarded together with the result.
        git(&fixture.root, &["checkout", "-q", &fixture.theirs]);
        let advanced = commit_goal(
            &fixture.root,
            goal_record("GOALA", "done", "2026-08-17T10:00:00Z"),
        );
        git(&fixture.root, &["checkout", "-q", &fixture.ours]);
        let repinned = pin_resolution_inputs(
            &fixture.repo(),
            &fixture.root,
            "rerun-1",
            &fixture.base,
            &fixture.ours,
            &advanced,
        )
        .unwrap();
        assert_eq!(repinned.theirs, advanced);
        assert!(repinned.result.is_none());
        assert!(
            git_stdout(
                &fixture.root,
                &["for-each-ref", "refs/refine/resolve/rerun-1/result"]
            )
            .is_empty()
        );
    }

    #[test]
    fn leftover_conflict_markers_are_rejected_with_feedback() {
        let fixture = ResolveFixture::new("markers");
        let prepared = fixture.prepare("markers-1");
        let seen_feedback = Arc::new(Mutex::new(None::<String>));
        let observed = Arc::clone(&seen_feedback);
        let resolver = ScriptedResolver::new(vec![
            // "Completed" without touching the workspace: markers remain.
            completing(|_request| {}),
            completing(move |request| {
                *observed.lock().unwrap() = request.feedback.map(str::to_string);
                write_resolution(
                    request,
                    goal_record("GOALA", "review", "2026-08-17T09:00:00Z"),
                );
            }),
        ]);

        let outcome =
            resolve_conflict(&fixture.repo(), &prepared, &resolver, &StateFileGates).unwrap();

        assert!(matches!(outcome, ResolutionOutcome::Resolved { .. }));
        let feedback = seen_feedback.lock().unwrap().clone().unwrap();
        assert!(feedback.contains("conflict markers"), "{feedback}");
    }

    /// Reading the budget must not spend it. The preparation an engagement
    /// needs runs between the two, and it can fail the same way on every pass;
    /// if the check charged, a repeatable preparation failure would hold a
    /// contention no agent has ever been handed.
    #[test]
    fn checking_the_contention_budget_never_spends_it() {
        let fixture = ResolveFixture::new("contention-check");
        let repo = fixture.repo();
        let records = vec!["goals/GOALA/goal.json".to_string()];

        for _ in 0..5 {
            assert!(
                contention_budget_available(&repo, &fixture.root, &fixture.theirs, &records)
                    .unwrap()
            );
        }
        assert!(contention_refs(&fixture.root).is_empty());

        for attempt in 1..=CONTENTION_ATTEMPT_LIMIT {
            assert_eq!(
                buy_contention_attempt(&repo, &fixture.root, &fixture.theirs, &records).unwrap(),
                Some(attempt)
            );
        }
        assert_eq!(
            contention_refs(&fixture.root).len(),
            CONTENTION_ATTEMPT_LIMIT as usize
        );
        assert!(
            !contention_budget_available(&repo, &fixture.root, &fixture.theirs, &records).unwrap()
        );
        assert!(
            buy_contention_attempt(&repo, &fixture.root, &fixture.theirs, &records)
                .unwrap()
                .is_none()
        );

        // Not a fence: a record contended for the first time brings its own
        // budget even while the one beside it is held.
        let both = vec![
            "goals/GOALA/goal.json".to_string(),
            "goals/GOALB/goal.json".to_string(),
        ];
        assert!(contention_budget_available(&repo, &fixture.root, &fixture.theirs, &both).unwrap());
        fs::remove_dir_all(&fixture.root).unwrap();
    }

    fn contention_refs(root: &Path) -> Vec<String> {
        git_stdout(
            root,
            &[
                "for-each-ref",
                "--format=%(refname)",
                CONTENTION_REF_NAMESPACE,
            ],
        )
        .lines()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn the_schema_gate_holds_goal_identity_to_the_record_path() {
        let fixture = ResolveFixture::new("schema");
        let prepared = fixture.prepare("schema-1");
        // Valid JSON, valid Goal shape, wrong identity for the path.
        fs::write(
            prepared.workspace.join(GOAL_PATH),
            goal_record("GOALB", "done", "2026-08-17T09:30:00Z"),
        )
        .unwrap();

        let verdict = StateFileGates
            .review(&prepared.workspace, &prepared.conflicts)
            .unwrap();

        let GateVerdict::Rejected(why) = verdict else {
            panic!("expected rejection, got {verdict:?}");
        };
        assert!(why.contains("GOALB") && why.contains("GOALA"), "{why}");
    }
}

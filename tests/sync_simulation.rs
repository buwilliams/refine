//! Multi-node state-sync simulation harness.
//!
//! Three simulated nodes — each a real clone of one local bare remote with its
//! own `.git/refine-live-state` live dir and a distinct node identity — drive
//! the real in-process sync entry points (`FileGitSyncService`). A scenario is
//! an explicit `Vec<Event>` executed by a scripted scheduler: no wall-clock
//! randomness, no sleeps as synchronization. Each named scenario checks one of
//! the invariants from the persistence-sync design's Testing section; ignored
//! tests are the executable spec of the stage-2 redesign.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use refine::process::supervisor::errors::{RefineError, RefineResult};
use refine::tools::host::git_sync::{
    FileGitSyncService, GitSyncResult, StateRecoveryAuthority, StateRecoveryDecision,
    StateRecoveryRunResult, StateSyncConflictReport, latest_state_sync_conflict_report,
};
use refine::tools::host::project_layout::refine_dir_for_target_root;

#[path = "sync_simulation/scenarios.rs"]
mod scenarios;

/// One scripted step of a simulation scenario. Node indices address the
/// fleet's clones in creation order (0 = node-a, 1 = node-b, 2 = node-c).
#[derive(Clone, Debug)]
enum Event {
    /// Write or advance a goal record in the node's live dir, the way the
    /// daemon persists records mid-workflow. `mutation` becomes the record's
    /// `name` member, so two nodes mutating the same record contest the same
    /// member from their common baseline.
    LiveWrite {
        node: usize,
        goal_id: &'static str,
        mutation: &'static str,
    },
    /// Run the real sync pipeline on the node.
    Sync { node: usize },
    /// Run the one-shot operator recovery on the node with a uniform
    /// authority.
    RecoveryRun {
        node: usize,
        authority: StateRecoveryAuthority,
    },
}

/// What one event produced, in scenario order.
enum Outcome {
    Write,
    Sync(RefineResult<GitSyncResult>),
    Recovery(RefineResult<StateRecoveryRunResult>),
}

impl Outcome {
    fn sync_ok(&self, label: &str) -> &GitSyncResult {
        match self {
            Outcome::Sync(Ok(result)) => result,
            Outcome::Sync(Err(error)) => panic!("{label}: sync failed: {error}"),
            _ => panic!("{label}: outcome is not a sync"),
        }
    }

    fn sync_err(&self, label: &str) -> &RefineError {
        match self {
            Outcome::Sync(Err(error)) => error,
            Outcome::Sync(Ok(result)) => {
                panic!("{label}: sync unexpectedly succeeded: {result:#?}")
            }
            _ => panic!("{label}: outcome is not a sync"),
        }
    }

    fn recovery_ok(&self, label: &str) -> &StateRecoveryRunResult {
        match self {
            Outcome::Recovery(Ok(result)) => result,
            Outcome::Recovery(Err(error)) => panic!("{label}: recovery failed: {error}"),
            _ => panic!("{label}: outcome is not a recovery run"),
        }
    }
}

struct SimulatedNode {
    /// Distinct fleet identity, recorded in the node-local (sync-excluded)
    /// active-node selection exactly where product code reads it.
    id: &'static str,
    target_root: PathBuf,
}

impl SimulatedNode {
    fn service(&self) -> FileGitSyncService {
        FileGitSyncService::new(&self.target_root, self.runtime_root())
    }

    fn runtime_root(&self) -> PathBuf {
        self.target_root.join("run")
    }

    fn live_refine(&self) -> PathBuf {
        refine_dir_for_target_root(&self.target_root).unwrap()
    }

    fn state_head(&self) -> String {
        git_stdout(&self.target_root, &["rev-parse", "refine/state"])
    }
}

struct SimulatedFleet {
    root: PathBuf,
    remote: PathBuf,
    push_block_marker: PathBuf,
    nodes: Vec<SimulatedNode>,
    /// Live writes not yet durably committed by a sync on their node, keyed
    /// by state-relative path. A rewrite before any sync supersedes the
    /// pending version — it was never committed anywhere.
    pending: Vec<BTreeMap<String, Vec<u8>>>,
    /// Every record version a sync attempt actually committed on any node.
    /// The no-lost-work invariant checks each entry stays content-reachable
    /// from the converged head or a retained ref.
    committed: Vec<(String, Vec<u8>)>,
}

impl SimulatedFleet {
    /// Build the bare origin, clone one target root per node id, give each
    /// clone its own live dir and node identity, and bootstrap the
    /// `refine/state` branch: the first node publishes it, the others adopt
    /// it, so every node starts from an agreed baseline.
    fn new(name: &str, node_ids: &[&'static str]) -> Self {
        let root = unique_temp_dir(name);
        let remote = root.join("remote.git");
        let seed = root.join("seed");
        fs::create_dir_all(&seed).unwrap();
        git(&root, &["init", "--bare", remote.to_str().unwrap()]);
        git(&seed, &["init", "-q"]);
        configure_repo(&seed);
        fs::write(seed.join("app.txt"), "base\n").unwrap();
        git(&seed, &["add", "app.txt"]);
        git(&seed, &["commit", "-q", "-m", "initial"]);
        git(&seed, &["branch", "-M", "main"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&seed, &["push", "-q", "-u", "origin", "main"]);
        let push_block_marker = root.join("block-state-pushes");
        install_state_push_gate(&remote, &push_block_marker);

        let nodes = node_ids
            .iter()
            .map(|id| {
                let target_root = root.join(id);
                git(
                    &root,
                    &[
                        "clone",
                        "-q",
                        remote.to_str().unwrap(),
                        target_root.to_str().unwrap(),
                    ],
                );
                configure_repo(&target_root);
                let node = SimulatedNode { id, target_root };
                write_active_node_identity(&node);
                node
            })
            .collect::<Vec<_>>();

        let fleet = Self {
            root,
            remote,
            push_block_marker,
            pending: vec![BTreeMap::new(); nodes.len()],
            committed: Vec::new(),
            nodes,
        };
        let bootstrap = fleet.nodes[0].service().sync().unwrap();
        assert!(bootstrap.pushed, "bootstrap publish failed: {bootstrap:#?}");
        for node in &fleet.nodes[1..] {
            let adopted = node.service().sync().unwrap();
            assert!(adopted.ok, "bootstrap adoption failed: {adopted:#?}");
        }
        fleet
    }

    /// Execute the scripted events in order and return one outcome per event.
    fn run(&mut self, events: &[Event]) -> Vec<Outcome> {
        events.iter().map(|event| self.apply(event)).collect()
    }

    fn apply(&mut self, event: &Event) -> Outcome {
        match event {
            Event::LiveWrite {
                node,
                goal_id,
                mutation,
            } => {
                let relative = goal_record_path(goal_id);
                let bytes = goal_record_bytes(goal_id, mutation);
                let destination = self.nodes[*node].live_refine().join(&relative);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                fs::write(destination, &bytes).unwrap();
                self.pending[*node].insert(relative, bytes);
                Outcome::Write
            }
            Event::Sync { node } => {
                let result = self.nodes[*node].service().sync();
                self.record_durably_committed(*node);
                Outcome::Sync(result)
            }
            Event::RecoveryRun { node, authority } => {
                let result = self.nodes[*node]
                    .service()
                    .run_state_recovery(StateRecoveryDecision::uniform(*authority));
                self.record_durably_committed(*node);
                Outcome::Recovery(result)
            }
        }
    }

    /// Move a node's pending versions into the committed ledger by consulting
    /// what the attempt actually made durable, not whether it returned Ok: a
    /// version is committed exactly when the node's local `refine/state` head
    /// now carries its bytes. A failed pass may still have committed locally
    /// before the publish was rejected — that version must be tracked — while
    /// an Ok skip or deferral committed nothing and must leave it pending.
    fn record_durably_committed(&mut self, node: usize) {
        let root = self.nodes[node].target_root.clone();
        let pending = std::mem::take(&mut self.pending[node]);
        for (relative, bytes) in pending {
            let durable = git_bytes_optional(
                &root,
                &["show", &format!("refine/state:.refine/{relative}")],
            )
            .is_some_and(|shown| shown == bytes);
            if durable {
                self.committed.push((relative, bytes));
            } else {
                self.pending[node].insert(relative, bytes);
            }
        }
    }

    /// Reject `refine/state` pushes at the origin until `allow_state_pushes`,
    /// leaving a node's local state branch strictly ahead of the remote — the
    /// committed-but-unpublished shape behind the ancestor-heads regression.
    fn block_state_pushes(&self) {
        fs::write(&self.push_block_marker, b"").unwrap();
    }

    fn allow_state_pushes(&self) {
        fs::remove_file(&self.push_block_marker).unwrap();
    }

    fn origin_state_head(&self) -> String {
        git_stdout(&self.remote, &["rev-parse", "refine/state"])
    }

    /// Every listed node and the origin share one `refine/state` head; that
    /// head is returned.
    fn assert_converged(&self, label: &str) -> String {
        let origin = self.origin_state_head();
        for node in &self.nodes {
            assert_eq!(
                node.state_head(),
                origin,
                "{label}: {} diverged from the origin state head",
                node.id
            );
        }
        origin
    }

    /// Every record version a successful sync committed on any node is still
    /// content-reachable from the given head or a retained `refs/refine/*`
    /// ref, checked in the given node's repository.
    fn assert_committed_versions_reachable(&self, node: usize, head: &str, label: &str) {
        let root = &self.nodes[node].target_root;
        let mut heads = vec![head.to_string()];
        heads.extend(
            git_stdout(
                root,
                &["for-each-ref", "--format=%(objectname)", "refs/refine"],
            )
            .lines()
            .map(str::to_string),
        );
        let arguments = ["rev-list".to_string()]
            .into_iter()
            .chain(heads)
            .collect::<Vec<_>>();
        let commits = git_stdout(
            root,
            &arguments.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
        for (relative, bytes) in &self.committed {
            let reachable = commits.iter().any(|commit| {
                git_bytes_optional(root, &["show", &format!("{commit}:.refine/{relative}")])
                    .is_some_and(|shown| &shown == bytes)
            });
            assert!(
                reachable,
                "{label}: committed version of {relative} ({}) is not reachable from the converged head or a retained ref",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    /// The record's `name` member as hydrated into the node's live dir.
    fn live_goal_name(&self, node: usize, goal_id: &str) -> String {
        let path = self.nodes[node]
            .live_refine()
            .join(goal_record_path(goal_id));
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())),
        )
        .unwrap();
        record["name"].as_str().unwrap().to_string()
    }

    fn latest_report(&self, node: usize) -> Option<StateSyncConflictReport> {
        latest_state_sync_conflict_report(&self.nodes[node].runtime_root()).unwrap()
    }

    /// The raw conflict-report bytes, for byte-exact "no new report appeared"
    /// comparisons that no serialization round-trip can weaken.
    fn raw_report_bytes(&self, node: usize) -> Option<Vec<u8>> {
        fs::read(
            self.nodes[node]
                .runtime_root()
                .join("state-sync-conflicts/latest.json"),
        )
        .ok()
    }
}

impl Drop for SimulatedFleet {
    fn drop(&mut self) {
        // A failing scenario's panic message points into this tree (repos,
        // conflict reports); keep the evidence when unwinding.
        if std::thread::panicking() {
            return;
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The daemon shards goal records as `goals/<id[..2]>/<id[2..]>/goal.json`.
fn goal_record_path(goal_id: &str) -> String {
    format!("goals/{}/{}/goal.json", &goal_id[..2], &goal_id[2..])
}

/// A minimal goal record in the shape the crate's sync fixtures persist. It
/// deliberately omits required Goal schema fields, so a contested member
/// cannot be settled by the semantic merge and falls through to the
/// fail-closed conflict report — the genuine two-sided divergence the
/// terminal-recovery and stable-identity scenarios need.
fn goal_record_bytes(goal_id: &str, name: &str) -> Vec<u8> {
    format!(r#"{{"id":"{goal_id}","name":"{name}","status":"todo","rounds":[]}}"#).into_bytes()
}

/// Record the machine-local active-node selection where identity resolution
/// reads it (`runtime/` is excluded from durable state, so identities never
/// replicate between simulated nodes).
fn write_active_node_identity(node: &SimulatedNode) {
    let live_refine = node.live_refine();
    let runtime_local = live_refine.join("runtime");
    fs::create_dir_all(&runtime_local).unwrap();
    fs::write(
        runtime_local.join("active-node.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "active_node_id": node.id,
            "refine_dir": live_refine.display().to_string(),
            "updated_at": "2026-08-17T00:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
}

/// Origin-side pre-receive hook that rejects `refine/state` pushes while the
/// marker file exists, so a scenario can hold a node in the
/// committed-but-unpublished state deterministically.
fn install_state_push_gate(remote: &Path, marker: &Path) {
    let hook = remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nwhile read old new ref; do\n  if test \"$ref\" = refs/heads/refine/state && test -e \"{}\"; then\n    exit 1\n  fi\ndone\nexit 0\n",
            marker.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn configure_repo(root: &Path) {
    git(root, &["config", "user.email", "sync-simulation@test"]);
    git(root, &["config", "user.name", "Sync Simulation"]);
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {}: {error}", args.join(" ")));
    assert!(
        output.status.success(),
        "git {} failed in {}\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed in {}: {}",
        args.join(" "),
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn git_bytes_optional(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "refine-sync-simulation-{name}-{}-{nanos}",
        std::process::id()
    ))
}

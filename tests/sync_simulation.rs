//! Multi-node state-sync simulation harness.
//!
//! Three simulated nodes — each a real clone of one local bare remote with its
//! own `.git/refine-live-state` live dir and a distinct node identity — drive
//! the real in-process sync entry points (`FileGitSyncService`). A scenario is
//! an explicit `Vec<Event>` executed by a scripted scheduler: no wall-clock
//! randomness, no sleeps as synchronization. Each named scenario checks one of
//! the invariants from the persistence-sync design's Testing section; together
//! they are the stage-2 acceptance gate.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use refine::process::supervisor::errors::{RefineError, RefineResult};
use refine::tools::git::resolve::{
    CONTENTION_ATTEMPT_LIMIT, ResolutionRequest, ResolverOutcome, ResolverOverrideGuard,
    ScriptedResolver, ScriptedStep, install_resolver_override,
};
use refine::tools::host::git_sync::{
    FileGitSyncService, GitSyncResult, StateRecoveryAuthority, StateRecoveryDecision,
    StateRecoveryRunResult, StateSyncConflictReport, latest_state_sync_conflict_report,
};
use refine::tools::host::project_layout::refine_dir_for_target_root;

#[path = "sync_simulation/rolling_upgrade.rs"]
mod rolling_upgrade;
#[path = "sync_simulation/scenarios.rs"]
mod scenarios;

/// Which machinery a simulated node runs. A fleet upgrades node by node, so
/// both kinds publish to one remote at the same time and every convergence
/// invariant has to hold across the mixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NodeKind {
    /// The shipped pipeline: `FileGitSyncService`, ancestry classification,
    /// merge commits, agent resolution.
    Current,
    /// A node that has not been upgraded yet. Its behavior is emulated
    /// mechanically with git — fetch, rebase the local state branch onto the
    /// remote, commit the live delta as one linear commit, push — rather than
    /// by keeping the retired code alive. What matters for compatibility is
    /// the shape a pre-upgrade node leaves on the shared branch, not which
    /// code produced it.
    Legacy,
}

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
    /// Write a goal record with explicit `name` and `status` members, so two
    /// nodes can change *different* members of the same record from a shared
    /// base — the provably disjoint case the structural driver must settle.
    LiveWriteMembers {
        node: usize,
        goal_id: &'static str,
        name: &'static str,
        status: &'static str,
    },
    /// Run the real sync pipeline on the node.
    Sync { node: usize },
    /// Run one pre-upgrade synchronization pass on a not-yet-upgraded node.
    LegacySync { node: usize },
    /// Upgrade a node in place: nothing durable changes, and its next `Sync`
    /// runs the current pipeline over exactly the state — branch, worktree,
    /// live store, and legacy leftovers — the pre-upgrade machinery left.
    Upgrade { node: usize },
    /// Write a goal record carrying an explicit `node_id` owner member, so a
    /// merge-base ownership decision can read who owned the record at the
    /// shared base.
    LiveWriteOwned {
        node: usize,
        goal_id: &'static str,
        name: &'static str,
        owner: &'static str,
    },
    /// Run the one-shot operator recovery on the node with a uniform
    /// authority.
    RecoveryRun {
        node: usize,
        authority: StateRecoveryAuthority,
    },
    /// Simulate a daemon crash on the node: every piece of in-process service
    /// state is dropped and rebuilt — the resolver the daemon had installed
    /// dies with it, and the node value is reconstructed from its identity
    /// and path alone. Durable state (target root repository, live dir,
    /// refs) is untouched: crash-only means the rerun must re-derive
    /// everything from it. (`FileGitSyncService` itself is stateless and is
    /// already constructed fresh per event; the crash-window *placement*
    /// comes from induced failures — a rejected push, an unwritable
    /// hydration destination, a resolver that dies mid-attempt — that stop a
    /// pass exactly where the process would have died.)
    Crash { node: usize },
}

/// What one event produced, in scenario order.
enum Outcome {
    Write,
    Crash,
    Upgrade,
    Sync(RefineResult<GitSyncResult>),
    LegacySync(LegacyPass),
    Recovery(RefineResult<StateRecoveryRunResult>),
}

/// What one pre-upgrade pass did. A legacy pass has no typed result to
/// return, so the harness reports the three mechanical facts a scenario can
/// assert on.
#[derive(Clone, Debug, Default)]
struct LegacyPass {
    /// The pass replayed this node's local commits onto the remote head.
    rebased: bool,
    /// The live delta became one new linear commit.
    committed: bool,
    /// The linear commit reached the origin.
    pushed: bool,
    /// Why git refused, when the rebase or the push lost.
    rejected: Option<String>,
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

    fn legacy(&self, label: &str) -> &LegacyPass {
        match self {
            Outcome::LegacySync(pass) => pass,
            _ => panic!("{label}: outcome is not a legacy sync"),
        }
    }

    /// A legacy pass that published: nothing was rejected, and the branch
    /// this node pushed is the branch the origin now carries.
    fn legacy_published(&self, label: &str) -> &LegacyPass {
        let pass = self.legacy(label);
        assert!(
            pass.pushed && pass.rejected.is_none(),
            "{label}: legacy pass did not publish: {pass:#?}"
        );
        pass
    }
}

struct SimulatedNode {
    /// Distinct fleet identity, recorded in the node-local (sync-excluded)
    /// active-node selection exactly where product code reads it.
    id: &'static str,
    target_root: PathBuf,
    /// Whether this node has been upgraded yet.
    kind: NodeKind,
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

    /// The isolated checkout of `refine/state` both machineries own, at the
    /// one path the product has always used — an upgraded node inherits the
    /// pre-upgrade node's worktree instead of creating a second one.
    fn state_worktree(&self) -> PathBuf {
        self.target_root.join(".git/refine-state-worktree")
    }

    /// The retired baseline fingerprint file, where the pre-upgrade
    /// machinery kept it.
    fn legacy_baseline_file(&self) -> PathBuf {
        self.target_root.join(".git/refine-state-baseline.json")
    }

    /// The retired baseline anchor refs still present in this repository.
    fn legacy_baseline_refs(&self) -> Vec<String> {
        git_stdout(
            &self.target_root,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/refine/state-baseline",
            ],
        )
        .lines()
        .map(str::to_string)
        .collect()
    }

    /// Every durable path the node's state branch carries, so a scenario can
    /// assert an exact file set instead of only the records it looked up.
    fn state_tree_paths(&self) -> Vec<String> {
        git_stdout(
            &self.target_root,
            &[
                "ls-tree",
                "-r",
                "--name-only",
                "refine/state",
                "--",
                ".refine",
            ],
        )
        .lines()
        .filter_map(|path| path.strip_prefix(".refine/").map(str::to_string))
        .collect()
    }
}

struct SimulatedFleet {
    root: PathBuf,
    remote: PathBuf,
    push_block_marker: PathBuf,
    push_race_marker: PathBuf,
    nodes: Vec<SimulatedNode>,
    /// The scripted conflict resolver installed on each node, if any — the
    /// injection point the wiring exposed for the harness. Held as a guard so
    /// a crash (or the fleet's end) uninstalls it.
    resolvers: Vec<Option<ResolverOverrideGuard>>,
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
        let push_race_marker = root.join("race-state-push");
        install_state_push_gate(&remote, &push_block_marker, &push_race_marker);

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
                let node = SimulatedNode {
                    id,
                    target_root,
                    kind: NodeKind::Current,
                };
                write_active_node_identity(&node);
                node
            })
            .collect::<Vec<_>>();

        let fleet = Self {
            root,
            remote,
            push_block_marker,
            push_race_marker,
            resolvers: (0..nodes.len()).map(|_| None).collect(),
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
            } => self.write_goal(*node, goal_id, goal_record_bytes(goal_id, mutation)),
            Event::LiveWriteMembers {
                node,
                goal_id,
                name,
                status,
            } => self.write_goal(
                *node,
                goal_id,
                goal_record_bytes_with(goal_id, name, status),
            ),
            Event::LiveWriteOwned {
                node,
                goal_id,
                name,
                owner,
            } => self.write_goal(
                *node,
                goal_id,
                goal_record_bytes_owned(goal_id, name, owner),
            ),
            Event::Crash { node } => {
                // The resolver the crashed daemon had installed dies with the
                // process; the node's in-process value is rebuilt from its
                // durable identity and path. Nothing under the target root is
                // touched.
                self.resolvers[*node] = None;
                let id = self.nodes[*node].id;
                let kind = self.nodes[*node].kind;
                self.nodes[*node] = SimulatedNode {
                    id,
                    target_root: self.root.join(id),
                    kind,
                };
                Outcome::Crash
            }
            Event::Sync { node } => {
                assert_eq!(
                    self.nodes[*node].kind,
                    NodeKind::Current,
                    "{}: the current pipeline runs only on an upgraded node",
                    self.nodes[*node].id
                );
                let result = self.nodes[*node].service().sync();
                self.record_durably_committed(*node);
                Outcome::Sync(result)
            }
            Event::LegacySync { node } => {
                assert_eq!(
                    self.nodes[*node].kind,
                    NodeKind::Legacy,
                    "{}: the pre-upgrade pass runs only on a node that has not been upgraded",
                    self.nodes[*node].id
                );
                let pass = self.legacy_sync(*node);
                self.record_durably_committed(*node);
                Outcome::LegacySync(pass)
            }
            Event::Upgrade { node } => {
                assert_eq!(
                    self.nodes[*node].kind,
                    NodeKind::Legacy,
                    "{}: the node is already upgraded",
                    self.nodes[*node].id
                );
                self.nodes[*node].kind = NodeKind::Current;
                Outcome::Upgrade
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

    fn write_goal(&mut self, node: usize, goal_id: &str, bytes: Vec<u8>) -> Outcome {
        let relative = goal_record_path(goal_id);
        let destination = self.nodes[node].live_refine().join(&relative);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, &bytes).unwrap();
        self.pending[node].insert(relative, bytes);
        Outcome::Write
    }

    /// Clone one more target root from the origin without syncing it, the way
    /// a node joins an already published fleet. Returns its index.
    fn add_node(&mut self, id: &'static str) -> usize {
        self.add_node_of_kind(id, NodeKind::Current)
    }

    /// Join a node that has not been upgraded yet: same clone, same live
    /// store, same identity — only its synchronization machinery differs.
    fn add_legacy_node(&mut self, id: &'static str) -> usize {
        self.add_node_of_kind(id, NodeKind::Legacy)
    }

    fn add_node_of_kind(&mut self, id: &'static str, kind: NodeKind) -> usize {
        let target_root = self.root.join(id);
        git(
            &self.root,
            &[
                "clone",
                "-q",
                self.remote.to_str().unwrap(),
                target_root.to_str().unwrap(),
            ],
        );
        configure_repo(&target_root);
        let node = SimulatedNode {
            id,
            target_root,
            kind,
        };
        write_active_node_identity(&node);
        self.nodes.push(node);
        self.resolvers.push(None);
        self.pending.push(BTreeMap::new());
        self.nodes.len() - 1
    }

    /// One pre-upgrade synchronization pass, emulated mechanically with git:
    /// fetch, rebase this node's `refine/state` onto the remote head, commit
    /// the live delta as one linear commit, push, then hydrate the branch
    /// back into the live store. It never creates a merge commit, never
    /// classifies ancestry, and never deletes a path it did not observe —
    /// the pre-upgrade machinery published a delta, not a mirror.
    ///
    /// The pass writes the retired baseline leftovers where that machinery
    /// wrote them (`.git/refine-state-baseline.json` plus a
    /// `refs/refine/state-baseline/<oid>` anchor) and shares the one state
    /// worktree path, so upgrading the node exercises inheriting both.
    fn legacy_sync(&mut self, node: usize) -> LegacyPass {
        let root = self.nodes[node].target_root.clone();
        let id = self.nodes[node].id;
        let live = self.nodes[node].live_refine();
        let worktree = self.nodes[node].state_worktree();
        let worktree_state = worktree.join(".refine");
        let remote_ref = "origin/refine/state";

        git(&root, &["fetch", "-q", "--prune", "origin"]);
        let remote_exists = git_try(
            &root,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/remotes/{remote_ref}"),
            ],
        )
        .0;
        let local_exists = git_try(
            &root,
            &["show-ref", "--verify", "--quiet", "refs/heads/refine/state"],
        )
        .0;
        if !local_exists && remote_exists {
            git(&root, &["branch", "--track", "refine/state", remote_ref]);
        }
        if !worktree.exists() {
            let path = worktree.to_str().unwrap().to_string();
            if local_exists || remote_exists {
                git(&root, &["worktree", "add", "-q", &path, "refine/state"]);
            } else {
                git(&root, &["worktree", "add", "-q", "--detach", &path, "HEAD"]);
                git(&worktree, &["switch", "--orphan", "refine/state"]);
                git(&worktree, &["rm", "-rf", "--ignore-unmatch", "."]);
            }
        }

        let mut pass = LegacyPass::default();
        if remote_exists {
            let (ok, output) = git_try(&worktree, &["rebase", remote_ref]);
            if !ok {
                let _ = git_try(&worktree, &["rebase", "--abort"]);
                pass.rejected = Some(output);
                return pass;
            }
            pass.rebased = true;
        }

        // Publish only what this node changed since the state it last agreed
        // with. The retired machinery kept its fingerprint file for exactly
        // this: a live copy that still matches the baseline is not a local
        // edit, and republishing it would resurrect a version the fleet has
        // already moved past.
        let baseline = self.read_legacy_baseline(node);
        let mut published = Vec::new();
        for relative in durable_state_paths(&live) {
            let bytes = fs::read(live.join(&relative)).unwrap();
            if baseline.get(&relative).map(String::as_str)
                == Some(state_fingerprint(&bytes).as_str())
            {
                continue;
            }
            let destination = worktree_state.join(&relative);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(&destination, &bytes).unwrap();
            published.push(relative);
        }
        git(&worktree, &["add", "-f", "-A", "--", ".refine"]);
        let staged = git_stdout(
            &worktree,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=no",
                "--",
                ".refine",
            ],
        );
        if !staged.is_empty() {
            git(
                &worktree,
                &[
                    "commit",
                    "-q",
                    "-m",
                    "Update Refine state",
                    "-m",
                    &format!("Node: {id}"),
                ],
            );
            pass.committed = true;
        }
        let (pushed, output) = git_try(&worktree, &["push", "-q", "origin", "refine/state"]);
        pass.pushed = pushed;
        if !pushed {
            pass.rejected = Some(output);
        }

        // Hydrate everything the branch carries that this pass did not
        // publish itself. Records are copied in, never removed: a
        // pre-upgrade pass moved a delta in both directions and had no way to
        // express a deletion it had not observed.
        for relative in durable_state_paths(&worktree_state) {
            if published.contains(&relative) {
                continue;
            }
            let bytes = fs::read(worktree_state.join(&relative)).unwrap();
            let destination = live.join(&relative);
            if fs::read(&destination).ok().as_deref() == Some(bytes.as_slice()) {
                continue;
            }
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::write(&destination, &bytes).unwrap();
        }
        self.write_legacy_baseline(node, &worktree_state);
        pass
    }

    /// The fingerprints of the state this node last agreed with, as the
    /// retired machinery recorded them.
    fn read_legacy_baseline(&self, node: usize) -> BTreeMap<String, String> {
        let Ok(bytes) = fs::read(self.nodes[node].legacy_baseline_file()) else {
            return BTreeMap::new();
        };
        let stored: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        stored
            .get("fingerprints")
            .and_then(serde_json::Value::as_object)
            .map(|fingerprints| {
                fingerprints
                    .iter()
                    .filter_map(|(path, value)| {
                        value
                            .as_str()
                            .map(|value| (path.clone(), value.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Record the pre-upgrade baseline fingerprint file and its retained
    /// anchor ref, the leftovers an upgraded node must retire.
    fn write_legacy_baseline(&self, node: usize, worktree_state: &Path) {
        let root = &self.nodes[node].target_root;
        let head = git_stdout(root, &["rev-parse", "refine/state"]);
        let fingerprints = durable_state_paths(worktree_state)
            .into_iter()
            .map(|relative| {
                let bytes = fs::read(worktree_state.join(&relative)).unwrap_or_default();
                let fingerprint = state_fingerprint(&bytes);
                (relative, serde_json::json!(fingerprint))
            })
            .collect::<serde_json::Map<_, _>>();
        let snapshot_ref = format!("refs/refine/state-baseline/{head}");
        fs::write(
            self.nodes[node].legacy_baseline_file(),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 2,
                "snapshot_oid": head,
                "snapshot_ref": snapshot_ref,
                "fingerprints": fingerprints
            }))
            .unwrap(),
        )
        .unwrap();
        git(root, &["update-ref", &snapshot_ref, &head]);
    }

    /// Install a scripted conflict resolver on the node, the way the daemon
    /// would carry an installed agent. Replaces any previous one; a
    /// `Crash { node }` event uninstalls it.
    fn install_resolver(&mut self, node: usize, steps: Vec<ScriptedStep>) {
        self.resolvers[node] = Some(install_resolver_override(
            &self.nodes[node].target_root,
            Arc::new(ScriptedResolver::new(steps)),
        ));
    }

    /// Every ref currently pinned under the node's bounded resolve namespace.
    fn resolve_ref_names(&self, node: usize) -> Vec<String> {
        git_stdout(
            &self.nodes[node].target_root,
            &["for-each-ref", "--format=%(refname)", "refs/refine/resolve"],
        )
        .lines()
        .map(str::to_string)
        .collect()
    }

    /// Every resolution workspace and lock still on disk for the node. A
    /// workspace is a full checkout of the state tree, so one left behind per
    /// divergence is the disk-growth class this capability must not have.
    fn resolve_workspace_names(&self, node: usize) -> Vec<String> {
        let root = self.nodes[node].target_root.join(".git/refine-resolve");
        let Ok(entries) = fs::read_dir(&root) else {
            return Vec::new();
        };
        let mut names = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
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

    /// Publish a node's local `refine/state` head to the origin's staging ref
    /// `refs/race/pending` without touching the state branch, so the race gate
    /// can later advance the branch to it mid-push.
    fn stage_race_commit(&self, node: usize) -> String {
        git(
            &self.nodes[node].target_root,
            &["push", "-q", "origin", "refine/state:refs/race/pending"],
        );
        git_stdout(&self.remote, &["rev-parse", "refs/race/pending"])
    }

    /// Arm the origin so the NEXT `refine/state` push loses a race: the
    /// pre-receive hook advances the branch to the staged commit and rejects
    /// the push, exactly as if a third node had published between the pushing
    /// pass's fetch and its push. One-shot: the hook consumes the marker.
    fn arm_state_push_race(&self) {
        fs::write(&self.push_race_marker, b"").unwrap();
    }

    fn state_push_race_armed(&self) -> bool {
        self.push_race_marker.exists()
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
        self.live_goal_member(node, goal_id, "name")
    }

    /// One string member of the record as hydrated into the node's live dir.
    fn live_goal_member(&self, node: usize, goal_id: &str, member: &str) -> String {
        let path = self.nodes[node]
            .live_refine()
            .join(goal_record_path(goal_id));
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(&path)
                .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())),
        )
        .unwrap();
        record[member].as_str().unwrap().to_string()
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

/// Evidence-preserving temp dir for scenarios that build repos outside a
/// fleet, with the same panicking-keeps-evidence discipline.
struct EvidenceDir(PathBuf);

impl EvidenceDir {
    fn new(name: &str) -> Self {
        let root = unique_temp_dir(name);
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for EvidenceDir {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let _ = fs::remove_dir_all(&self.0);
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
    goal_record_bytes_with(goal_id, name, "todo")
}

fn goal_record_bytes_with(goal_id: &str, name: &str, status: &str) -> Vec<u8> {
    format!(r#"{{"id":"{goal_id}","name":"{name}","status":"{status}","rounds":[]}}"#).into_bytes()
}

/// A minimal record carrying an explicit `node_id` owner, so the merge base
/// can testify who owned it when a merge-base ownership decision asks.
fn goal_record_bytes_owned(goal_id: &str, name: &str, owner: &str) -> Vec<u8> {
    format!(
        r#"{{"id":"{goal_id}","name":"{name}","status":"todo","node_id":"{owner}","rounds":[]}}"#
    )
    .into_bytes()
}

/// A complete, schema-valid Goal record — what an accepted agent resolution
/// must produce, because the state acceptance gates deserialize the resolved
/// bytes as a Goal and hold its id to the record path.
fn full_goal_record_bytes(goal_id: &str, name: &str) -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "id": goal_id,
        "name": name,
        "status": "todo",
        "priority": "low",
        "reporter": null,
        "branch_name": null,
        "feature_id": null,
        "feature_order": null,
        "node_id": "node-a",
        "created": "2026-08-17T07:00:00Z",
        "updated": "2026-08-17T09:00:00Z",
        "notes": [],
        "rounds": []
    }))
    .unwrap()
}

/// A scripted resolver step that edits the workspace and claims completion —
/// the acceptance gates decide whether the claim holds.
fn completing(edit: impl FnMut(&ResolutionRequest<'_>) + Send + 'static) -> ScriptedStep {
    let mut edit = edit;
    Box::new(move |request| {
        edit(request);
        Ok(ResolverOutcome::Completed)
    })
}

/// A script of `count` identical steps that answer the same way every time
/// and count the call — the spend meter contention scenarios read. The script
/// is deliberately longer than any bounded run may spend, so an unbounded one
/// is measured rather than cut short by an exhausted script.
fn counting_script(
    calls: &Arc<AtomicU32>,
    count: usize,
    answer: fn() -> ResolverOutcome,
) -> Vec<ScriptedStep> {
    (0..count)
        .map(|_| {
            let counted = Arc::clone(calls);
            Box::new(move |_request: &ResolutionRequest<'_>| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok(answer())
            }) as ScriptedStep
        })
        .collect()
}

/// A scripted resolver step that resolves one contested goal record by
/// writing a complete Goal whose `name` composes both sides' intents.
fn resolving_goal(goal_id: &'static str, merged_name: &'static str) -> ScriptedStep {
    completing(move |request| {
        let destination = request
            .workspace_dir
            .join(".refine")
            .join(goal_record_path(goal_id));
        fs::write(destination, full_goal_record_bytes(goal_id, merged_name)).unwrap();
    })
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

/// Origin-side pre-receive hook with two deterministic gates for
/// `refine/state` pushes: while the block marker exists every push is
/// rejected (the committed-but-unpublished shape), and while the race marker
/// exists exactly one push is rejected AFTER the hook itself advances the
/// branch to the staged `refs/race/pending` commit — the remote moving
/// between a pass's fetch and its push. `update-ref` runs outside the push's
/// quarantine because the staged commit's objects were fully received by an
/// earlier, completed push.
fn install_state_push_gate(remote: &Path, block_marker: &Path, race_marker: &Path) {
    let hook = remote.join("hooks/pre-receive");
    fs::write(
        &hook,
        format!(
            concat!(
                "#!/bin/sh\n",
                "while read old new ref; do\n",
                "  if test \"$ref\" = refs/heads/refine/state; then\n",
                "    if test -e \"{block}\"; then\n",
                "      exit 1\n",
                "    fi\n",
                "    if test -e \"{race}\"; then\n",
                "      env -u GIT_QUARANTINE_PATH git update-ref refs/heads/refine/state \"$(git rev-parse refs/race/pending)\"\n",
                "      rm -f \"{race}\"\n",
                "      exit 1\n",
                "    fi\n",
                "  fi\n",
                "done\n",
                "exit 0\n",
            ),
            block = block_marker.display(),
            race = race_marker.display()
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

/// Run git without asserting success: the pre-upgrade pass has to observe a
/// rejected push or a failed rebase the way the old binary did.
fn git_try(root: &Path, args: &[&str]) -> (bool, String) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {}: {error}", args.join(" ")));
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), text.trim().to_string())
}

/// Every durable state path under `root`, relative and slash-separated.
fn durable_state_paths(root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    collect_durable_state_paths(root, root, &mut paths);
    paths.sort();
    paths
}

fn collect_durable_state_paths(root: &Path, current: &Path, paths: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        if excluded_from_durable_state(&relative) {
            continue;
        }
        if path.is_dir() {
            collect_durable_state_paths(root, &path, paths);
        } else {
            paths.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// A content fingerprint in the shape the retired baseline file stored one:
/// stable per bytes, and never compared across anything but itself.
fn state_fingerprint(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The product's durable-state exclusions, as they apply to the files this
/// harness creates: node-local evidence directories and in-flight scratch
/// files never reach the branch (`src/tools/host/git_sync/state_files.rs`).
fn excluded_from_durable_state(relative: &Path) -> bool {
    let leading = relative
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str());
    if matches!(
        leading,
        Some("run" | "runtime" | "logs" | "support-bundles" | "provider-bin")
    ) {
        return true;
    }
    let name = relative
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    name.ends_with(".lock") || name.ends_with(".tmp") || name.starts_with(".refine-sync-")
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

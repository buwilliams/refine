//! Per-node fleet synchronization status.
//!
//! A fleet upgrades node by node, in any order, so at any moment some nodes
//! run the current build and some still run the previous one. The shared
//! `refine/state` branch handles that by itself — both machineries publish
//! commits the other can read. The one place the version difference is
//! visible is the daemon API: a node that has not been upgraded yet rejects
//! this build's contract version with `426`, and that is a fact about THAT
//! node, not about the fleet. It is reported as the node's `pending_upgrade`
//! status; every other node still syncs and the command still succeeds.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::application::fleet::service::FileFleetService;
use crate::application::protocol::API_CONTRACT_VERSION;
use crate::error::RefineResult;
use crate::model::node::Node;

/// The node's daemon accepted the synchronization request and queued its own
/// pass. It is a receipt for the request, not a verdict on the reconciliation:
/// the pass runs on that node, under that node's own lock, and reports its
/// result through that node's own sync surface and state-sync health. Waiting
/// for it here would hold the whole fan-out behind one node's agent
/// resolution, which takes minutes by design.
pub const NODE_SYNC_QUEUED: &str = "queued";
/// The node's daemon is still on the previous build: it rejected this build's
/// API contract version. It keeps working and keeps converging over the state
/// branch; it simply has not been upgraded yet.
pub const NODE_SYNC_PENDING_UPGRADE: &str = "pending_upgrade";
/// No daemon answered at the node's recorded address.
pub const NODE_SYNC_UNREACHABLE: &str = "unreachable";
/// A daemon answered, and answered an error.
pub const NODE_SYNC_FAILED: &str = "failed";
/// The node answered, and answered that its Git is too old to synchronize
/// state at all. Like `pending_upgrade` this is that NODE's condition and
/// never the fleet's: every other node keeps converging while an operator
/// upgrades Git on this one.
pub const NODE_SYNC_UNSUPPORTED_GIT: &str = "unsupported_git";
/// The node is disabled, so it was not contacted.
pub const NODE_SYNC_DISABLED: &str = "disabled";
/// This machine's own node, synchronized in-process by the calling pass
/// rather than over the API.
pub const NODE_SYNC_LOCAL: &str = "local";

const NODE_DAEMON_TIMEOUT: Duration = Duration::from_secs(2);

/// One node's outcome from a fleet synchronization pass.
#[derive(Clone, Debug, Serialize)]
pub struct FleetNodeSyncStatus {
    pub node_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The API contract version the node's daemon reported, when it reported
    /// one — the "2" against this build's "3".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_contract_version: Option<String>,
    /// The contract version this build speaks.
    pub expected_api_contract_version: String,
}

impl FleetNodeSyncStatus {
    fn new(node_id: &str, status: &str, detail: Option<String>) -> Self {
        Self {
            node_id: node_id.to_string(),
            status: status.to_string(),
            detail,
            api_contract_version: None,
            expected_api_contract_version: API_CONTRACT_VERSION.to_string(),
        }
    }

    pub fn pending_upgrade(&self) -> bool {
        self.status == NODE_SYNC_PENDING_UPGRADE
    }
}

/// What a fleet synchronization pass observed across the fleet. The report is
/// per node by construction: no single node's condition can stand in for the
/// fleet's.
#[derive(Clone, Debug, Serialize)]
pub struct FleetNodeSyncReport {
    pub nodes: Vec<FleetNodeSyncStatus>,
    /// Nodes that have not been upgraded yet, in registry order. They are
    /// reported, never failed: their state converges over `refine/state`
    /// until each is upgraded in turn.
    pub pending_upgrade: Vec<String>,
}

impl FleetNodeSyncReport {
    fn new(nodes: Vec<FleetNodeSyncStatus>) -> Self {
        let pending_upgrade = nodes
            .iter()
            .filter(|node| node.pending_upgrade())
            .map(|node| node.node_id.clone())
            .collect();
        Self {
            nodes,
            pending_upgrade,
        }
    }
}

/// What one node's daemon answered when asked to synchronize.
#[derive(Clone, Debug)]
pub enum NodeDaemonReply {
    /// The daemon answered with an HTTP status and a JSON body.
    Answered {
        status: u16,
        body: serde_json::Value,
    },
    /// Nothing answered at the node's address.
    Unreachable(String),
}

/// How the fleet reaches one node's daemon. The trait exists so the fleet's
/// classification of a reply is tested without a network.
pub trait FleetNodeDaemonClient: Send + Sync {
    fn sync_node(&self, node: &Node) -> NodeDaemonReply;
}

type SharedNodeDaemonClient = Arc<dyn FleetNodeDaemonClient>;

/// The shipped binary always talks real HTTP. The override registry below
/// exists only so the classification of a reply is tested without a network,
/// and nothing outside this crate's tests reaches it, so it is compiled out
/// of the binary rather than living in it as an unreachable seam.
#[cfg(not(test))]
fn node_daemon_client(_refine_dir: &Path) -> SharedNodeDaemonClient {
    Arc::new(HttpNodeDaemonClient)
}

#[cfg(test)]
fn node_daemon_client_overrides() -> &'static Mutex<BTreeMap<PathBuf, SharedNodeDaemonClient>> {
    static OVERRIDES: OnceLock<Mutex<BTreeMap<PathBuf, SharedNodeDaemonClient>>> = OnceLock::new();
    OVERRIDES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
fn node_daemon_client_key(refine_dir: &Path) -> PathBuf {
    refine_dir
        .canonicalize()
        .unwrap_or_else(|_| refine_dir.to_path_buf())
}

/// Uninstalls its project's node-daemon client override when dropped.
#[cfg(test)]
pub struct FleetNodeDaemonClientGuard {
    key: PathBuf,
}

#[cfg(test)]
impl Drop for FleetNodeDaemonClientGuard {
    fn drop(&mut self) {
        if let Ok(mut overrides) = node_daemon_client_overrides().lock() {
            overrides.remove(&self.key);
        }
    }
}

/// Route this project's node-daemon requests to the given client instead of
/// real HTTP. Keyed by Refine directory so concurrent tests never observe
/// each other's client.
#[cfg(test)]
pub fn install_node_daemon_client(
    refine_dir: &Path,
    client: SharedNodeDaemonClient,
) -> FleetNodeDaemonClientGuard {
    let key = node_daemon_client_key(refine_dir);
    node_daemon_client_overrides()
        .lock()
        .expect("node daemon client override registry poisoned")
        .insert(key.clone(), client);
    FleetNodeDaemonClientGuard { key }
}

#[cfg(test)]
fn node_daemon_client(refine_dir: &Path) -> SharedNodeDaemonClient {
    node_daemon_client_overrides()
        .lock()
        .ok()
        .and_then(|overrides| overrides.get(&node_daemon_client_key(refine_dir)).cloned())
        .unwrap_or_else(|| Arc::new(HttpNodeDaemonClient))
}

/// The real transport: one bounded HTTP request to the node's daemon,
/// carrying this build's API contract version so a node still on the previous
/// build answers `426` instead of acting on a request it does not understand.
pub struct HttpNodeDaemonClient;

impl FleetNodeDaemonClient for HttpNodeDaemonClient {
    fn sync_node(&self, node: &Node) -> NodeDaemonReply {
        let host = node.ssh_host.trim();
        let authority = format!("{host}:{}", node.refine_port);
        let address = match authority.to_socket_addrs() {
            Ok(mut addresses) => match addresses.next() {
                Some(address) => address,
                None => {
                    return NodeDaemonReply::Unreachable(format!(
                        "{authority} resolved to no address"
                    ));
                }
            },
            Err(error) => return NodeDaemonReply::Unreachable(format!("{authority}: {error}")),
        };
        let mut stream = match TcpStream::connect_timeout(&address, NODE_DAEMON_TIMEOUT) {
            Ok(stream) => stream,
            Err(error) => return NodeDaemonReply::Unreachable(format!("{authority}: {error}")),
        };
        let _ = stream.set_read_timeout(Some(NODE_DAEMON_TIMEOUT));
        let _ = stream.set_write_timeout(Some(NODE_DAEMON_TIMEOUT));
        let request = format!(
            "POST /api/sync HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: 0\r\nConnection: close\r\nX-Refine-API-Version: {API_CONTRACT_VERSION}\r\nIdempotency-Key: fleet-sync-{}\r\n\r\n",
            fleet_sync_idempotency_key()
        );
        if let Err(error) = stream.write_all(request.as_bytes()) {
            return NodeDaemonReply::Unreachable(format!("{authority}: {error}"));
        }
        let mut response = Vec::new();
        if let Err(error) = stream.read_to_end(&mut response) {
            // The request was already delivered, so the node may well be
            // running the sync it was asked for; what failed is this node's
            // read of the answer. Say so rather than implying nothing arrived.
            return NodeDaemonReply::Unreachable(format!(
                "{authority}: the sync request was delivered but no answer arrived within {}s: {error}",
                NODE_DAEMON_TIMEOUT.as_secs()
            ));
        }
        parse_node_daemon_response(&response).unwrap_or_else(|| {
            NodeDaemonReply::Unreachable(format!("{authority}: malformed response"))
        })
    }
}

fn fleet_sync_idempotency_key() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    )
}

fn parse_node_daemon_response(response: &[u8]) -> Option<NodeDaemonReply> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    let (head, body) = response.split_at(split);
    let body = &body[4..];
    let status = String::from_utf8_lossy(head)
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse::<u16>()
        .ok()?;
    let body = serde_json::from_slice::<serde_json::Value>(body).unwrap_or(serde_json::Value::Null);
    Some(NodeDaemonReply::Answered { status, body })
}

impl FileFleetService {
    /// Ask every other fleet node's daemon to synchronize, and report what
    /// each one answered. The caller has already synchronized this machine's
    /// own node in-process; a node that rejects this build's API contract is
    /// recorded as `pending_upgrade` and the pass continues, because a fleet
    /// upgrades one node at a time and the rest of it must keep working
    /// throughout. Nodes are asked in registry order and every request is
    /// bounded by [`NODE_DAEMON_TIMEOUT`], so an unreachable fleet costs that
    /// timeout per node and never hangs.
    pub fn sync_nodes(&self) -> RefineResult<FleetNodeSyncReport> {
        self.sync_nodes_with(node_daemon_client(&self.refine_dir).as_ref())
    }

    /// The statuses are this pass's own observation and stay in this pass's
    /// report. They are deliberately NOT written into `nodes.json`:
    ///
    /// - `node.health` is the node's provisioning verdict, written by
    ///   bootstrap and read by work distribution (`fleet distribute` and
    ///   `refine next` withhold work from a `failed` node). A two-second
    ///   HTTP answer must not decide that: a busy daemon would withhold work
    ///   from a healthy node, and a sync pass would erase the bootstrap
    ///   failure that is the reason an operator was told to reprovision.
    /// - `nodes.json` is synchronized state shared by the whole fleet, but
    ///   "worker-3 did not answer me" is a fact about the link between THIS
    ///   node and worker-3. Publishing it fleet-wide would have every node
    ///   overwriting every other node's contrary observation of the same
    ///   link on every pass.
    pub(crate) fn sync_nodes_with(
        &self,
        client: &dyn FleetNodeDaemonClient,
    ) -> RefineResult<FleetNodeSyncReport> {
        let registry = self.load_node_registry_with_legacy_fleet()?;
        let active_node_id = self.nodes().active_node_id().unwrap_or_default();
        let mut statuses = Vec::new();
        for node in registry.nodes.iter().filter(|node| !node.archived) {
            let status = if node.id == active_node_id {
                // This machine's own node: the calling pass already ran the
                // real pipeline in-process. Contacting its daemon would be
                // this command asking itself.
                FleetNodeSyncStatus::new(&node.id, NODE_SYNC_LOCAL, None)
            } else if !node.enabled {
                FleetNodeSyncStatus::new(&node.id, NODE_SYNC_DISABLED, None)
            } else if node.ssh_host.trim().is_empty() {
                FleetNodeSyncStatus::new(
                    &node.id,
                    NODE_SYNC_UNREACHABLE,
                    Some(format!(
                        "node {} has no host recorded; set one with refine fleet edit-node",
                        node.id
                    )),
                )
            } else {
                classify_node_reply(&node.id, client.sync_node(node))
            };
            statuses.push(status);
        }
        Ok(FleetNodeSyncReport::new(statuses))
    }
}

/// This node's own sync result with the fleet's per-node statuses attached.
/// The pass's own `ok` stays the result: a node that has not been upgraded
/// yet is reported under `nodes`, never promoted into a failure of the whole
/// command.
pub fn fleet_sync_response(
    sync: serde_json::Value,
    nodes: serde_json::Value,
    pending_upgrade: serde_json::Value,
) -> serde_json::Value {
    let mut sync = sync;
    if let Some(object) = sync.as_object_mut() {
        object.insert("nodes".to_string(), nodes);
        object.insert("pending_upgrade".to_string(), pending_upgrade);
        return sync;
    }
    serde_json::json!({
        "ok": true,
        "sync": sync,
        "nodes": nodes,
        "pending_upgrade": pending_upgrade
    })
}

/// Turn one daemon reply into that node's status. The API contract rejection
/// is the whole point: it is the only cross-version surface between nodes, so
/// it gets its own status instead of collapsing into a generic failure.
fn classify_node_reply(node_id: &str, reply: NodeDaemonReply) -> FleetNodeSyncStatus {
    match reply {
        NodeDaemonReply::Unreachable(detail) => {
            FleetNodeSyncStatus::new(node_id, NODE_SYNC_UNREACHABLE, Some(detail))
        }
        NodeDaemonReply::Answered { status: 426, body } => {
            let mut status = FleetNodeSyncStatus::new(
                node_id,
                NODE_SYNC_PENDING_UPGRADE,
                Some(format!(
                    "node {node_id} is still on the previous build and rejected API contract version {API_CONTRACT_VERSION}; upgrade it when its turn comes — its state keeps converging on refine/state until then"
                )),
            );
            status.api_contract_version = body
                .get("api_contract_version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            status
        }
        NodeDaemonReply::Answered { status, body } if status >= 400 => {
            let error = body.get("error");
            let message = error
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("node {node_id} answered HTTP {status}"));
            // A stable reason, not prose: an unsupported Git is a named
            // per-node condition an operator fixes in one step, so it does not
            // collapse into a generic failure any more than a pending upgrade
            // does.
            let unsupported_git = error
                .and_then(|error| error.get("reason"))
                .and_then(serde_json::Value::as_str)
                == Some("unsupported_git_version");
            FleetNodeSyncStatus::new(
                node_id,
                if unsupported_git {
                    NODE_SYNC_UNSUPPORTED_GIT
                } else {
                    NODE_SYNC_FAILED
                },
                Some(message),
            )
        }
        NodeDaemonReply::Answered { .. } => FleetNodeSyncStatus::new(
            node_id,
            NODE_SYNC_QUEUED,
            Some(format!(
                "node {node_id} accepted the request and queued its own sync; its result is reported by that node"
            )),
        ),
    }
}
